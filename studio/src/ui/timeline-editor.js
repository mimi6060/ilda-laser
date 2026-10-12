// Laser Studio — timeline-editor.js (T-435).
// Sections (as in the former index.html script):
//   timeline editor (T-162)
//   musical time: the show's tempo map (Secondes) or the live meter (Temps)
//   view
//   snapping (Magnétisme)
//   show mirror, server sync, undo
//   Verrou de piste (T-445)
//   library (cue pages, figures, evolving cues)
//   envelopes (T-163): a curve on a control per event, in a lane under the track
//   track heads (DOM, aligned with the canvas lanes)
//   toolbar
//   built-in examples (T-124 to T-129): read-only, opened as a copy
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { lastTimeline, loadShows, shows } from './shows.js';
import { $, api, apiJson, post } from './core.js';
import { activeWs, remember, remembered } from './workspaces.js';
import { cueCategories, presets } from './cues.js';
import { assignTlNoteKey, assignTlRefKey, assignTlScanKey, tlSelected } from './timeline-markers.js';
import { GEN_NAMES } from './layers.js';
import { dmxOutChannels, dmxOutNames } from './outputs.js';
import { esc } from './midi.js';

// ---------- timeline editor (T-162) ----------
// The page keeps a mirror of the loaded show (`TL.show`), edits it, and
// sends every finished change to POST /api/timeline/edit, which validates
// it and swaps it into the player without stopping playback (events go
// through the usual layers → live → calibration → safety → gate path).
// Undo/redo are snapshots of that mirror (50 levels). Enregistrer writes
// the show file; nothing here reads or writes the arm state.
const TL = {
  show: null, name: null, rev: -1, state: null, stateAt: 0, bpm: 120,
  undo: [], redo: [], sel: new Set(), marker: -1, clip: null,
  snap: remembered('laserStudio.tlSnap') || 'strong', follow: true,
  t0: 0, pps: 40, drag: null, drop: null, need: true,
  inflight: 0, fetching: false, drawMs: 0, draws: 0, libFor: null, headsKey: '',
  // Envelope lanes (T-163): track index → the parameter shown under it;
  // the mode new envelopes get there; the targets (GET /api/lfos); hover.
  env: {}, envMode: {}, envTargets: null, hover: null,
};
const TL_RULER = 30, TL_MARK = 18, TL_WAVE = 40, TL_LANE = 34, TL_TOP = TL_RULER + TL_MARK + TL_WAVE;
const TL_UNDO = 50, TL_MIN_LEN = 0.01, TL_SNAP_PX = 8, TL_ENV = 48, TL_ENV_PAD = 6;
// Seuil anti-glissement (T-445): a press must move this far (CSS pixels,
// whatever the zoom) before an event, a marker or a loop edge moves.
const TL_DRAG_PX = 4;
const TL_LAYER_COLORS = ['#3ddc84', '#4da3ff', '#ffb020', '#e062ff'];
// DMX tracks (T-453): one colour of their own, not a layer's.
const TL_DMX_COLOR = '#c8ccd8';
const TL_MARK_COLORS = [[255, 176, 32], [77, 163, 255], [224, 98, 255], [61, 220, 132], [255, 77, 79]];
const tlSeconds = () => !TL.show || TL.show.time_base !== 'beats';
// Empty state: the ruler and this many empty tracks (no show loaded).
const TL_EMPTY_TRACKS = 4;

// --- musical time: the show's tempo map (Secondes) or the live meter (Temps) ---
function tlSegments() {
  const tm = TL.show && TL.show.tempo_map && TL.show.tempo_map.length ? TL.show.tempo_map : [{ at_s: 0, bpm: 120, beats_per_bar: 4 }];
  const out = [];
  let beats = 0, bars = 0;
  tm.forEach((p, i) => {
    const from = i === 0 ? 0 : p.at_s, to = i + 1 < tm.length ? tm[i + 1].at_s : Infinity;
    out.push({ from, to, bpm: p.bpm, bpb: p.beats_per_bar, beat0: beats, bar0: bars });
    if (to < Infinity) { beats += (to - from) * p.bpm / 60; bars += (to - from) * p.bpm / 60 / p.beats_per_bar; }
  });
  return out;
}
function tlLiveBpb() { return (TL.state && TL.state.beats_per_bar) || 4; }
function tlBeatAt(u) {
  if (!tlSeconds()) return u;
  const s = tlSegments().find(g => u < g.to);
  return s.beat0 + (u - s.from) * s.bpm / 60;
}
function tlUAtBeat(b) {
  if (!tlSeconds()) return b;
  const segs = tlSegments();
  const s = segs.find(g => b < g.beat0 + (g.to - g.from) * g.bpm / 60) || segs[segs.length - 1];
  return s.from + (b - s.beat0) * 60 / s.bpm;
}
function tlBarAt(u) {
  if (!tlSeconds()) return u / tlLiveBpb();
  const s = tlSegments().find(g => u < g.to);
  return s.bar0 + (u - s.from) * s.bpm / 60 / s.bpb;
}
function tlUAtBar(bar) {
  if (!tlSeconds()) return bar * tlLiveBpb();
  const segs = tlSegments();
  const s = segs.find(g => bar < g.bar0 + (g.to - g.from) * g.bpm / 60 / g.bpb) || segs[segs.length - 1];
  return s.from + (bar - s.bar0) * s.bpb * 60 / s.bpm;
}
const tlSecAt = u => tlSeconds() ? u : u * 60 / (TL.bpm || 120);
function tlEnd() {
  let end = 0;
  if (TL.show) for (const t of TL.show.tracks) for (const e of t.events) end = Math.max(end, e.start + e.len);
  if (TL.state && TL.state.length) end = Math.max(end, TL.state.length);
  return end;
}

// --- view ---
const tlCanvas = () => $('tlCanvas');
const tlW = () => tlCanvas().clientWidth || 1;
const tlUToX = u => (u - TL.t0) * TL.pps;
const tlXToU = x => TL.t0 + x / TL.pps;
// Lanes have their own height: a track with an open envelope lane is taller.
const tlEnvOf = ti => (TL.show && TL.show.tracks[ti] && TL.env[ti]) || null;
const tlLaneH = ti => TL_LANE + (tlEnvOf(ti) ? TL_ENV : 0);
function tlLaneY(i) {
  if (!TL.show) return TL_TOP + i * TL_LANE;
  let y = TL_TOP;
  for (let k = 0; k < i && k < TL.show.tracks.length; k++) y += tlLaneH(k);
  return y + Math.max(0, i - TL.show.tracks.length) * TL_LANE;
}
function tlSetView(t0, pps) {
  TL.fitPending = false;
  TL.pps = Math.max(0.5, Math.min(2000, pps));
  TL.t0 = Math.max(0, t0);
  TL.need = true;
}
function tlFit() {
  const end = Math.max(tlEnd(), tlSeconds() ? 30 : 32);
  tlSetView(0, tlW() / (end * 1.03));
}
// The grid unit: beats when they are far enough apart, else bars (or groups of bars).
function tlGrid() {
  const beatPx = (tlUAtBeat(tlBeatAt(TL.t0) + 1) - tlUAtBeat(tlBeatAt(TL.t0))) * TL.pps;
  if (beatPx >= 12) return { kind: 'beat', step: 1 };
  const barPx = (tlUAtBar(tlBarAt(TL.t0) + 1) - tlUAtBar(tlBarAt(TL.t0))) * TL.pps;
  let step = 1;
  while (barPx * step < 12) step *= 2;
  return { kind: 'bar', step };
}
function tlGridLines(u0, u1) {
  const g = tlGrid(), out = [];
  const at = g.kind === 'beat' ? tlBeatAt : tlBarAt, back = g.kind === 'beat' ? tlUAtBeat : tlUAtBar;
  let n = Math.floor(Math.max(0, at(Math.max(0, u0))) / g.step) * g.step;
  for (let i = 0; i < 4000; i++, n += g.step) {
    const u = back(n);
    if (u > u1) break;
    if (u >= u0 - 1e-9) out.push(u);
  }
  return out;
}

// --- snapping (Magnétisme) ---
function tlSnapU(u, skip = new Set()) {
  if (TL.snap === 'off') return Math.max(0, u);
  const tol = TL_SNAP_PX / TL.pps;
  const g = tlGrid(), at = g.kind === 'beat' ? tlBeatAt : tlBarAt, back = g.kind === 'beat' ? tlUAtBeat : tlUAtBar;
  const n = at(Math.max(0, u));
  const grid = [back(Math.floor(n / g.step) * g.step), back(Math.ceil(n / g.step) * g.step)];
  const marks = TL.show ? TL.show.markers.map(m => m.at) : [];
  const nearest = list => list.reduce((b, v) => Math.abs(v - u) < Math.abs(b - u) ? v : b, Infinity);
  // Markers win when close (strong and medium).
  const m = nearest(marks);
  if (Math.abs(m - u) <= tol) return m;
  if (TL.snap === 'strong') return Math.max(0, nearest(grid));
  const edges = [];
  if (TL.show) for (const t of TL.show.tracks) for (const e of t.events) if (!skip.has(e.id)) edges.push(e.start, e.start + e.len);
  const best = nearest(grid.concat(edges));
  return Math.abs(best - u) <= tol ? best : Math.max(0, u);
}

// --- show mirror, server sync, undo ---
function tlMsg(text, err = false) { $('tlEdMsg').textContent = text; $('tlEdMsg').classList.toggle('err', err); }
function tlEvents() {
  const out = [];
  if (TL.show) TL.show.tracks.forEach((t, ti) => t.events.forEach(e => out.push({ t, ti, e })));
  return out;
}
function tlNextId() { return tlEvents().reduce((m, x) => Math.max(m, x.e.id), 0) + 1; }
function tlPushUndo(snapshot) {
  TL.undo.push(snapshot);
  if (TL.undo.length > TL_UNDO) TL.undo.shift();
  TL.redo = [];
}
// One undoable change: snapshot, apply, send. False when nothing went
// out (no change, or a locked track touched: undone here, T-445).
function tlChange(fn) {
  if (!TL.show) return false;
  const before = JSON.stringify(TL.show);
  fn();
  if (JSON.stringify(TL.show) === before) return false;
  if (tlLockBroken(JSON.parse(before), TL.show)) {
    TL.show = JSON.parse(before);
    TL.headsKey = assignTlScanKey(assignTlNoteKey(assignTlRefKey(''))); TL.need = true;
    tlMsg('Piste verrouillée', true);
    return false;
  }
  tlPushUndo(before);
  tlCommit();
  return true;
}
// --- Verrou de piste (T-445) ---
const tlLockedTi = ti => !!(TL.show && TL.show.tracks[ti] && TL.show.tracks[ti].locked);
// The server's rule (Show::check_locks): a track locked before and after
// keeps its events, name, layer and kind, at its place; unlocking is free
// (the track keeps its name, else it is another one that moved up).
function tlLockBroken(before, after) {
  return before.tracks.some((o, i) => {
    if (!o.locked) return false;
    const t = after.tracks[i];
    if (!t) return true;
    if (!t.locked) return t.name !== o.name;
    return t.name !== o.name || t.layer !== o.layer || t.kind !== o.kind || JSON.stringify(t.events) !== JSON.stringify(o.events);
  });
}
const tlIgnoredMsg = n => n === 1 ? '1 événement verrouillé ignoré' : `${n} événements verrouillés ignorés`;
function tlLockToggle(ti) {
  const t = TL.show && TL.show.tracks[ti];
  if (!t) { tlMsg('Sélectionnez un événement ou placez le pointeur sur une piste.', true); return; }
  tlChange(() => { t.locked = !t.locked; });
  tlMsg(t.locked ? `Piste « ${t.name} » verrouillée : ses événements ne bougent plus.` : `Piste « ${t.name} » déverrouillée.`);
}
// Alt+L: the track of the selected event, else the one under the pointer.
function tlLockKey() {
  const sel = tlSelected();
  tlLockToggle(sel.length ? sel[0].ti : TL.pointerTi);
}
// Tout verrouiller / Tout déverrouiller: one change, one undo step.
function tlLockAll() {
  if (!TL.show || !TL.show.tracks.length) return;
  const all = TL.show.tracks.every(t => t.locked);
  tlChange(() => { for (const t of TL.show.tracks) t.locked = !all; });
  tlMsg(all ? 'Toutes les pistes sont déverrouillées.' : 'Toutes les pistes sont verrouillées.');
}
async function tlCommit() {
  TL.need = true;
  tlHeads();
  const body = JSON.stringify({ show: TL.show });
  TL.inflight++;
  try {
    const r = await api('POST', '/api/timeline/edit', body, { quiet: true });
    if (!r.ok) {
      tlMsg('Modification refusée : ' + await r.text(), true);
      TL.undo.pop();
      TL.rev = -1; // fetch the server's show again
      return;
    }
    const j = await r.json();
    TL.rev = j.state.rev;
    // The server's copy (ids, sorting) unless the page changed meanwhile.
    if (TL.inflight === 1 && !TL.drag && JSON.stringify(TL.show) === body) TL.show = j.show;
    tlShowFlags(j.state);
    // Envelopes on a parameter no envelope may drive: kept, ignored at playback (T-163).
    if (j.warnings && j.warnings.length) tlMsg(j.warnings.join(' · '), true);
  } catch { tlMsg('Studio injoignable : modification non envoyée', true); TL.rev = -1; }
  finally { TL.inflight--; TL.need = true; }
}
function tlUndo() {
  if (!TL.show || !TL.undo.length || TL.drag) return;
  TL.redo.push(JSON.stringify(TL.show));
  TL.show = JSON.parse(TL.undo.pop());
  TL.sel.clear(); TL.marker = -1;
  tlCommit();
}
function tlRedo() {
  if (!TL.show || !TL.redo.length || TL.drag) return;
  TL.undo.push(JSON.stringify(TL.show));
  TL.show = JSON.parse(TL.redo.pop());
  TL.sel.clear(); TL.marker = -1;
  tlCommit();
}
async function tlFetch() {
  TL.fetching = true;
  try {
    const j = await apiJson('GET', '/api/timeline');
    if (TL.inflight || TL.drag) return; // an edit went out meanwhile: next frame
    const show = j.show, name = show ? show.name : null;
    if (name !== TL.name) {
      TL.undo = []; TL.redo = []; TL.sel.clear(); TL.marker = -1;
      TL.name = name; TL.show = show; TL.state = j.state;
      TL.fitPending = true; // once the canvas has its width
      tlEnvAuto();
    } else {
      TL.show = show;
    }
    TL.rev = j.state.rev;
    tlShowFlags(j.state);
    tlHeads(); tlTempoFields();
    TL.need = true;
  } catch { /* studio unreachable: tried again next frame */ } finally { TL.fetching = false; }
}
function tlShowFlags(st) {
  const example = !!(st && st.name && st.example);
  $('tlModified').textContent = example ? `◆ exemple, non enregistré${st.modified ? ' (modifié)' : ''} : Enregistrer en fait une copie à votre nom`
    : st && st.name && st.modified ? '● modifié, non enregistré' : '';
  $('tlModified').classList.toggle('example', example);
}
// « Tempo conseillé » of a Temps show that notes one (the examples): one
// click sets the live tempo, which the show follows.
function tlTempoHint() {
  const tm = TL.show && TL.show.time_base === 'beats' && TL.show.tempo_map && TL.show.tempo_map[0];
  const b = $('tlTempoHint');
  b.hidden = !tm || Math.abs((TL.bpm || 120) - tm.bpm) < 0.05;
  if (!tm) return;
  const text = `Tempo conseillé : ${+tm.bpm.toFixed(1)} BPM`;
  if (b.textContent !== text) { b.textContent = text; b.title = `Régler le tempo en direct à ${+tm.bpm.toFixed(1)} BPM (ce show suit le tempo en direct)`; }
}
$('tlTempoHint').onclick = () => {
  const tm = TL.show && TL.show.tempo_map && TL.show.tempo_map[0];
  if (tm) post('/api/control', { id: 'tempo.bpm', value: tm.bpm });
};
// Every state (applyState, `at` when it was read): playhead, and the show if it changed elsewhere.
function tlOnState(t, tempo, at = performance.now()) {
  if (!t) return;
  TL.state = t; TL.stateAt = at;
  if (tempo && tempo.bpm) TL.bpm = tempo.bpm;
  if ((t.name !== TL.name || t.rev !== TL.rev) && !TL.inflight && !TL.drag && !TL.fetching) tlFetch();
  if (!TL.inflight) tlShowFlags(t);
  tlTempoHint();
  const has = !!TL.show;
  for (const id of ['tlSave', 'tlRecord', 'tlAddMarker', 'tlCsvBtn', 'tlCopy', 'tlPaste', 'tlDup', 'tlDel', 'tlAddTrack', 'tlFit']) $(id).disabled = !has;
  $('tlLockAll').disabled = !has || !TL.show.tracks.length;
  $('tlUndo').disabled = !has || !TL.undo.length;
  $('tlRedo').disabled = !has || !TL.redo.length;
  $('tlPaste').disabled = !has || !TL.clip;
  if (presets !== TL.libFor && activeWs === 'timeline') tlLib();
  if (t.playing) TL.need = true;
}
// Where the playhead is now (extrapolated between two frames while playing).
function tlPlayhead() {
  const t = TL.state;
  if (!t || !t.name) return 0;
  if (!t.playing || t.waiting) return t.position;
  const dt = Math.min(0.2, (performance.now() - TL.stateAt) / 1000);
  return t.position + dt * (t.time_base === 'beats' ? (TL.bpm || 120) / 60 : 1);
}

// --- library (cue pages, figures, evolving cues) ---
function tlLib() {
  TL.libFor = presets;
  const cat = $('tlLibCat'), cur = cat.value;
  cat.innerHTML = '<option value="">Tous les cues</option>' + cueCategories.map(c => `<option>${esc(c)}</option>`).join('');
  cat.value = cueCategories.includes(cur) ? cur : '';
  const f = $('tlLibFilter').value.trim().toLowerCase();
  const list = presets.filter(p => (!cat.value || p.category === cat.value) && (!f || p.name.toLowerCase().includes(f)));
  $('tlLibList').innerHTML = list.map(p => `<div class="tl-lib-item" draggable="true" data-id="${esc(p.id)}" title="${esc(p.category)} · glisser sur une piste">${esc(p.name)}${p.beats ? `<span class="d">évolutif · ${+p.beats} t</span>` : ''}</div>`).join('')
    || '<div class="hint">Aucun cue</div>';
}
$('tlLibCat').addEventListener('change', tlLib);
$('tlLibFilter').addEventListener('input', tlLib);
$('tlLibList').addEventListener('dragstart', e => {
  const item = e.target.closest('.tl-lib-item');
  if (!item) return;
  TL.dragCue = item.dataset.id;
  e.dataTransfer.setData('text/plain', item.dataset.id);
  e.dataTransfer.effectAllowed = 'copy';
});
$('tlLibList').addEventListener('dragend', () => { TL.dragCue = null; TL.drop = null; TL.need = true; });
const tlPresetName = id => (presets.find(p => p.id === id) || {}).name || id;
// A look stored in the show (the examples' tweaked cues, countdown digits).
function tlEventName(e) {
  if (e.source.kind === 'cue') return tlPresetName(e.source.id);
  if (e.source.kind === 'dmx') return `DMX · ${e.source.channel} = ${e.source.value}`;
  const c = e.source.settings && e.source.settings.content || {};
  if (c.kind === 'text') return `Texte « ${c.text} »`;
  if (c.kind === 'generator') return 'Look · ' + (GEN_NAMES[c.generator] || c.generator);
  return 'Look';
}
// Default length: an evolving cue's own length, else 4 bars.
function tlDefaultLen(id, start) {
  const p = presets.find(q => q.id === id);
  if (p && p.beats) return Math.max(TL_MIN_LEN, tlUAtBeat(tlBeatAt(start) + p.beats) - start);
  return Math.max(TL_MIN_LEN, tlUAtBar(tlBarAt(start) + 4) - start);
}

// --- envelopes (T-163): a curve on a control per event, in a lane under the track ---
// Keys are { at (show units from the event start), value, ease }; the studio
// evaluates them (program::curve_at) on the frame's copies, before the
// master live stage and the safety chain. Here they are only drawn and edited.
const TL_MAX_ENVS = 16, TL_MAX_ENV_KEYS = 64;
const TL_EASES = [['linear', 'Linéaire'], ['smooth', 'Douce'], ['ease_in', 'Accélère'], ['ease_out', 'Ralentit'], ['step', 'Palier']];
const tlEaseFr = e => (TL_EASES.find(x => x[0] === e) || TL_EASES[0])[1];
// The cue programs' easing (evolving.rs, KeyEase::apply), for drawing.
function tlEase(ease, u) {
  u = Math.max(0, Math.min(1, u));
  switch (ease) {
    case 'step': return 0;
    case 'ease_in': return u * u;
    case 'ease_out': return 1 - (1 - u) * (1 - u);
    case 'smooth': return u * u * (3 - 2 * u);
    default: return u;
  }
}
function tlCurveAt(keys, x) {
  const k = keys.filter(q => Number.isFinite(q.at) && Number.isFinite(q.value)).sort((a, b) => a.at - b.at);
  if (!k.length) return null;
  if (x <= k[0].at) return k[0].value;
  if (x >= k[k.length - 1].at) return k[k.length - 1].value;
  let i = 0;
  while (i + 1 < k.length && k[i + 1].at <= x) i++;
  const a = k[i], b = k[i + 1];
  return a.value + (b.value - a.value) * tlEase(a.ease || 'linear', (x - a.at) / (b.at - a.at));
}
// What an envelope may drive: the controls LFOs may move (never arm, safety, calibration, zones, outputs).
async function tlLoadEnvTargets() {
  if (TL.envTargets || TL.envLoading) return;
  TL.envLoading = true;
  try { TL.envTargets = (await apiJson('GET', '/api/lfos')).targets || []; TL.headsKey = ''; TL.need = true; } catch { /* tried again later */ } finally { TL.envLoading = false; }
}
const tlEnvTarget = id => (TL.envTargets || []).find(t => t.id === id);
const tlEnvLabel = id => { const t = tlEnvTarget(id); return t ? t.label : `${id} (inconnu, ignoré)`; };
// Value range of a lane: the control's (Absolu), or a factor ×0 to ×2 (Relatif).
function tlEnvRange(id, mode) {
  if (mode === 'relative') return [0, 2];
  const t = tlEnvTarget(id);
  return t ? [t.min, t.max] : [0, 1];
}
const tlEnvFind = (e, id) => (e.envelopes || []).find(v => v.target === id) || null;
// The mode of a parameter on a track: its envelopes', else the one chosen for new ones.
function tlEnvModeOf(ti, id) {
  for (const e of TL.show.tracks[ti].events) { const v = tlEnvFind(e, id); if (v) return v.mode || 'absolute'; }
  return TL.envMode[ti] || 'absolute';
}
function tlEnvGeom(ti) {
  const id = tlEnvOf(ti), mode = tlEnvModeOf(ti, id), [min, max] = tlEnvRange(id, mode);
  const top = tlLaneY(ti) + TL_LANE, y0 = top + TL_ENV_PAD, y1 = top + TL_ENV - TL_ENV_PAD, span = max - min || 1;
  return {
    id, mode, min, max, y0, y1,
    vy: v => y1 - (Math.max(min, Math.min(max, v)) - min) / span * (y1 - y0),
    yv: y => min + (y1 - Math.max(y0, Math.min(y1, y))) / (y1 - y0) * span,
  };
}
const tlFmtV = v => v == null ? '' : Math.abs(v) >= 100 ? v.toFixed(0) : Math.abs(v) >= 10 ? v.toFixed(1) : v.toFixed(2);
// The node of the shown envelope under (x, y) on track ti: { e, env, k }.
function tlEnvKeyAt(ti, x, y) {
  const g = tlEnvGeom(ti);
  let best = null, dist = 7;
  for (const e of TL.show.tracks[ti].events) {
    const env = tlEnvFind(e, g.id);
    if (!env) continue;
    for (const k of env.keys) {
      const d = Math.hypot(tlUToX(e.start + k.at) - x, g.vy(k.value) - y);
      if (d < dist) { dist = d; best = { e, env, k }; }
    }
  }
  return best;
}
function tlEnvEventAt(ti, x) {
  const t = TL.show.tracks[ti];
  for (let i = t.events.length - 1; i >= 0; i--) {
    const e = t.events[i];
    if (x >= tlUToX(e.start) && x <= tlUToX(e.start + e.len)) return e;
  }
  return null;
}
// Double-click in the lane: a node there (the envelope is made if needed).
function tlEnvAddKey(ti, x, y) {
  const e = tlEnvEventAt(ti, x);
  if (!e) { tlMsg('Enveloppe : double-cliquez au-dessus d\'un événement de la piste.', true); return; }
  const g = tlEnvGeom(ti);
  let full = false;
  tlChange(() => {
    e.envelopes = e.envelopes || [];
    let env = tlEnvFind(e, g.id);
    if (!env) {
      if (e.envelopes.length >= TL_MAX_ENVS) { full = true; return; }
      env = { target: g.id, mode: g.mode, keys: [] };
      e.envelopes.push(env);
    }
    if (env.keys.length >= TL_MAX_ENV_KEYS) { full = true; return; }
    const at = Math.max(0, Math.min(e.len, tlSnapU(tlXToU(x)) - e.start));
    env.keys.push({ at, value: +g.yv(y).toFixed(4), ease: 'linear' });
    env.keys.sort((a, b) => a.at - b.at);
  });
  if (full) tlMsg(`Enveloppe : ${TL_MAX_ENVS} enveloppes par événement et ${TL_MAX_ENV_KEYS} nœuds par enveloppe au plus.`, true);
}
function tlEnvDelKey(hit) {
  tlChange(() => {
    hit.env.keys.splice(hit.env.keys.indexOf(hit.k), 1);
    if (!hit.env.keys.length) hit.e.envelopes.splice(hit.e.envelopes.indexOf(hit.env), 1);
  });
}
function tlEnvCycleEase(hit) {
  const ok = tlChange(() => {
    const i = TL_EASES.findIndex(x => x[0] === (hit.k.ease || 'linear'));
    hit.k.ease = TL_EASES[(i + 1) % TL_EASES.length][0];
  });
  if (ok) tlMsg(`Courbe du nœud : ${tlEaseFr(hit.k.ease)}`);
}
// Show or hide the lane of a track (`id` null: hidden; the envelopes stay).
function tlEnvOpen(ti, id) {
  if (id) TL.env[ti] = id; else delete TL.env[ti];
  TL.headsKey = ''; TL.need = true;
  if (id) tlMsg('Enveloppe : double-clic ajoute un nœud (sur un nœud : change sa courbe), glisser le déplace, clic droit le supprime.');
}
// The parameter a lane opens on: the track's first envelope, else the dimmer.
function tlEnvDefault(ti) {
  for (const e of TL.show.tracks[ti].events) if (e.envelopes && e.envelopes.length) return e.envelopes[0].target;
  return 'master.brightness';
}
// A show just loaded: lanes open on bus tracks and on tracks with envelopes.
function tlEnvAuto() {
  TL.env = {}; TL.envMode = {}; TL.hover = null;
  if (TL.show) TL.show.tracks.forEach((t, i) => {
    if (t.kind === 'bus' || t.events.some(e => e.envelopes && e.envelopes.length)) TL.env[i] = tlEnvDefault(i);
  });
}
// A track deleted: the lanes below it move up.
function tlEnvShift(removed) {
  const shift = m => Object.fromEntries(Object.entries(m).filter(([k]) => +k !== removed).map(([k, v]) => [+k > removed ? +k - 1 : +k, v]));
  TL.env = shift(TL.env); TL.envMode = shift(TL.envMode);
}
// Double-click on a bus track: a bus event of 4 bars (it plays nothing; its envelopes act on the whole layer).
// A DMX event (T-453): the first named channel of the DMX output at 255,
// 4 bars long; its properties row sets the rest.
function tlDmxAdd(ti, x) {
  if (TL.show.tracks[ti].locked) { tlMsg('Piste verrouillée', true); return; }
  const start = tlSnapU(tlXToU(x)), names = dmxOutNames();
  tlChange(() => {
    const ev = { id: tlNextId(), start, len: Math.max(TL_MIN_LEN, tlUAtBar(tlBarAt(start) + 4) - start), source: { kind: 'dmx', channel: names[0] || 'Brouillard', value: 255 },
      fade_in: { seconds: 0 }, fade_out: { seconds: 0 } };
    TL.show.tracks[ti].events.push(ev);
    TL.sel = new Set([ev.id]);
  });
  if (!names.length) tlMsg('Aucun canal nommé : déclarez-les dans RÉGLAGES › Sorties et projecteurs › Sortie DMX.', true);
}
function tlBusAdd(ti, x) {
  const start = tlSnapU(tlXToU(x));
  tlChange(() => {
    const ev = { id: tlNextId(), start, len: Math.max(TL_MIN_LEN, tlUAtBar(tlBarAt(start) + 4) - start), source: { kind: 'cue', id: '' }, envelopes: [] };
    TL.show.tracks[ti].events.push(ev);
    TL.sel = new Set([ev.id]);
  });
}
function tlEnvOptions(cur) {
  const list = TL.envTargets || [];
  const groups = [...new Set(list.map(t => t.group || ''))];
  let html = list.some(t => t.id === cur) ? '' : `<option value="${esc(cur)}" selected>${esc(tlEnvLabel(cur))}</option>`;
  for (const gr of groups) {
    html += `<optgroup label="${esc(gr || 'Autres')}">` + list.filter(t => (t.group || '') === gr).map(t => `<option value="${esc(t.id)}"${t.id === cur ? ' selected' : ''}>${esc(t.label)}</option>`).join('') + '</optgroup>';
  }
  return html;
}

// --- track heads (DOM, aligned with the canvas lanes) ---
function tlHeads() {
  const tracks = TL.show ? TL.show.tracks : [];
  if (TL.show && !TL.envTargets) tlLoadEnvTargets();
  const key = TL.show ? JSON.stringify([tracks.map((t, i) => [t.name, t.layer, t.mute, t.solo, t.kind, !!t.locked, TL.env[i] || '', TL.env[i] ? tlEnvModeOf(i, TL.env[i]) : '']), !!TL.envTargets]) : 'empty';
  const all = !!tracks.length && tracks.every(t => t.locked);
  $('tlLockAll').textContent = all ? 'Tout déverrouiller' : 'Tout verrouiller';
  const c = tlCanvas();
  const h = TL.show ? (tracks.length ? tlLaneY(tracks.length) : TL_TOP + TL_LANE) : TL_TOP + TL_EMPTY_TRACKS * TL_LANE;
  if (c.style.height !== h + 'px') { c.style.height = h + 'px'; TL.need = true; }
  $('tlEmpty').hidden = !!TL.show;
  if (key === TL.headsKey && $('tlHeads').childElementCount) return;
  if ($('tlHeads').contains(document.activeElement)) return; // renaming: redrawn afterwards
  TL.headsKey = key;
  if (!TL.show) {
    // Empty state: what a show will look like, four empty tracks.
    $('tlHeads').innerHTML = `<div class="top" style="height:${TL_TOP}px">Pistes</div>` + Array.from({ length: TL_EMPTY_TRACKS }, (_, i) =>
      `<div class="tl-head empty" style="height:${TL_LANE}px;border-left-color:${TL_LAYER_COLORS[i % 4]}">Piste ${i + 1}</div>`).join('');
    return;
  }
  $('tlHeads').innerHTML = `<div class="top" style="height:${TL_TOP}px">Pistes</div>` + tracks.map((t, i) => `
    <div class="tl-head${t.locked ? ' locked' : ''}" data-ti="${i}" style="height:${TL_LANE}px;border-left-color:${TL_LAYER_COLORS[(t.layer - 1) % 4]}">
      <input type="text" value="${esc(t.name)}" maxlength="64" data-f="name" title="Nom de la piste"${t.locked ? ' disabled' : ''}>
      <select data-f="layer" title="Calque"${t.locked ? ' disabled' : ''}>${[1, 2, 3, 4].map(n => `<option value="${n}"${n === t.layer ? ' selected' : ''}>C${n}</option>`).join('')}</select>
      <select data-f="kind" title="Cues : la piste joue des cues · Bus : ses enveloppes agissent sur toutes les pistes de son calque · DMX : ses événements règlent la sortie DMX"${t.locked ? ' disabled' : ''}>
        <option value="cues"${t.kind === 'cues' ? ' selected' : ''}>Cues</option><option value="bus"${t.kind === 'bus' ? ' selected' : ''}>Bus</option><option value="dmx"${t.kind === 'dmx' ? ' selected' : ''}>DMX</option></select>
      <button data-f="mute" class="${t.mute ? 'active' : ''}" title="Muet">M</button>
      <button data-f="solo" class="${t.solo ? 'active' : ''}" title="Solo">S</button>
      <button data-f="lock" class="${t.locked ? 'active' : ''}" title="${t.locked ? 'Déverrouiller la piste' : 'Verrouiller la piste'} (Alt+L)" aria-pressed="${!!t.locked}">V</button>
      ${t.kind === 'dmx' ? '' : `<button data-f="env" class="${TL.env[i] ? 'active' : ''}" title="+ Enveloppe : une ligne sous la piste pour dessiner un paramètre (taille, rotation, luminosité…)">∿</button>`}
      <button data-f="del" title="Supprimer la piste (Annuler la rétablit)">✕</button>
    </div>${TL.env[i] ? `
    <div class="tl-head tl-envhead" data-ti="${i}" style="height:${TL_ENV}px;border-left-color:${TL_LAYER_COLORS[(t.layer - 1) % 4]}">
      <span class="lbl">Env.</span>
      <select data-f="envt" title="Paramètre de l'enveloppe (ceux qu'un LFO peut moduler)">${tlEnvOptions(TL.env[i])}</select>
      <select data-f="envm" title="Absolu : la courbe remplace la valeur · Relatif : elle la multiplie (×0 à ×2). Une luminosité ne fait que baisser."${t.locked ? ' disabled' : ''}>
        <option value="absolute"${tlEnvModeOf(i, TL.env[i]) === 'absolute' ? ' selected' : ''}>Absolu</option><option value="relative"${tlEnvModeOf(i, TL.env[i]) === 'relative' ? ' selected' : ''}>Relatif</option></select>
      <button data-f="envx" title="Masquer la ligne (les enveloppes restent)">✕</button>
    </div>` : ''}`).join('');
}
$('tlHeads').addEventListener('change', e => {
  const head = e.target.closest('.tl-head');
  if (!head || !TL.show) return;
  const ti = +head.dataset.ti, t = TL.show.tracks[ti], f = e.target.dataset.f, v = e.target.value;
  if (f === 'name') tlChange(() => { t.name = v.slice(0, 64); });
  if (f === 'layer') tlChange(() => { t.layer = +v; });
  if (f === 'kind') {
    // Cue events and DMX events don't mix: only an empty track changes to or from DMX.
    if ((v === 'dmx' || t.kind === 'dmx') && t.events.length) {
      tlMsg('Videz la piste avant de la changer en piste DMX (ou l\'inverse)', true);
      e.target.value = t.kind; e.target.blur(); return;
    }
    tlChange(() => { t.kind = v; });
    if (v === 'dmx') { tlEnvOpen(ti, null); tlMsg('Piste DMX : double-cliquez sur la piste pour poser un événement, puis choisissez son canal et sa valeur.'); dmxOutChannels(); }
    if (v === 'bus' && !TL.env[ti]) tlEnvOpen(ti, tlEnvDefault(ti));
    if (v === 'bus') tlMsg('Piste bus : double-cliquez sur la piste pour y poser un événement, puis dessinez ses enveloppes : elles agissent sur toutes les pistes du calque.');
  }
  if (f === 'envt') tlEnvOpen(ti, v);
  if (f === 'envm') {
    TL.envMode[ti] = v;
    tlChange(() => { for (const ev of t.events) { const env = tlEnvFind(ev, TL.env[ti]); if (env) env.mode = v; } });
    TL.headsKey = ''; TL.need = true;
  }
  e.target.blur();
});
$('tlHeads').addEventListener('click', e => {
  const b = e.target.closest('button'), head = e.target.closest('.tl-head');
  if (!b || !head || !TL.show) return;
  const i = +head.dataset.ti, t = TL.show.tracks[i];
  if (b.dataset.f === 'mute') tlChange(() => { t.mute = !t.mute; });
  if (b.dataset.f === 'solo') tlChange(() => { t.solo = !t.solo; });
  if (b.dataset.f === 'lock') { b.blur(); tlLockToggle(i); }
  if (b.dataset.f === 'del') {
    if (t.locked) { tlMsg('Piste verrouillée', true); return; }
    if (tlChange(() => { TL.show.tracks.splice(i, 1); TL.sel.clear(); })) tlEnvShift(i);
    TL.headsKey = '';
  }
  // The heads are drawn again only when none has the focus.
  if (b.dataset.f === 'env') { b.blur(); tlEnvOpen(i, TL.env[i] ? null : tlEnvDefault(i)); }
  if (b.dataset.f === 'envx') { b.blur(); tlEnvOpen(i, null); }
});
$('tlLockAll').onclick = tlLockAll;
function tlTrackMenu(open) { $('tlTrackMenu').hidden = !open; $('tlAddTrack').setAttribute('aria-expanded', open); }
$('tlAddTrack').onclick = e => { e.stopPropagation(); tlTrackMenu($('tlTrackMenu').hidden); };
document.addEventListener('click', e => { if (!e.target.closest('#tlTrackMenu') && e.target !== $('tlAddTrack')) tlTrackMenu(false); });
function tlAddTrackOf(kind) {
  tlTrackMenu(false);
  if (!TL.show) return;
  tlChange(() => {
    const n = TL.show.tracks.length;
    const name = kind === 'dmx' ? `DMX ${TL.show.tracks.filter(t => t.kind === 'dmx').length + 1}` : `Piste ${n + 1}`;
    TL.show.tracks.push({ name, kind, layer: (n % 4) + 1, mute: false, solo: false, events: [] });
  });
  if (kind === 'dmx') { tlMsg('Piste DMX : double-cliquez sur la piste pour poser un événement, puis choisissez son canal et sa valeur.'); dmxOutChannels(); }
}
$('tlAddCues').onclick = () => tlAddTrackOf('cues');
$('tlAddDmx').onclick = () => tlAddTrackOf('dmx');

// --- toolbar ---
function tlSetSnap(v) {
  TL.snap = ['strong', 'medium', 'off'].includes(v) ? v : 'strong';
  remember('laserStudio.tlSnap', TL.snap);
  document.querySelectorAll('#tlSnap button').forEach(b => b.classList.toggle('active', b.dataset.snap === TL.snap));
}
$('tlSnap').addEventListener('click', e => { if (e.target.dataset.snap) tlSetSnap(e.target.dataset.snap); });
tlSetSnap(TL.snap);
$('tlFit').onclick = tlFit;
$('tlFollow').addEventListener('change', () => { TL.follow = $('tlFollow').checked; });
$('tlNew').onclick = () => { $('tlNewRow').hidden = !$('tlNewRow').hidden; if (!$('tlNewRow').hidden) $('tlNewName').focus(); };
// Empty state: the name row opens with a free name, ready for Entrée.
$('tlEmptyNew').onclick = async () => {
  await loadShows();
  $('tlNewRow').hidden = false;
  if (!$('tlNewName').value.trim()) $('tlNewName').value = tlFreeName('Nouveau show');
  $('tlNewName').focus(); $('tlNewName').select();
  tlMsg('Nommez le show, puis Créer (ou Entrée).');
};

// Before a show replaces the one in the editor (another show, a new one,
// an example, a proposed show): its unsaved changes (T-162) would be lost.
// One question for all (T-433); true = go on.
function dropShowChanges(why = 'les abandonner') {
  const st = lastTimeline || (TL.state && { name: TL.name, modified: TL.state.modified });
  if (!st || !st.name || !st.modified) return true;
  return confirm(`« ${st.name} » a des modifications non enregistrées : ${why} ?`);
}

// --- built-in examples (T-124 to T-129): read-only, opened as a copy ---
let tlExamples = null;
async function openTlExamples() {
  $('tlExDialog').hidden = false;
  if (!tlExamples) {
    try { tlExamples = await apiJson('GET', '/api/timeline/examples'); } catch { tlExamples = null; }
  }
  $('tlExList').innerHTML = (tlExamples || []).map(x => `<button class="tl-ex" data-ex="${esc(x.id)}">
      <b>${esc(x.title)}</b><span class="meta">${+x.bpm} BPM conseillé · ${x.bars} mesures</span><span class="desc">${esc(x.description)}</span></button>`).join('')
    || '<div class="hint">Exemples indisponibles.</div>';
  const first = $('tlExList').querySelector('button');
  if (first) first.focus();
}
function closeTlExamples() { $('tlExDialog').hidden = true; }
$('tlExamples').onclick = openTlExamples;
$('tlEmptyEx').onclick = openTlExamples;
$('tlExClose').onclick = closeTlExamples;
$('tlExDialog').addEventListener('click', e => { if (e.target === $('tlExDialog')) closeTlExamples(); });
$('tlExList').addEventListener('click', async e => {
  const b = e.target.closest('[data-ex]');
  if (!b) return;
  const x = (tlExamples || []).find(q => q.id === b.dataset.ex);
  if (!dropShowChanges()) return;
  const r = await api('POST', '/api/timeline/load', { example: b.dataset.ex }, { quiet: true });
  if (!r.ok) { tlMsg(await r.text(), true); return; }
  closeTlExamples();
  $('tlNewRow').hidden = true;
  TL.rev = -1;
  tlMsg(`Exemple « ${x ? x.title : b.dataset.ex} » ouvert : une copie, rien n'est enregistré. ▶ Lecture démarre à la mesure suivante${x ? ` (tempo conseillé ${+x.bpm} BPM)` : ''}.`);
});
async function tlNewShow() {
  const name = $('tlNewName').value.trim();
  if (!name) { tlMsg('Donnez un nom au show.', true); return; }
  if (!dropShowChanges()) return;
  const r = await api('POST', '/api/timeline/new', { name, time_base: $('tlNewBase').value }, { quiet: true });
  if (!r.ok) { tlMsg(await r.text(), true); return; }
  $('tlNewRow').hidden = true; $('tlNewName').value = '';
  tlMsg(`Show « ${name} » créé : glissez des cues de la bibliothèque sur les pistes.`);
  TL.rev = -1;
  await loadShows();
}
$('tlNewOk').onclick = tlNewShow;
$('tlNewName').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); tlNewShow(); } });
// A name nobody uses yet: `base`, else `base 2`, `base 3`…
function tlFreeName(base) {
  const taken = new Set(shows.map(s => s.name));
  if (!taken.has(base)) return base;
  let n = 2;
  while (taken.has(`${base} ${n}`)) n++;
  return `${base} ${n}`;
}
$('tlSave').onclick = async () => {
  let body = '{}';
  if (TL.state && TL.state.example) {
    // An example's copy is saved under a name of its own, never over a show.
    await loadShows();
    const name = prompt('Enregistrer une copie de l\'exemple sous le nom :', tlFreeName(TL.name || 'Mon show'));
    if (name === null) return;
    body = JSON.stringify({ name: name.trim() });
  }
  const r = await api('POST', '/api/timeline/save', body, { quiet: true });
  if (!r.ok) { tlMsg('Enregistrement impossible : ' + await r.text(), true); return; }
  const st = await r.json();
  TL.rev = st.rev; tlShowFlags(st);
  tlMsg(`Show « ${st.name} » enregistré.`);
  loadShows();
};
function tlTempoFields() {
  const sec = tlSeconds() && !!TL.show, tm = TL.show && TL.show.tempo_map && TL.show.tempo_map[0];
  $('tlBpm').disabled = $('tlBpb').disabled = !sec;
  $('tlBpm').title = sec ? 'Tempo du show (secondes) : règle, magnétisme et copier-coller en mesures' : 'Show en temps : suit le tempo en direct';
  if (document.activeElement !== $('tlBpm')) $('tlBpm').value = sec ? (tm ? tm.bpm : 120) : '';
  if (document.activeElement !== $('tlBpb')) $('tlBpb').value = sec ? (tm ? tm.beats_per_bar : 4) : '';
}
function tlSetTempo() {
  const bpm = +$('tlBpm').value, bpb = Math.round(+$('tlBpb').value);
  if (!(bpm >= 20 && bpm <= 300) || !(bpb >= 1 && bpb <= 16)) { tlTempoFields(); return; }
  tlChange(() => {
    if (!TL.show.tempo_map.length) TL.show.tempo_map.push({ at_s: 0, bpm: 120, beats_per_bar: 4 });
    TL.show.tempo_map[0].bpm = bpm; TL.show.tempo_map[0].beats_per_bar = bpb;
  });
}
$('tlBpm').addEventListener('change', tlSetTempo);
$('tlBpb').addEventListener('change', tlSetTempo);

export { TL, TL_DMX_COLOR, TL_DRAG_PX, TL_EASES, TL_EMPTY_TRACKS, TL_ENV, TL_LANE, TL_LAYER_COLORS,
  TL_MARK, TL_MARK_COLORS, TL_MIN_LEN, TL_RULER, TL_TOP, TL_WAVE, closeTlExamples, dropShowChanges,
  tlBarAt, tlBusAdd, tlCanvas, tlChange, tlCommit, tlCurveAt, tlDefaultLen, tlDmxAdd, tlEaseFr,
  tlEnvAddKey, tlEnvCycleEase, tlEnvDelKey, tlEnvEventAt, tlEnvFind, tlEnvGeom, tlEnvKeyAt,
  tlEnvLabel, tlEnvOf, tlEventName, tlEvents, tlFit, tlFmtV, tlGrid, tlGridLines, tlHeads,
  tlIgnoredMsg, tlLaneH, tlLaneY, tlLockAll, tlLockBroken, tlLockKey, tlLockedTi, tlMsg, tlNextId,
  tlOnState, tlPlayhead, tlPushUndo, tlRedo, tlSecAt, tlSeconds, tlSetView, tlSnapU, tlUAtBar,
  tlUToX, tlUndo, tlW, tlXToU };
