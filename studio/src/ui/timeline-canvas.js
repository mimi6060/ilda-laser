// Laser Studio — timeline-canvas.js (T-435).
// Sections (as in the former index.html script):
//   drawing
//   mouse: select, move, resize, duplicate, markers, loop region, seek
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { tlAddMarker, tlDmxRow, tlMarkRow, tlNoteRow, tlRefRow, tlScanRow, tlSelected, tlSpreadRow } from './timeline-markers.js';
import { $, api, post } from './core.js';
import { LOCKED_FR, activeWs, flash, showMode } from './workspaces.js';
import { fmtTime } from './shows.js';
import { TL, TL_DMX_COLOR, TL_DRAG_PX, TL_EMPTY_TRACKS, TL_ENV, TL_LANE, TL_LAYER_COLORS, TL_MARK,
  TL_MIN_LEN, TL_RULER, TL_TOP, TL_WAVE, tlBarAt, tlBusAdd, tlCanvas, tlChange, tlCommit, tlCurveAt,
  tlDefaultLen, tlDmxAdd, tlEaseFr, tlEnvAddKey, tlEnvCycleEase, tlEnvDelKey, tlEnvEventAt,
  tlEnvFind, tlEnvGeom, tlEnvKeyAt, tlEnvLabel, tlEnvOf, tlEventName, tlEvents, tlFit, tlFmtV,
  tlGrid, tlGridLines, tlHeads, tlIgnoredMsg, tlLaneH, tlLaneY, tlLockBroken, tlLockedTi, tlMsg,
  tlNextId, tlPlayhead, tlPushUndo, tlSecAt, tlSeconds, tlSetView, tlSnapU, tlUToX, tlW, tlXToU } from './timeline-editor.js';

// --- drawing ---
let tlColors = null;
function tlDraw() {
  const t0p = performance.now();
  const c = tlCanvas(), dpr = window.devicePixelRatio || 1;
  const W = c.clientWidth, H = c.clientHeight;
  if (!W || !H) return;
  if (c.width !== Math.round(W * dpr) || c.height !== Math.round(H * dpr)) { c.width = Math.round(W * dpr); c.height = Math.round(H * dpr); }
  const g = c.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  if (!tlColors) { const cs = getComputedStyle(document.documentElement); tlColors = { line: cs.getPropertyValue('--line').trim(), muted: cs.getPropertyValue('--muted').trim(), accent: cs.getPropertyValue('--accent').trim(), text: cs.getPropertyValue('--text').trim(), panel: cs.getPropertyValue('--panel').trim(), tl: cs.getPropertyValue('--ws-timeline').trim() || '#3d9bff' }; }
  const C = tlColors;
  g.clearRect(0, 0, W, H);
  g.font = '11px system-ui, sans-serif';
  g.textBaseline = 'middle';
  const u0 = TL.t0, u1 = tlXToU(W);
  // Lanes (with no show: the empty tracks of the empty state).
  const lanes = TL.show ? TL.show.tracks.length : TL_EMPTY_TRACKS;
  for (let i = 0; i < lanes; i++) { g.fillStyle = i % 2 ? '#ffffff05' : '#00000030'; g.fillRect(0, tlLaneY(i), W, TL.show ? tlLaneH(i) : TL_LANE); }
  // Verrou de piste (T-445): locked lanes lightly hatched.
  if (TL.show) TL.show.tracks.forEach((t, i) => {
    if (!t.locked) return;
    const y = tlLaneY(i), h = tlLaneH(i);
    g.save(); g.beginPath(); g.rect(0, y, W, h); g.clip();
    g.strokeStyle = '#ffffff12'; g.beginPath();
    for (let x = -h; x < W; x += 8) { g.moveTo(x, y + h); g.lineTo(x + h, y); }
    g.stroke(); g.restore();
  });
  g.fillStyle = C.panel; g.fillRect(0, 0, W, TL_RULER);
  // Grid: bars bright, beats faint, labels in the ruler (bars on top, seconds below).
  const grid = tlGrid();
  let lastLabel = -1e9;
  for (const u of tlGridLines(u0, u1)) {
    const x = Math.round(tlUToX(u)) + 0.5, bar = tlBarAt(u), onBar = Math.abs(bar - Math.round(bar)) < 1e-6;
    g.strokeStyle = onBar ? '#ffffff26' : '#ffffff0d';
    g.beginPath(); g.moveTo(x, onBar ? 0 : TL_RULER / 2); g.lineTo(x, H); g.stroke();
    if (onBar && x - lastLabel > 34) { g.fillStyle = C.text; g.fillText(String(Math.round(bar) + 1), x + 3, 8); lastLabel = x; }
  }
  // Seconds row.
  const secPx = TL.pps * (tlSeconds() ? 1 : (TL.bpm || 120) / 60);
  let step = 1; for (const s of [1, 2, 5, 10, 15, 30, 60, 120, 300]) { step = s; if (s * secPx >= 60) break; }
  g.fillStyle = C.muted;
  const s0 = Math.floor(tlSecAt(u0) / step) * step;
  for (let s = s0; s <= tlSecAt(u1); s += step) {
    const u = tlSeconds() ? s : s * (TL.bpm || 120) / 60, x = tlUToX(u);
    g.fillRect(Math.round(x), TL_RULER - 5, 1, 5);
    g.fillText(fmtTime(s).replace(/\.0$/, ''), x + 3, TL_RULER - 8);
  }
  g.strokeStyle = C.line; g.beginPath(); g.moveTo(0, TL_RULER + 0.5); g.lineTo(W, TL_RULER + 0.5); g.moveTo(0, TL_RULER + TL_MARK + 0.5); g.lineTo(W, TL_RULER + TL_MARK + 0.5); g.moveTo(0, TL_TOP + 0.5); g.lineTo(W, TL_TOP + 0.5); g.stroke();
  if (!TL.show) { TL.draws++; return; }
  // Loop region.
  const r = TL.show.loop_region;
  if (r) {
    const a = tlUToX(r[0]), b = tlUToX(r[1]);
    g.fillStyle = TL.state && TL.state.loop ? '#3ddc8455' : '#3ddc8428'; g.fillRect(a, 0, b - a, TL_RULER);
    g.fillStyle = '#3ddc840c'; g.fillRect(a, TL_RULER, b - a, H - TL_RULER);
    g.fillStyle = C.accent; g.fillRect(a - 1, 0, 3, TL_RULER); g.fillRect(b - 2, 0, 3, TL_RULER);
  }
  // Waveform (Secondes shows with a song): the last fetched view, mapped onto this one.
  const w = TL.wave;
  if (w && TL.state && TL.state.audio && tlSeconds() && w.file === TL.state.audio.file) {
    const mid = TL_RULER + TL_MARK + TL_WAVE / 2, amp = TL_WAVE / 2 - 2, du = (w.to - w.from) / w.px;
    g.strokeStyle = C.accent; g.globalAlpha = 0.6; g.beginPath();
    for (let i = 0; i < w.max.length; i++) {
      const x = tlUToX(w.from + i * du);
      if (x < -1 || x > W + 1) continue;
      g.moveTo(Math.round(x) + 0.5, mid - w.max[i] * amp); g.lineTo(Math.round(x) + 0.5, Math.max(mid - w.min[i] * amp, mid - w.max[i] * amp + 1));
    }
    g.stroke(); g.globalAlpha = 1;
  } else {
    g.fillStyle = C.muted; g.fillText(tlSeconds() ? 'Pas de morceau' : 'Show en temps : pas de morceau', 8, TL_RULER + TL_MARK + TL_WAVE / 2);
  }
  // Events.
  const active = new Set(TL.state ? TL.state.active : []);
  TL.show.tracks.forEach((t, ti) => {
    const y = tlLaneY(ti) + 3, h = TL_LANE - 6, col = t.kind === 'dmx' ? TL_DMX_COLOR : TL_LAYER_COLORS[(t.layer - 1) % 4], bus = t.kind === 'bus';
    for (const e of t.events) {
      const x0 = tlUToX(e.start), x1 = tlUToX(e.start + e.len);
      if (x1 < 0 || x0 > W) continue;
      const sel = TL.sel.has(e.id);
      // A bus event plays nothing: outlined; its envelopes act on the layer.
      g.globalAlpha = bus ? (t.mute ? 0.12 : 0.25) : t.mute ? 0.35 : active.has(e.id) ? 1 : 0.75;
      g.fillStyle = col; g.fillRect(x0, y, Math.max(2, x1 - x0), h);
      g.globalAlpha = 1;
      if (bus) { g.strokeStyle = col; g.setLineDash([4, 3]); g.strokeRect(x0 + 0.5, y + 0.5, Math.max(2, x1 - x0) - 1, h - 1); g.setLineDash([]); }
      if (sel) { g.strokeStyle = '#fff'; g.lineWidth = 2; g.strokeRect(x0 + 1, y + 1, Math.max(2, x1 - x0) - 2, h - 2); g.lineWidth = 1; }
      // Annotated (T-430): a corner mark; the note shows on hover.
      if (e.notes || e.tags?.length) { g.fillStyle = '#ffb020'; g.beginPath(); g.moveTo(x1 - 8, y); g.lineTo(x1, y); g.lineTo(x1, y + 8); g.closePath(); g.fill(); }
      if (x1 - x0 > 24) {
        g.save(); g.beginPath(); g.rect(x0, y, x1 - x0, h); g.clip();
        g.fillStyle = bus ? C.text : '#06120b'; g.fillText(bus ? `Bus · calque ${t.layer}` : tlEventName(e), Math.max(x0, 0) + 4, y + h / 2);
        g.restore();
      }
    }
  });
  // Envelope lanes (T-163): each event's curve on the lane's parameter, its nodes, the value under the mouse.
  TL.show.tracks.forEach((t, ti) => {
    const id = tlEnvOf(ti);
    if (!id) return;
    const eg = tlEnvGeom(ti), top = tlLaneY(ti) + TL_LANE, col = TL_LAYER_COLORS[(t.layer - 1) % 4];
    g.fillStyle = '#00000038'; g.fillRect(0, top, W, TL_ENV);
    g.strokeStyle = C.line; g.setLineDash([2, 3]); g.beginPath(); g.moveTo(0, top + 0.5); g.lineTo(W, top + 0.5); g.stroke(); g.setLineDash([]);
    for (const e of t.events) {
      const x0 = tlUToX(e.start), x1 = tlUToX(e.start + e.len);
      if (x1 < 0 || x0 > W) continue;
      g.fillStyle = col; g.globalAlpha = 0.1; g.fillRect(x0, top + 2, x1 - x0, TL_ENV - 4); g.globalAlpha = 1;
      const env = tlEnvFind(e, id);
      if (!env || !env.keys.length) continue;
      const a = Math.max(x0, 0), b = Math.min(x1, W);
      g.strokeStyle = C.tl; g.lineWidth = 1.5; g.beginPath();
      for (let px = a; ; px = Math.min(b, px + 2)) {
        const py = eg.vy(tlCurveAt(env.keys, tlXToU(px) - e.start));
        if (px === a) g.moveTo(px, py); else g.lineTo(px, py);
        if (px >= b) break;
      }
      g.stroke(); g.lineWidth = 1;
      for (const k of env.keys) {
        const kx = tlUToX(e.start + k.at), ky = eg.vy(k.value);
        if (kx < -4 || kx > W + 4) continue;
        g.fillStyle = TL.drag && TL.drag.kind === 'envkey' && TL.drag.k === k ? C.text : C.tl;
        g.fillRect(kx - 3.5, ky - 3.5, 7, 7);
      }
    }
    g.fillStyle = C.muted;
    g.fillText(`${tlEnvLabel(id)} · ${eg.mode === 'relative' ? `Relatif ×${eg.min}…×${eg.max}` : `Absolu ${tlFmtV(eg.min)}…${tlFmtV(eg.max)}`}`, 4, top + 9);
  });
  const hv = TL.hover;
  if (hv && tlEnvOf(hv.ti)) {
    const d = TL.drag && TL.drag.kind === 'envkey' ? TL.drag : null;
    const hit = d || tlEnvKeyAt(hv.ti, hv.x, hv.y), e = hit ? hit.e : tlEnvEventAt(hv.ti, hv.x);
    const env = e && tlEnvFind(e, tlEnvOf(hv.ti));
    const text = hit ? `${tlFmtV(hit.k.value)} · ${tlEaseFr(hit.k.ease)}` : env && env.keys.length ? tlFmtV(tlCurveAt(env.keys, tlXToU(hv.x) - e.start)) : '';
    if (text) {
      const tw = g.measureText(text).width + 8, tx = Math.min(W - tw - 2, hv.x + 10), ty = Math.max(2, hv.y - 20);
      g.fillStyle = C.panel; g.fillRect(tx, ty, tw, 16); g.strokeStyle = C.tl; g.strokeRect(tx + 0.5, ty + 0.5, tw - 1, 15);
      g.fillStyle = C.text; g.fillText(text, tx + 4, ty + 8);
    }
  }
  // Drop preview.
  if (TL.drop) {
    const x0 = tlUToX(TL.drop.start), x1 = tlUToX(TL.drop.start + TL.drop.len);
    g.strokeStyle = '#fff'; g.setLineDash([4, 3]); g.strokeRect(x0, tlLaneY(TL.drop.ti) + 3, x1 - x0, TL_LANE - 6); g.setLineDash([]);
  }
  // Marquee.
  const d = TL.drag;
  if (d && d.kind === 'marquee') { g.strokeStyle = C.text; g.setLineDash([3, 3]); g.strokeRect(Math.min(d.x0, d.x1), Math.min(d.y0, d.y1), Math.abs(d.x1 - d.x0), Math.abs(d.y1 - d.y0)); g.setLineDash([]); }
  // Markers: flags in their lane, a line across the tracks. Labels never
  // overlap (T-509): each is cut to the room before the next flag, the
  // full name shows on hover (tlMarkerLabelAt).
  TL.markerLabels = tlMarkerLayout(TL.show.markers.map(m => ({ x: Math.round(tlUToX(m.at)) + 0.5, name: m.name })), W, s => g.measureText(s).width);
  TL.show.markers.forEach((m, i) => {
    const x = Math.round(tlUToX(m.at)) + 0.5;
    if (x < -60 || x > W + 2) return;
    const col = `rgb(${m.color.join(',')})`;
    g.strokeStyle = col; g.globalAlpha = 0.6; g.setLineDash([2, 3]); g.beginPath(); g.moveTo(x, TL_RULER + TL_MARK); g.lineTo(x, H); g.stroke(); g.setLineDash([]); g.globalAlpha = 1;
    g.fillStyle = col; g.beginPath(); g.moveTo(x, TL_RULER + 2); g.lineTo(x + 8, TL_RULER + 2); g.lineTo(x + 8, TL_RULER + TL_MARK - 6); g.lineTo(x, TL_RULER + TL_MARK - 2); g.closePath(); g.fill();
    if (i === TL.marker) { g.strokeStyle = '#fff'; g.stroke(); }
    const lab = TL.markerLabels[i];
    if (lab.text) { g.fillStyle = C.text; g.fillText(lab.text, x + 11, TL_RULER + TL_MARK / 2); }
  });
  // Playhead.
  if (TL.state && TL.state.name) {
    const x = Math.round(tlUToX(tlPlayhead())) + 0.5;
    g.strokeStyle = TL.state.waiting ? '#ffb020' : C.accent; g.lineWidth = 2; g.beginPath(); g.moveTo(x, 0); g.lineTo(x, H); g.stroke(); g.lineWidth = 1;
  }
  TL.draws++;
  TL.drawMs = TL.drawMs * 0.9 + (performance.now() - t0p) * 0.1;
}
// The waveform of the part in view, fetched again when the view settles.
let tlWaveTimer = 0;
function tlWaveWanted() {
  const a = TL.state && TL.state.audio;
  if (!a || !tlSeconds() || !TL.show) return;
  const W = Math.round(tlW()), u1 = tlXToU(W);
  const key = `${a.file}|${a.offset_s}|${TL.t0.toFixed(3)}|${u1.toFixed(3)}|${W}`;
  if (key === TL.waveKey) return;
  TL.waveKey = key;
  clearTimeout(tlWaveTimer);
  tlWaveTimer = setTimeout(async () => {
    try {
      const r = await api('GET', `/api/timeline/waveform?from=${TL.t0}&to=${u1}&px=${Math.max(1, Math.min(8192, W))}`);
      if (r.ok) { TL.wave = await r.json(); TL.need = true; }
    } catch { TL.waveKey = ''; }
  }, 120);
}
function tlLoop() {
  if (activeWs === 'timeline' && !$('tlEditor').hidden) {
    const t = TL.state;
    // Suivre: page the view along with the playhead.
    if (TL.follow && t && t.playing && !TL.drag) {
      const x = tlUToX(tlPlayhead()), W = tlW();
      if (x > W * 0.92 || x < 0) tlSetView(tlPlayhead() - (W * 0.08) / TL.pps, TL.pps);
    }
    tlHeads();
    tlScanRow();
    tlNoteRow();
    tlDmxRow();
    tlSpreadRow();
    tlRefRow();
    if (TL.fitPending && tlCanvas().clientWidth) { TL.fitPending = false; tlFit(); }
    tlWaveWanted();
    if (TL.need || (t && t.playing)) { TL.need = false; tlDraw(); }
  }
  requestAnimationFrame(tlLoop);
}

// --- mouse: select, move, resize, duplicate, markers, loop region, seek ---
function tlLocal(e) { const b = tlCanvas().getBoundingClientRect(); return { x: e.clientX - b.left, y: e.clientY - b.top }; }
function tlTrackAt(y) {
  if (!TL.show || y < TL_TOP) return Math.floor((y - TL_TOP) / TL_LANE);
  let top = TL_TOP;
  for (let i = 0; i < TL.show.tracks.length; i++) { const h = tlLaneH(i); if (y < top + h) return i; top += h; }
  return TL.show.tracks.length + Math.floor((y - top) / TL_LANE);
}
// In the envelope lane of track `ti` (below its events).
const tlInEnv = (ti, y) => !!tlEnvOf(ti) && y >= tlLaneY(ti) + TL_LANE && y < tlLaneY(ti) + tlLaneH(ti);
function tlEventAt(ti, x) {
  const t = TL.show && TL.show.tracks[ti];
  if (!t) return null;
  for (let i = t.events.length - 1; i >= 0; i--) {
    const e = t.events[i], x0 = tlUToX(e.start), x1 = tlUToX(e.start + e.len);
    if (x >= x0 - 2 && x <= x1 + 2) {
      const edge = x1 - x0 > 12 ? (x - x0 < 6 ? 'l' : x1 - x < 6 ? 'r' : null) : (x1 - x < 3 ? 'r' : null);
      return { e, ti, edge };
    }
  }
  return null;
}
function tlMarkerAt(x) {
  if (!TL.show) return -1;
  let best = -1, dist = 7;
  TL.show.markers.forEach((m, i) => { const d = Math.abs(tlUToX(m.at) - x + 4); if (d < dist) { dist = d; best = i; } });
  return best;
}
// Marker labels without overlap (T-509). `marks`: [{ x, name }] in time
// order, `W` the canvas width, `measure(text)` its width in px. Each label
// starts 11 px after its flag and may run up to 4 px before the next flag
// (or the right edge); a name that does not fit is cut with « … », and
// under 3 characters nothing is drawn (the flag stays, the name is on
// hover). Returns [{ text, x0, x1, cut }] with x0..x1 the drawn span.
function tlMarkerLayout(marks, W, measure) {
  const ELLIPSIS = '…', GAP = 11, BEFORE_NEXT = 4;
  return marks.map((m, i) => {
    const x0 = m.x + GAP, limit = (i + 1 < marks.length ? Math.max(marks[i + 1].x, m.x) : Math.max(W, m.x)) - BEFORE_NEXT;
    const room = limit - x0, name = String(m.name || '');
    if (measure(name) <= room) return { text: name, x0, x1: x0 + measure(name), cut: false };
    let n = name.length;
    while (n > 0 && measure(name.slice(0, n) + ELLIPSIS) > room) n--;
    const text = n >= 2 ? name.slice(0, n) + ELLIPSIS : '';
    return { text, x0, x1: text ? x0 + measure(text) : x0, cut: true };
  });
}
/** The marker whose flag or label is under x in the markers lane, or -1. */
function tlMarkerLabelAt(x) {
  if (!TL.show || !TL.markerLabels) return -1;
  const i = tlMarkerAt(x);
  if (i >= 0) return i;
  return TL.markerLabels.findIndex(l => l.text && x >= l.x0 - 2 && x <= l.x1 + 2);
}
function tlSeek(u) { post('/api/timeline/seek', { position: Math.max(0, u) }); }
// Move an event object to another track.
function tlMoveTo(e, ti) {
  for (const t of TL.show.tracks) { const i = t.events.indexOf(e); if (i >= 0) { t.events.splice(i, 1); break; } }
  TL.show.tracks[ti].events.push(e);
}
const tlCv = tlCanvas();
tlCv.addEventListener('pointerdown', e => {
  if (!TL.show || e.button !== 0) return;
  tlCv.focus();
  const { x, y } = tlLocal(e), u = tlXToU(x);
  const before = JSON.stringify(TL.show);
  tlCv.setPointerCapture(e.pointerId);
  // Mode spectacle (T-509): the tracks are read only; a press above them
  // only seeks, a press on them says why nothing moves.
  if (showMode) {
    if (y < TL_TOP) TL.drag = { kind: 'seek', x0: x, u0: u };
    else flash(LOCKED_FR);
    TL.need = true;
    return;
  }
  if (y < TL_RULER) {
    const r = TL.show.loop_region;
    const which = r ? (Math.abs(tlUToX(r[0]) - x) < 6 ? 0 : Math.abs(tlUToX(r[1]) - x) < 6 ? 1 : -1) : -1;
    TL.drag = which >= 0 ? { kind: 'loop', which, before, x0: x, y0: y } : { kind: 'ruler', before, x0: x, u0: u };
  } else if (y < TL_RULER + TL_MARK) {
    const i = tlMarkerAt(x);
    TL.marker = i; tlMarkRow();
    if (i >= 0) {
      const m = TL.show.markers[i];
      // Events snapped to the marker move with it (not those of a locked track).
      const stuck = tlEvents().filter(v => Math.abs(v.e.start - m.at) < 1e-6 && !v.t.locked).map(v => v.e);
      TL.drag = { kind: 'marker', m, at0: m.at, stuck, before, x0: x, y0: y };
    }
  } else if (y < TL_TOP) {
    TL.drag = { kind: 'seek', x0: x, u0: u };
  } else if (tlInEnv(tlTrackAt(y), y)) {
    // Envelope lane: grab a node (double-click adds one, right click deletes it).
    const ti = tlTrackAt(y), hit = tlEnvKeyAt(ti, x, y);
    TL.drag = hit && !tlLockedTi(ti) ? { kind: 'envkey', ti, ...hit, at0: hit.k.at, before, x0: x, y0: y } : null;
    if (hit && !TL.drag) tlMsg('Piste verrouillée', true);
    TL.marker = -1; tlMarkRow();
  } else {
    const ti = tlTrackAt(y), hit = tlEventAt(ti, x);
    if (hit) {
      if (e.shiftKey) { TL.sel.has(hit.e.id) ? TL.sel.delete(hit.e.id) : TL.sel.add(hit.e.id); TL.need = true; TL.drag = null; return; }
      if (!TL.sel.has(hit.e.id)) { TL.sel.clear(); TL.sel.add(hit.e.id); }
      let primary = hit.e, dup = false;
      // Verrou de piste (T-445): what is on a locked track stays selected but never moves.
      const ignored = hit.edge ? 0 : tlSelected().filter(s => s.t.locked).length;
      if (e.altKey && !hit.edge) {
        // Alt+glisser: the copies move, the originals stay.
        let id = tlNextId();
        const copies = new Set();
        for (const s of tlSelected()) {
          if (s.t.locked) continue;
          const c = { ...JSON.parse(JSON.stringify(s.e)), id: id++ };
          s.t.events.push(c); copies.add(c.id);
          if (s.e === hit.e) primary = c;
        }
        if (copies.size) { TL.sel = copies; dup = true; }
      }
      const orig = tlSelected().filter(s => !s.t.locked).map(s => ({ e: s.e, ti: s.ti, start: s.e.start, len: s.e.len }));
      const frozen = hit.edge ? tlLockedTi(ti) : !orig.length;
      TL.drag = frozen ? { kind: 'locked', x0: x, y0: y }
        : { kind: hit.edge ? 'resize' : 'move', edge: hit.edge, primary, p0: { start: primary.start, len: primary.len }, orig, ti0: ti, x0: x, y0: y, before, dup, ignored };
    } else {
      if (!e.shiftKey) TL.sel.clear();
      TL.drag = { kind: 'marquee', x0: x, y0: y, x1: x, y1: y, base: new Set(TL.sel) };
    }
    TL.marker = -1; tlMarkRow();
  }
  TL.need = true;
});
tlCv.addEventListener('pointermove', e => {
  const { x, y } = tlLocal(e), d = TL.drag;
  TL.pointerTi = TL.show && y >= TL_TOP ? tlTrackAt(y) : -1; // Alt+L
  // The value under the mouse in an envelope lane (and while dragging a node).
  const envTi = TL.show && y >= TL_TOP ? tlTrackAt(y) : -1;
  const hover = d && d.kind === 'envkey' ? { ti: d.ti, x, y } : envTi >= 0 && tlInEnv(envTi, y) ? { ti: envTi, x, y } : null;
  if (hover || TL.hover) { TL.hover = hover; TL.need = true; }
  if (!d) {
    if (hover) { tlCv.style.cursor = tlEnvKeyAt(hover.ti, x, y) ? 'move' : 'crosshair'; return; }
    const hit = TL.show && y >= TL_TOP ? tlEventAt(tlTrackAt(y), x) : null;
    // In the markers lane: the marker's full name (its label may be cut).
    const mi = TL.show && y >= TL_RULER && y < TL_RULER + TL_MARK ? tlMarkerLabelAt(x) : -1;
    const tip = mi >= 0 ? TL.show.markers[mi].name
      : hit && (hit.e.notes || hit.e.tags?.length) ? [hit.e.tags?.length ? '# ' + hit.e.tags.join(', ') : '', hit.e.notes || ''].filter(Boolean).join('\n') : '';
    tlCv.dataset.baseTitle ??= tlCv.title;
    if (tlCv.title !== (tip || tlCv.dataset.baseTitle)) tlCv.title = tip || tlCv.dataset.baseTitle;
    const r = TL.show && TL.show.loop_region;
    const onEdge = r && y < TL_RULER && (Math.abs(tlUToX(r[0]) - x) < 6 || Math.abs(tlUToX(r[1]) - x) < 6);
    tlCv.style.cursor = hit && tlLockedTi(hit.ti) ? 'not-allowed' : hit && hit.edge || onEdge ? 'ew-resize' : hit ? 'grab' : y < TL_RULER ? 'pointer' : 'default';
    return;
  }
  // Seuil anti-glissement (T-445): under TL_DRAG_PX from the press, a
  // press only selects; past it, the drag starts from the press (no jump).
  if (TL_THRESHOLD_KINDS.includes(d.kind) && !d.started) {
    if (Math.hypot(x - d.x0, y - d.y0) < TL_DRAG_PX) return;
    d.started = d.moved = true;
    if (d.kind === 'locked') tlMsg('Piste verrouillée', true);
    else if (d.ignored) tlMsg(tlIgnoredMsg(d.ignored));
  }
  if (d.kind === 'locked') return;
  if (Math.abs(x - d.x0) > 2) d.moved = true;
  if (d.kind === 'envkey') {
    // Within its event; its time keeps still until the mouse really moves sideways.
    if (Math.abs(y - d.y0) > 2) d.moved = true;
    const u = TL.snap === 'off' ? tlXToU(x) : tlSnapU(tlXToU(x));
    d.k.at = Math.abs(x - d.x0) > 2 ? Math.max(0, Math.min(d.e.len, u - d.e.start)) : d.at0;
    d.k.value = +tlEnvGeom(d.ti).yv(y).toFixed(4);
    TL.need = true;
    return;
  }
  const du = (x - d.x0) / TL.pps;
  if (d.kind === 'move') {
    const skip = new Set(d.orig.map(o => o.e.id)).add(d.primary.id);
    let delta = tlSnapU(d.p0.start + du, skip) - d.p0.start;
    delta = Math.max(delta, -Math.min(...d.orig.map(o => o.start)));
    const n = TL.show.tracks.length;
    let dt = Math.max(0, Math.min(n - 1, tlTrackAt(y))) - d.ti0;
    dt = Math.max(dt, -Math.min(...d.orig.map(o => o.ti)));
    dt = Math.min(dt, n - 1 - Math.max(...d.orig.map(o => o.ti)));
    // Never onto a locked track: the events stay on the last free one.
    if (d.orig.some(o => tlLockedTi(o.ti + dt))) dt = d.dt || 0;
    d.dt = dt;
    if (dt) d.moved = true;
    for (const o of d.orig) {
      o.e.start = o.start + delta;
      const cur = TL.show.tracks.findIndex(t => t.events.includes(o.e));
      if (cur !== o.ti + dt) tlMoveTo(o.e, o.ti + dt);
    }
  } else if (d.kind === 'resize') {
    const e0 = d.p0, skip = new Set([d.primary.id]);
    if (d.edge === 'r') {
      const end = tlSnapU(e0.start + e0.len + du, skip);
      d.primary.len = Math.max(TL_MIN_LEN, end - e0.start);
    } else {
      const end = e0.start + e0.len, start = Math.min(end - TL_MIN_LEN, tlSnapU(e0.start + du, skip));
      d.primary.start = Math.max(0, start); d.primary.len = end - d.primary.start;
    }
  } else if (d.kind === 'marker') {
    const at = TL.snap === 'off' ? Math.max(0, d.at0 + du) : tlSnapU(d.at0 + du, new Set(d.stuck.map(s => s.id)));
    d.m.at = Math.max(0, at);
    for (const s of d.stuck) s.start = d.m.at;
  } else if (d.kind === 'loop') {
    const r = TL.show.loop_region, v = tlSnapU(tlXToU(x));
    r[d.which] = v;
    if (r[0] > r[1]) { r.reverse(); d.which = 1 - d.which; }
  } else if (d.kind === 'ruler' && d.moved) {
    const a = tlSnapU(d.u0), b = tlSnapU(tlXToU(x));
    TL.show.loop_region = [Math.min(a, b), Math.max(a, b)];
  } else if (d.kind === 'marquee') {
    d.x1 = x; d.y1 = y;
    const ua = tlXToU(Math.min(d.x0, x)), ub = tlXToU(Math.max(d.x0, x));
    const ta = tlTrackAt(Math.min(d.y0, y)), tb = tlTrackAt(Math.max(d.y0, y));
    TL.sel = new Set(d.base);
    for (const v of tlEvents()) if (v.ti >= ta && v.ti <= tb && v.e.start < ub && v.e.start + v.e.len > ua) TL.sel.add(v.e.id);
  }
  TL.need = true;
});
const TL_THRESHOLD_KINDS = ['move', 'resize', 'marker', 'loop', 'locked'];
function tlPointerUp(e) {
  const d = TL.drag;
  if (!d) return;
  TL.drag = null;
  if (d.kind === 'ruler' && !d.moved) tlSeek(d.u0);
  else if (d.kind === 'seek' && !d.moved) tlSeek(d.u0);
  else if (d.kind === 'envkey') {
    if (d.moved) {
      d.env.keys.sort((a, b) => a.at - b.at);
      tlPushUndo(d.before);
      tlCommit();
    }
  }
  else if (['move', 'resize', 'marker', 'loop', 'ruler'].includes(d.kind)) {
    if (d.kind === 'ruler' && TL.show.loop_region && TL.show.loop_region[1] - TL.show.loop_region[0] < 1e-6) TL.show.loop_region = null;
    if (d.moved && JSON.stringify(TL.show) !== d.before) {
      if (d.kind === 'marker') TL.show.markers.sort((a, b) => a.at - b.at), TL.marker = TL.show.markers.indexOf(d.m);
      if (tlLockBroken(JSON.parse(d.before), TL.show)) { TL.show = JSON.parse(d.before); tlMsg('Piste verrouillée', true); } // never expected
      else { tlPushUndo(d.before); tlCommit(); }
    } else if (d.dup) {
      TL.show = JSON.parse(d.before); TL.sel.clear(); // Alt+click without a move: no copy
    }
  }
  TL.need = true;
}
tlCv.addEventListener('pointerup', tlPointerUp);
tlCv.addEventListener('pointercancel', tlPointerUp);
tlCv.addEventListener('dblclick', e => {
  if (!TL.show) return;
  if (showMode) { flash(LOCKED_FR); return; }
  const { x, y } = tlLocal(e);
  if (y < TL_RULER) tlChange(() => { TL.show.loop_region = null; });
  else if (y < TL_RULER + TL_MARK && tlMarkerAt(x) < 0) tlAddMarker(tlSnapU(tlXToU(x)));
  else if (y >= TL_TOP) {
    const ti = tlTrackAt(y), t = TL.show.tracks[ti];
    if (!t) return;
    if (tlInEnv(ti, y)) {
      // Envelope lane: a node here; on a node, its next curve.
      const hit = tlEnvKeyAt(ti, x, y);
      if (hit) tlEnvCycleEase(hit); else tlEnvAddKey(ti, x, y);
    } else if (t.kind === 'bus' && !tlEventAt(ti, x)) tlBusAdd(ti, x);
    else if (t.kind === 'dmx' && !tlEventAt(ti, x)) tlDmxAdd(ti, x);
  }
});
// Molette: zoom around the cursor; Maj+molette or horizontal swipe: scroll.
tlCv.addEventListener('wheel', e => {
  if (!TL.show) return;
  e.preventDefault();
  const { x } = tlLocal(e);
  if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
    tlSetView(TL.t0 + (e.shiftKey ? e.deltaY || e.deltaX : e.deltaX) / TL.pps, TL.pps);
  } else {
    const u = tlXToU(x), pps = Math.max(0.5, Math.min(2000, TL.pps * Math.exp(-e.deltaY * 0.0015)));
    tlSetView(u - x / pps, pps);
  }
}, { passive: false });
// Drag from the library.
function tlDropAt(e) {
  const { x, y } = tlLocal(e), ti = tlTrackAt(y);
  if (!TL.show || !TL.dragCue || y < TL_TOP || ti < 0 || ti >= TL.show.tracks.length || TL.show.tracks[ti].kind !== 'cues') return null;
  if (TL.show.tracks[ti].locked) { tlMsg('Piste verrouillée', true); return null; }
  const start = tlSnapU(tlXToU(x));
  return { ti, start, len: tlDefaultLen(TL.dragCue, start), id: TL.dragCue };
}
tlCv.addEventListener('dragover', e => {
  const d = tlDropAt(e);
  TL.drop = d; TL.need = true;
  if (d) { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; }
});
tlCv.addEventListener('dragleave', () => { TL.drop = null; TL.need = true; });
tlCv.addEventListener('drop', e => {
  e.preventDefault();
  const d = tlDropAt(e);
  TL.drop = null; TL.dragCue = null;
  if (!d) return;
  tlChange(() => {
    const ev = { id: tlNextId(), start: d.start, len: d.len, source: { kind: 'cue', id: d.id } };
    TL.show.tracks[d.ti].events.push(ev);
    TL.sel = new Set([ev.id]);
  });
  tlCv.focus();
});
// Keys of the timeline: in the shortcut table (T-284).
new ResizeObserver(() => { TL.need = true; }).observe(tlCanvas());
// For the e2e tests: the view and the draw time.
// Clic droit in an envelope lane: deletes the node under the mouse.
tlCv.addEventListener('contextmenu', e => {
  if (!TL.show) return;
  const { x, y } = tlLocal(e), ti = tlTrackAt(y);
  if (y < TL_TOP || !tlInEnv(ti, y)) return;
  e.preventDefault();
  const hit = tlEnvKeyAt(ti, x, y);
  if (hit) tlEnvDelKey(hit);
});
tlCv.addEventListener('pointerleave', () => { if (TL.hover && !TL.drag) { TL.hover = null; TL.need = true; } });
window.tlDebug = {
  view: () => ({ t0: TL.t0, pps: TL.pps, top: TL_TOP, lane: TL_LANE, ruler: TL_RULER, mark: TL_MARK }), setView: tlSetView, stats: () => ({ drawMs: TL.drawMs, draws: TL.draws }), show: () => TL.show,
  // Envelope lanes (T-163): the top of a track's lane, and the value ↔ y mapping of its envelope lane.
  laneY: ti => tlLaneY(ti),
  dragPx: TL_DRAG_PX,
  env: ti => { if (!tlEnvOf(ti)) return null; const g = tlEnvGeom(ti); return { id: g.id, mode: g.mode, min: g.min, max: g.max, y0: g.y0, y1: g.y1 }; },
};
requestAnimationFrame(tlLoop);

export { tlCv, tlMarkerLayout };
