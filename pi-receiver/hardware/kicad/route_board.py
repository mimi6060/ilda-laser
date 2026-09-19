#!/usr/bin/env python3
"""Loads board.kicad_pcb (placed, all nets assigned, unrouted) and routes
every net with simple Manhattan (L-shaped) traces, pad to pad. Not an
optimal/production autorouter - a straightforward "connect the dots"
pass, using vias to hop to B.Cu wherever a trace would otherwise cross
another already-routed trace on the same layer. Multi-pad nets (GND,
VDD_3V3, VDD_5V, LDAC) are routed as a chain through their pads in
position order, which is not shortest-path optimal but is electrically
correct.
"""
import itertools
import pcbnew

mm = pcbnew.FromMM
TRACE_W = mm(0.25)
VIA_D = mm(0.6)
VIA_DRILL = mm(0.3)
CLEARANCE = mm(0.15)

board = pcbnew.LoadBoard("board.kicad_pcb")

placed_segments = []  # (layer, (x1,y1), (x2,y2)) for naive crossing checks

def segments_cross(a1, a2, b1, b2):
    def ccw(p, q, r):
        return (r[1] - p[1]) * (q[0] - p[0]) > (q[1] - p[1]) * (r[0] - p[0])
    return ccw(a1, b1, b2) != ccw(a2, b1, b2) and ccw(a1, a2, b1) != ccw(a1, a2, b2)

def would_cross(layer, p1, p2):
    for l, s1, s2 in placed_segments:
        if l != layer:
            continue
        if segments_cross(p1, p2, s1, s2):
            return True
    return False

def add_track(p1, p2, layer, net):
    t = pcbnew.PCB_TRACK(board)
    t.SetStart(pcbnew.VECTOR2I(*p1))
    t.SetEnd(pcbnew.VECTOR2I(*p2))
    t.SetWidth(TRACE_W)
    t.SetLayer(layer)
    t.SetNet(net)
    board.Add(t)
    placed_segments.append((layer, p1, p2))

placed_vias = {}  # (net_code, pos) -> True, to avoid duplicate/co-located vias

def add_via(pos, net):
    key = (net.GetNetCode(), pos)
    if key in placed_vias:
        return
    placed_vias[key] = True
    v = pcbnew.PCB_VIA(board)
    v.SetPosition(pcbnew.VECTOR2I(*pos))
    v.SetWidth(VIA_D)
    v.SetDrill(VIA_DRILL)
    v.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu)
    v.SetNet(net)
    board.Add(v)

def try_l(p1, p2, corner, layer):
    return (not would_cross(layer, p1, corner)) and (not would_cross(layer, corner, p2))

def commit_l(p1, p2, corner, layer, net):
    if p1 != corner:
        add_track(p1, corner, layer, net)
    if corner != p2:
        add_track(corner, p2, layer, net)

def route_l(p1, p2, net):
    """Route an L-shaped (or straight) path from p1 to p2, trying both L
    orientations on F.Cu, then both on B.Cu (via a via pair), before
    giving up and forcing an F.Cu route (logged as a likely DRC issue for
    a human to fix in KiCad's interactive router)."""
    corner_a = (p2[0], p1[1])
    corner_b = (p1[0], p2[1])

    for corner in (corner_a, corner_b):
        if try_l(p1, p2, corner, pcbnew.F_Cu):
            commit_l(p1, p2, corner, pcbnew.F_Cu, net)
            return True
    for corner in (corner_a, corner_b):
        if try_l(p1, p2, corner, pcbnew.B_Cu):
            add_via(p1, net)
            add_via(p2, net)
            commit_l(p1, p2, corner, pcbnew.B_Cu, net)
            return True
    # Nothing avoided a crossing - place it on F.Cu anyway so the net is
    # at least electrically connected, and report it as needing a manual
    # re-route.
    commit_l(p1, p2, corner_a, pcbnew.F_Cu, net)
    return False

def pad_pos(pad):
    p = pad.GetPosition()
    return (p.x, p.y)

def dist(a, b):
    return abs(a[0] - b[0]) + abs(a[1] - b[1])

skip_nets = {"", "unconnected"}
jobs = []  # (net, pos_a, pos_b) - collected first so all nets can be
           # globally sorted shortest-first, which tends to leave the
           # long cross-board runs (more likely to need a layer hop) for
           # last, after the short local ones have already claimed the
           # direct paths.
for net in board.GetNetInfo().NetsByName().values():
    name = net.GetNetname()
    if not name or name in skip_nets:
        continue
    pads = [p for p in board.GetPads() if p.GetNetname() == name]
    positions = sorted({pad_pos(p) for p in pads})
    if len(positions) < 2:
        continue
    for a, b in zip(positions, positions[1:]):
        jobs.append((net, a, b))

jobs.sort(key=lambda j: dist(j[1], j[2]))

clean = 0
forced = []
for net, a, b in jobs:
    ok = route_l(a, b, net)
    if ok:
        clean += 1
    else:
        forced.append((net.GetNetname(), a, b))

print(f"routed {len(jobs)} connections: {clean} cleanly, {len(forced)} forced (likely crossing)")
for name, a, b in forced[:30]:
    print("  FORCED:", name, a, b)

pcbnew.SaveBoard("board_routed.kicad_pcb", board)
print("OK route_board")
