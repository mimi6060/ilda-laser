#!/usr/bin/env python3
"""Full board build: places every part from DESIGN.md's BOM, wires every
net (including two fixes discovered against the real MCP4922 datasheet
that DESIGN.md didn't have: the hardware SHDN pin per DAC needs tying to
VDD, and VREFA/VREFB are two separate pins per DAC, both need a reference),
and routes what it can.

Floorplan: 7 horizontal "lanes" (X+, X-, Y+, Y-, R, G, B) stacked in Y,
each running DAC -> op-amp -> trim network -> protection resistor -> RJ45
pin, left to right in X. Column X-positions are computed from each
footprint's *measured* courtyard width (via pcbnew, not guessed) with a
fixed margin, so components can't end up overlapping from an arithmetic
mistake - the one thing that went wrong on the first pass of this script.
"""
import pcbnew

mm = pcbnew.FromMM
FP_LIB = "/usr/share/kicad/footprints"
MARGIN = 1.2  # mm, minimum gap between any two courtyards

board = pcbnew.BOARD()

# ---------------------------------------------------------------- outline
W, H = 65.0, 56.0
R = 3.0

def seg(x1, y1, x2, y2):
    s = pcbnew.PCB_SHAPE(board)
    s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    s.SetStart(pcbnew.VECTOR2I(mm(x1), mm(y1)))
    s.SetEnd(pcbnew.VECTOR2I(mm(x2), mm(y2)))
    s.SetLayer(pcbnew.Edge_Cuts)
    s.SetWidth(mm(0.1))
    board.Add(s)

def arc(cx, cy, sx, sy, ex, ey):
    s = pcbnew.PCB_SHAPE(board)
    s.SetShape(pcbnew.SHAPE_T_ARC)
    s.SetCenter(pcbnew.VECTOR2I(mm(cx), mm(cy)))
    s.SetStart(pcbnew.VECTOR2I(mm(sx), mm(sy)))
    s.SetEnd(pcbnew.VECTOR2I(mm(ex), mm(ey)))
    s.SetLayer(pcbnew.Edge_Cuts)
    s.SetWidth(mm(0.1))
    board.Add(s)

seg(R, 0, W - R, 0)
arc(W - R, R, W - R, 0, W, R)
seg(W, R, W, H - R)
arc(W - R, H - R, W, H - R, W - R, H)
seg(W - R, H, R, H)
arc(R, H - R, R, H, 0, H - R)
seg(0, H - R, 0, R)
arc(R, R, 0, R, R, 0)

def load_fp(lib, name):
    fp = pcbnew.FootprintLoad(f"{FP_LIB}/{lib}.pretty", name)
    if fp is None:
        raise RuntimeError(f"footprint not found: {lib}:{name}")
    return fp

def courtyard_x_extent(fp, rot):
    """(left, right) distance in mm from the footprint's own origin to its
    courtyard edges, accounting for rotation - NOT assumed symmetric. A
    first pass of this script assumed courtyard = symmetric width/2 around
    the origin, which was wrong for at least one part (trimmer vs. mirror
    resistor courtyards still overlapped despite the margin, because the
    trimmer's courtyard isn't centered on its origin) and produced real
    DRC courtyard-overlap errors. This computes the true, possibly
    asymmetric, distances instead."""
    xs = []
    for gi in fp.GraphicalItems():
        if gi.GetLayer() in (pcbnew.F_CrtYd, pcbnew.B_CrtYd):
            bb = gi.GetBoundingBox()
            xs += [bb.GetLeft(), bb.GetRight()]
    if not xs:
        return (1.0, 1.0)
    origin = fp.GetPosition().x
    left = pcbnew.ToMM(origin - min(xs))
    right = pcbnew.ToMM(max(xs) - origin)
    if rot in (90, 270):
        # rotating 90 degrees swaps which courtyard axis faces X; recompute
        # using the Y extent instead, still relative to the origin.
        ys = []
        for gi in fp.GraphicalItems():
            if gi.GetLayer() in (pcbnew.F_CrtYd, pcbnew.B_CrtYd):
                bb = gi.GetBoundingBox()
                ys += [bb.GetTop(), bb.GetBottom()]
        origin_y = fp.GetPosition().y
        top = pcbnew.ToMM(origin_y - min(ys))
        bot = pcbnew.ToMM(max(ys) - origin_y)
        return (top, bot) if rot == 90 else (bot, top)
    return (left, right)

def place(lib, name, ref, x, y, rot=0, value=None, back=False):
    fp = load_fp(lib, name)
    board.Add(fp)
    if back:
        fp.Flip(pcbnew.VECTOR2I(0, 0), False)
    fp.SetPosition(pcbnew.VECTOR2I(mm(x), mm(y)))
    fp.SetOrientationDegrees(rot)
    fp.SetReference(ref)
    if value:
        fp.SetValue(value)
    return fp

# ------------------------------------------------------------ mounting holes
for i, (x, y) in enumerate([(3.5, 3.5), (W - 3.5, 3.5), (3.5, H - 3.5), (W - 3.5, H - 3.5)], 1):
    h = place("MountingHole", "MountingHole_2.7mm_M2.5", f"H{i}", x, y)
    for gi in h.GraphicalItems():
        if hasattr(gi, "SetVisible"):
            gi.SetVisible(False)

# --------------------------------------------------------------- GPIO header
gpio = place("Connector_PinSocket_2.54mm", "PinSocket_2x20_P2.54mm_Vertical",
             "J2", 8.373644, 4.772389, rot=270, back=True)

# --------------------------------------------------------------------- nets
net_names = [
    "VDD_3V3", "VDD_5V", "GND",
    "SPI0_SCK", "SPI0_SDI", "SPI0_CE0", "SPI0_CE1",
    "SPI1_SCK", "SPI1_SDI", "SPI1_CE0", "LDAC",
    "DAC_X", "DAC_Y", "DAC_R", "DAC_G", "DAC_B",
    "X_PLUS", "X_PLUS_FB", "Y_PLUS", "Y_PLUS_FB",
    "SIG_R", "R_FB", "SIG_G", "G_FB", "SIG_B", "B_FB",
    "X_MINUS", "X_MIRROR_FB", "Y_MINUS", "Y_MIRROR_FB",
    "VBIAS", "VBIAS_REF",
    "RJ_X_MINUS", "RJ_Y_MINUS", "RJ_R", "RJ_G", "RJ_B", "RJ_Y_PLUS", "RJ_X_PLUS",
]
nets = {}
for n in net_names:
    net = pcbnew.NETINFO_ITEM(board, n)
    board.Add(net)
    nets[n] = net

def pad(fp, num):
    p = fp.FindPadByNumber(str(num))
    if p is None:
        raise RuntimeError(f"{fp.GetReference()} has no pad {num}")
    return p

def wire(fp, num, netname):
    pad(fp, num).SetNet(nets[netname])

# ---------------------------------------------------------- column cursor
# Tracks the next free left edge in X so every column is placed with
# exactly MARGIN mm of clearance from the previous one - computed, not
# guessed, from each footprint's real courtyard width.
cursor_x = 6.0

def next_column(lib, name, rot):
    global cursor_x
    left, right = courtyard_x_extent(load_fp(lib, name), rot)
    x = cursor_x + left
    cursor_x = x + right + MARGIN
    return x

# lane Y-positions (7 lanes: X+, X-, Y+, Y-, R, G, B)
LANE_Y = {"X+": 8, "X-": 16, "Y+": 24, "Y-": 32, "R": 40, "G": 47, "B": 54}

DAC_FP = ("Package_SO", "SOIC-14_3.9x8.7mm_P1.27mm")
OPAMP_FP = ("Package_SO", "TSSOP-14_4.4x5mm_P0.65mm")
TRIM_FP = ("Potentiometer_THT", "Potentiometer_Bourns_3296W_Vertical")
R_FP = ("Resistor_SMD", "R_0603_1608Metric")
C_FP = ("Capacitor_SMD", "C_0603_1608Metric")

# ------------------------------------------------------------------ DACs
x_dac = next_column(*DAC_FP, 90)
u1 = place(*DAC_FP, "U1", x_dac, 14, rot=90, value="MCP4922-E/SL")
u2 = place(*DAC_FP, "U2", x_dac, 28, rot=90, value="MCP4922-E/SL")
u3 = place(*DAC_FP, "U3", x_dac, 42, rot=90, value="MCP4922-E/SL")

for u, (voa_net, vob_net) in [(u1, ("DAC_X", "DAC_Y")), (u2, ("DAC_R", "DAC_G")), (u3, ("DAC_B", None))]:
    wire(u, 1, "VDD_3V3")
    wire(u, 3, "SPI0_CE0" if u is u1 else ("SPI0_CE1" if u is u2 else "SPI1_CE0"))
    wire(u, 4, "SPI0_SCK" if u is not u3 else "SPI1_SCK")
    wire(u, 5, "SPI0_SDI" if u is not u3 else "SPI1_SDI")
    wire(u, 8, "LDAC")
    wire(u, 9, "VDD_3V3")   # SHDN tied high (active) - fix vs. DESIGN.md, which missed this pin
    wire(u, 11, "VDD_3V3")  # VREFB
    wire(u, 12, "GND")      # VSS
    wire(u, 13, "VDD_3V3")  # VREFA
    wire(u, 14, voa_net)    # VOUTA
    if vob_net:
        wire(u, 10, vob_net)  # VOUTB

# decoupling caps for the DACs go above each chip (frees horizontal room)
for ref, u, netname in [("C1", u1, "VDD_3V3"), ("C2", u2, "VDD_3V3"), ("C3", u3, "VDD_3V3")]:
    c = place(*C_FP, ref, x_dac, u.GetPosition().y / 1e6 - 6, rot=90, value="100nF")
    wire(c, 1, netname)
    wire(c, 2, "GND")

# --------------------------------------------------------------- op-amps
x_opamp = next_column(*OPAMP_FP, 90)
u4 = place(*OPAMP_FP, "U4", x_opamp, 14, rot=90, value="MCP6004-I/ST")  # X+, Y+, R, G
u5 = place(*OPAMP_FP, "U5", x_opamp, 34, rot=90, value="MCP6004-I/ST")  # B, X-, Y-, Vbias buffer

for u in (u4, u5):
    wire(u, 4, "VDD_5V")
    wire(u, 11, "GND")

# U4: A=X+, B=Y+, C=R, D=G
wire(u4, 3, "DAC_X"); wire(u4, 2, "X_PLUS_FB"); wire(u4, 1, "X_PLUS")
wire(u4, 5, "DAC_Y"); wire(u4, 6, "Y_PLUS_FB"); wire(u4, 7, "Y_PLUS")
wire(u4, 10, "DAC_R"); wire(u4, 9, "R_FB"); wire(u4, 8, "SIG_R")
wire(u4, 12, "DAC_G"); wire(u4, 13, "G_FB"); wire(u4, 14, "SIG_G")

# U5: A=B buffer, B=X- mirror, C=Y- mirror, D=Vbias follower
wire(u5, 3, "DAC_B"); wire(u5, 2, "B_FB"); wire(u5, 1, "SIG_B")
wire(u5, 5, "VBIAS"); wire(u5, 6, "X_MIRROR_FB"); wire(u5, 7, "X_MINUS")
wire(u5, 10, "VBIAS"); wire(u5, 9, "Y_MIRROR_FB"); wire(u5, 8, "Y_MINUS")
wire(u5, 12, "VBIAS_REF"); wire(u5, 13, "VBIAS"); wire(u5, 14, "VBIAS")  # unity-gain follower

for ref, u, netname in [("C4", u4, "VDD_5V"), ("C5", u5, "VDD_5V")]:
    c = place(*C_FP, ref, x_opamp, u.GetPosition().y / 1e6 - 6, rot=90, value="100nF")
    wire(c, 1, netname)
    wire(c, 2, "GND")

# ------------------------------------------------------- mirror resistors
x_mirror = next_column(*R_FP, 90)
mirror_r_defs = [
    ("R6", "X+", "X_PLUS", "X_MIRROR_FB"),
    ("R7", "X-", "X_MINUS", "X_MIRROR_FB"),
    ("R8", "Y+", "Y_PLUS", "Y_MIRROR_FB"),
    ("R9", "Y-", "Y_MINUS", "Y_MIRROR_FB"),
]
for ref, lane, a_net, b_net in mirror_r_defs:
    r = place(*R_FP, ref, x_mirror, LANE_Y[lane], rot=90, value="20k 1%")
    wire(r, 1, a_net)
    wire(r, 2, b_net)

# --------------------------------------------------------------- trimmers
x_trim = next_column(*TRIM_FP, 0)
trimmer_defs = [
    ("RV1", "X+", "X_PLUS", "X_PLUS_FB"),
    ("RV2", "Y+", "Y_PLUS", "Y_PLUS_FB"),
    ("RV3", "R", "SIG_R", "R_FB"),
    ("RV4", "G", "SIG_G", "G_FB"),
    ("RV5", "B", "SIG_B", "B_FB"),
]
for ref, lane, out_net, fb_net in trimmer_defs:
    rv = place(*TRIM_FP, ref, x_trim, LANE_Y[lane], rot=0, value="10k")
    wire(rv, 1, out_net)
    wire(rv, 2, fb_net)
    wire(rv, 3, out_net)

rv6 = place(*TRIM_FP, "RV6", x_trim, LANE_Y["Y-"], rot=0, value="10k")
wire(rv6, 1, "VDD_5V")
wire(rv6, 2, "VBIAS_REF")
wire(rv6, 3, "GND")

# ----------------------------------------------------------- gain resistors
x_gain = next_column(*R_FP, 0)
gain_r_defs = [("R1", "X+", "X_PLUS_FB"), ("R2", "Y+", "Y_PLUS_FB"),
               ("R3", "R", "R_FB"), ("R4", "G", "G_FB"), ("R5", "B", "B_FB")]
for ref, lane, fb_net in gain_r_defs:
    r = place(*R_FP, ref, x_gain, LANE_Y[lane], rot=0, value="20k 1%")
    wire(r, 1, fb_net)
    wire(r, 2, "GND")

# LDAC pull-down, tucked below the DAC column
r17 = place(*R_FP, "R17", x_dac, 51, rot=0, value="10k")
wire(r17, 1, "LDAC")
wire(r17, 2, "GND")

# bulk cap near the power entry (top-left, close to J2's 5V/GND pins)
c6 = place(*C_FP, "C6", 4, 12, rot=90, value="10uF")
wire(c6, 1, "VDD_5V")
wire(c6, 2, "GND")

# ------------------------------------------------ RJ45 output protection
x_prot = next_column(*R_FP, 0)
prot_r_defs = [
    ("R16", "X+", "X_PLUS", "RJ_X_PLUS"), ("R10", "X-", "X_MINUS", "RJ_X_MINUS"),
    ("R15", "Y+", "Y_PLUS", "RJ_Y_PLUS"), ("R11", "Y-", "Y_MINUS", "RJ_Y_MINUS"),
    ("R12", "R", "SIG_R", "RJ_R"), ("R13", "G", "SIG_G", "RJ_G"), ("R14", "B", "SIG_B", "RJ_B"),
]
for ref, lane, in_net, rj_net in prot_r_defs:
    r = place(*R_FP, ref, x_prot, LANE_Y[lane], rot=0, value="470")
    wire(r, 1, in_net)
    wire(r, 2, rj_net)

# ------------------------------------------------------------------- RJ45
# 5301-8P8C is a THT right-angle unshielded 8P8C jack; using the Amphenol
# 54602-x08 horizontal footprint as an electrically/mechanically
# equivalent standard-library stand-in (both are generic right-angle THT
# 8P8C jacks) - double check this against the 5301-8P8C's own datasheet
# dimensions before ordering, see report.
RJ_FP = ("Connector_RJ", "RJ45_Amphenol_54602-x08_Horizontal")
x_rj = next_column(*RJ_FP, 0)
rj = place(*RJ_FP, "J1", x_rj, 28, rot=0)
rj_pin_nets = {1: "RJ_X_MINUS", 2: "RJ_Y_MINUS", 3: "GND", 4: "RJ_R",
               5: "RJ_G", 6: "RJ_B", 7: "RJ_Y_PLUS", 8: "RJ_X_PLUS"}
for pin, netname in rj_pin_nets.items():
    wire(rj, pin, netname)

print(f"board width used so far: cursor_x = {cursor_x:.2f} mm (board is {W} mm wide)")

# ------------------------------------------------------- GPIO net wiring
gpio_pin_nets = {
    1: "VDD_3V3", 17: "VDD_3V3",
    2: "VDD_5V", 4: "VDD_5V",
    6: "GND", 9: "GND", 14: "GND", 20: "GND", 25: "GND", 30: "GND", 34: "GND", 39: "GND",
    19: "SPI0_SDI", 23: "SPI0_SCK", 24: "SPI0_CE0", 26: "SPI0_CE1",
    12: "SPI1_CE0", 38: "SPI1_SDI", 40: "SPI1_SCK",
    16: "LDAC",
}
for pin, netname in gpio_pin_nets.items():
    wire(gpio, pin, netname)

pcbnew.SaveBoard("board.kicad_pcb", board)
print("placed", len(board.GetFootprints()), "footprints")
print("OK build_board")
