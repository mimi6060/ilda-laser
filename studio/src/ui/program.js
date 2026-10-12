// Laser Studio — program.js (T-435).
// Sections (as in the former index.html script):
//   CRÉATION › Cue évolutif: the cue program editor (T-164)
//   the curves canvas
//   live preview: pictures of the program at the tempo, never the laser
//   controls
//   CRÉATION › Abstraits: stacked oscillators (T-440)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './media.js';
import { $, api, hex, rgb, setCheck, setVal, settings } from './core.js';
import { LOCKED_FR, activeWs, remember, remembered, setWorkspace, showMode } from './workspaces.js';
import { drawThumb, loadPresets, loadSlots, presetById, presets } from './cues.js';
import { menuCue } from './presets.js';
import { TL_EASES, tlCurveAt, tlFmtV } from './timeline-editor.js';
import { GEN_NAMES } from './layers.js';
import { LFO_PERIODS, LFO_WAVES, lfoEsc } from './modulators.js';
import { CTAB_KEY, setCreationTab, setFigTool, showFigProps } from './figures.js';

// ---------- CRÉATION › Cue évolutif: the cue program editor (T-164) ----------
// Edits a cue's program (program.rs, T-157): length, play mode, inner
// layers, curves (lanes of keys, drawn with the timeline envelopes' easing
// and curve code, T-163) and the cue's own LFOs. The live preview is a
// series of pictures from POST /api/cues/program/preview, played here at
// the tempo: display only, never sent to the laser. « Enregistrer » sets
// the program on the cue's cell (it plays at the cue's next launch);
// « Enregistrer dans une case » makes a new user cue of it (T-405). The
// server refuses both in show mode (423) and checks every program.
const PG_LENGTHS = [4, 8, 16, 32, 64];
const PG_TARGET_FR = {
  brightness: 'Luminosité', size: 'Taille', size_x: 'Taille X', size_y: 'Taille Y', pos_x: 'Position X', pos_y: 'Position Y',
  'rot_x.angle': 'Rotation X (angle)', 'rot_y.angle': 'Rotation Y (angle)', 'rot_z.angle': 'Rotation Z (angle)',
  'rot_x.speed': 'Rotation X (vitesse °/s)', 'rot_y.speed': 'Rotation Y (vitesse °/s)', 'rot_z.speed': 'Rotation Z (vitesse °/s)',
  perspective: 'Perspective', 'color.hue': 'Teinte', 'strobe.div': 'Strobe (éclairs par temps)',
};
// Canvas layout: the ruler, then one lane per curve.
const PG_RULER = 24, PG_LANE = 72, PG_PAD = 10, PG_KEY_R = 5;
const PG_CUE_KEY = 'laserStudio.programCue';
let pgTempo = null;
function assignPgTempo(v) { return (pgTempo = v); }
const pg = {
  id: null, info: null, prog: null, saved: 'null', sel: null, drag: null, loading: 0,
  pv: null, pvAt: 0, pvBeat: 0, pvTimer: 0, pvDebounce: 0, pvSeq: 0, colors: null,
};
const pgTargetFr = id => PG_TARGET_FR[id] || id;
const pgTarget = id => (pg.info && pg.info.targets.find(t => t.id === id)) || { id, min: 0, max: 1, default: 0 };
const pgLen = () => (pg.prog && pg.prog.length_beats) || 16;
const pgDirty = () => !!pg.prog && JSON.stringify(pg.prog) !== pg.saved;
function pgMsg(text, err = false) {
  $('pgMsg').textContent = text;
  $('pgMsg').classList.toggle('err', err);
}
// A refusal of the studio, in French (its own message for a bad program).
function pgRefusal(status, text, what) {
  if (status === 423) return LOCKED_FR;
  if (status === 404) return 'Ce cue n\'existe plus.';
  if (status === 400 && text && !/^(invalid JSON|unreadable body|expected)/.test(text)) return `${what} : ${text}`;
  return `${what} : le studio n'a pas pu le lire (erreur ${status}).`;
}
async function pgPost(path, body) {
  try {
    const r = await api('POST', path, body, { quiet: true });
    return { ok: r.ok, status: r.status, text: await r.text() };
  } catch { return { ok: false, status: 0, text: '' }; }
}

// The program as the editor holds it: every field present, keys in order
// (so « modifié » is a plain comparison with what was saved).
function pgNormalize(p, info) {
  p = p ? JSON.parse(JSON.stringify(p)) : {};
  return {
    length_beats: p.length_beats ?? info.content_beats,
    play: p.play || 'loop',
    layers: p.layers || [],
    lanes: (p.lanes || []).map(l => ({
      target: l.target,
      keys: (l.keys || []).map(k => ({ beat: k.beat ?? 0, value: k.value ?? 0, ease: k.ease || 'linear' })).sort((a, b) => a.beat - b.beat),
    })),
    lfos: (p.lfos || []).map(m => ({ target: 'size', wave: 'sine', rate: { beats: 4 }, depth: 0.5, phase: 0, offset: 0, enabled: true, ...m })),
  };
}

function pgSync() {
  const on = activeWs === 'creation' && $('workspace').dataset.ctab === 'program';
  $('pgEditor').hidden = !on;
  if (!on) { pgStopPreview(); return; }
  pgFillCues(); pgFillLayerSrc();
  if (pg.id) { pgRender(); pgSchedulePreview(0); return; }
  const first = presetById.get(remembered(PG_CUE_KEY)) || presets.find(p => p.program) || presets[0];
  if (first) pgOpen(first.id);
}
async function pgOpen(id, force = false) {
  if (!force && pg.id && id !== pg.id && pgDirty() && !confirm('Abandonner les modifications du programme en cours ?')) {
    $('pgCue').value = pg.id;
    return;
  }
  const seq = ++pg.loading;
  let info;
  try {
    const r = await api('GET', '/api/cues/program?id=' + encodeURIComponent(id), undefined, { cache: 'no-store', quiet: true });
    if (!r.ok) { if (seq === pg.loading) pgMsg('Ce cue n\'existe plus.', true); return; }
    info = await r.json();
  } catch { return; }
  if (seq !== pg.loading) return;
  pg.id = id; pg.info = info; pg.sel = null; pg.drag = null; pg.pv = null;
  pg.prog = pgNormalize(info.program, info);
  pg.saved = JSON.stringify(pg.prog);
  remember(PG_CUE_KEY, id);
  pgFillCues();
  pgMsg(info.program ? '' : 'Ce cue n\'a pas encore de programme : ajoutez une courbe, une couche ou un modulateur.');
  pgRender();
  pgSchedulePreview(0);
}
// The cues, by category (∿ = has a program).
function pgFillCues() {
  const sel = $('pgCue'), key = presets.map(p => p.id + (p.program ? '∿' : '') + p.name).join('|');
  if (sel.dataset.key !== key) {
    sel.dataset.key = key;
    const groups = new Map();
    for (const p of presets) { if (!groups.has(p.category)) groups.set(p.category, []); groups.get(p.category).push(p); }
    sel.replaceChildren(...[...groups].map(([cat, list]) => {
      const g = document.createElement('optgroup');
      g.label = cat;
      g.append(...list.map(p => new Option((p.program ? '∿ ' : '') + p.name, p.id)));
      return g;
    }));
  }
  sel.value = pg.id || '';
}
// What an inner layer can be: an effect, a shape, the wave, or any cue's content.
function pgFillLayerSrc() {
  const sel = $('pgLayerSrc'), key = presets.length + ':' + $('gen').options.length + ':' + $('shapes').children.length;
  if (sel.dataset.key === key) return;
  sel.dataset.key = key;
  const group = (label, opts) => { const g = document.createElement('optgroup'); g.label = label; g.append(...opts); return g; };
  const cats = new Map();
  for (const p of presets) { if (!cats.has(p.category)) cats.set(p.category, []); cats.get(p.category).push(p); }
  sel.replaceChildren(
    group('Effets', [...$('gen').options].map(o => new Option(o.textContent, 'gen:' + o.value))),
    group('Formes', [...[...$('shapes').querySelectorAll('button[data-shape]')].map(b => new Option(b.textContent, 'shape:' + b.dataset.shape)), new Option('Onde', 'wave')]),
    ...[...cats].map(([c, list]) => group('Cues · ' + c, list.map(p => new Option(p.name, 'cue:' + p.id)))),
  );
}
function pgContentFr(c) {
  const shape = s => { const b = $('shapes').querySelector(`button[data-shape="${CSS.escape(s)}"]`); return b ? b.textContent : s; };
  switch (c.kind) {
    case 'generator': return 'Effet · ' + (GEN_NAMES[c.generator] || c.generator);
    case 'shape': return 'Forme · ' + shape(c.shape);
    case 'text': return `Texte « ${c.text} »`;
    case 'wave': return 'Onde';
    case 'evolving': return `Cue évolutif (${(c.keys || []).length} clés)`;
    case 'figure': return `Figure « ${c.name || ''} »`;
    case 'ilda': return 'ILDA · ' + c.file;
    case 'abstract': return `Abstrait (${(c.oscs || []).length} oscillateurs)`;
    case 'wire': return `Objet 3D « ${c.name || ''} »`;
    default: return c.kind;
  }
}

// Every edit goes through here: applied, drawn, previewed. `light`: only
// the canvas and the key row (drags, sliders), the lists stay as they are.
function pgChange(fn, opts = {}) {
  if (!pg.prog) return;
  fn(pg.prog);
  for (const l of pg.prog.lanes) l.keys.sort((a, b) => a.beat - b.beat);
  if (opts.light) { $('pgModified').hidden = !pgDirty(); pgRenderKeyRow(); pgDraw(); } else pgRender();
  pgSchedulePreview();
}
function pgRender() {
  const p = pg.prog, info = pg.info;
  if (!p) return;
  $('pgCueName').textContent = info.name;
  $('pgCueName').title = info.category;
  $('pgCueInfo').textContent = `${info.user ? 'Cue utilisateur' : 'Cue intégré'} · ${info.category} · `
    + (info.own ? 'programme de la case' : info.program ? 'programme du look' : 'sans programme');
  $('pgRemove').hidden = !info.own;
  const lens = [...new Set([...PG_LENGTHS, p.length_beats])].sort((a, b) => a - b);
  $('pgLen').innerHTML = lens.map(l => `<option value="${l}">${+l.toFixed(3)} temps</option>`).join('');
  $('pgLen').value = String(p.length_beats);
  document.querySelectorAll('#pgPlay [data-play]').forEach(b => {
    b.classList.toggle('active', b.dataset.play === p.play);
    b.setAttribute('aria-pressed', String(b.dataset.play === p.play));
  });
  $('pgModified').hidden = !pgDirty();
  pgRenderLayers(); pgRenderLfos(); pgRenderHeads(); pgRenderKeyRow(); pgDraw();
}
function pgRenderHeads() {
  const p = pg.prog, used = new Set(p.lanes.map(l => l.target));
  const avail = pg.info.targets.filter(t => !used.has(t.id));
  const top = document.createElement('div');
  top.className = 'top'; top.textContent = 'Mesures';
  const heads = p.lanes.map(l => {
    const h = document.createElement('div');
    h.className = 'pg-head'; h.dataset.target = l.target;
    const s = document.createElement('select');
    s.setAttribute('aria-label', 'Paramètre de la courbe');
    s.title = 'Paramètre de la courbe (les clés sont ramenées dans sa plage)';
    s.append(...[pgTarget(l.target), ...avail].map(t => new Option(pgTargetFr(t.id), t.id)));
    s.value = l.target;
    s.onchange = () => pgChange(() => {
      const t = pgTarget(s.value);
      l.target = s.value;
      l.keys.forEach(k => { k.value = Math.min(t.max, Math.max(t.min, k.value)); });
    });
    const t = pgTarget(l.target), row = document.createElement('div');
    row.className = 'row';
    const rng = document.createElement('span');
    rng.className = 'rng';
    rng.textContent = `${tlFmtV(t.min)} … ${tlFmtV(t.max)} · ${l.keys.length} clé${l.keys.length > 1 ? 's' : ''}`;
    const del = document.createElement('button');
    del.textContent = '×'; del.title = 'Supprimer la courbe';
    del.setAttribute('aria-label', 'Supprimer la courbe ' + pgTargetFr(l.target));
    del.onclick = () => pgChange(q => { q.lanes.splice(q.lanes.indexOf(l), 1); });
    row.append(rng, del);
    h.append(s, row);
    return h;
  });
  if (!heads.length) {
    const e = document.createElement('div');
    e.className = 'pg-head empty';
    e.textContent = 'Aucune courbe : choisissez un paramètre, puis « Ajouter une courbe ».';
    heads.push(e);
  }
  $('pgHeads').replaceChildren(top, ...heads);
  const was = $('pgAddTarget').value;
  $('pgAddTarget').innerHTML = avail.map(t => `<option value="${t.id}">${lfoEsc(pgTargetFr(t.id))}</option>`).join('');
  $('pgAddTarget').value = avail.some(t => t.id === was) ? was : avail.some(t => t.id === 'size') ? 'size' : (avail[0] || {}).id || '';
  $('pgAddLane').disabled = !avail.length || p.lanes.length >= pg.info.max.lanes;
}
function pgRenderKeyRow() {
  const s = pg.sel, on = !!(s && pg.prog && pg.prog.lanes.includes(s.lane) && s.lane.keys.includes(s.key));
  if (!on) pg.sel = null;
  $('pgKeyRow').setAttribute('aria-disabled', String(!on));
  $('pgKeyLbl').textContent = on ? `Clé de « ${pgTargetFr(s.lane.target)} » :` : 'Clé : aucune (cliquez dans une courbe)';
  if (!on) return;
  const t = pgTarget(s.lane.target);
  $('pgKeyBeat').max = pgLen();
  if (document.activeElement !== $('pgKeyBeat')) $('pgKeyBeat').value = +s.key.beat.toFixed(3);
  $('pgKeyValue').min = t.min; $('pgKeyValue').max = t.max;
  if (document.activeElement !== $('pgKeyValue')) $('pgKeyValue').value = +s.key.value.toFixed(4);
  $('pgKeyEase').value = s.key.ease;
}
function pgRenderLayers() {
  const p = pg.prog;
  const btn = (txt, title, fn, disabled) => {
    const b = document.createElement('button');
    b.textContent = txt; b.title = title; b.setAttribute('aria-label', title); b.disabled = !!disabled; b.onclick = fn;
    return b;
  };
  const rows = p.layers.map((c, i) => {
    const row = document.createElement('div');
    row.className = 'pg-item';
    const n = document.createElement('span');
    n.className = 'n'; n.textContent = `${i + 1}. ${pgContentFr(c)}`;
    const move = d => pgChange(q => q.layers.splice(i + d, 0, q.layers.splice(i, 1)[0]));
    row.append(n, btn('↑', 'Monter la couche', () => move(-1), i === 0), btn('↓', 'Descendre la couche', () => move(1), i === p.layers.length - 1),
      btn('×', 'Supprimer la couche', () => pgChange(q => q.layers.splice(i, 1))));
    return row;
  });
  if (!rows.length) { const e = document.createElement('div'); e.className = 'hint'; e.textContent = 'Aucune couche : le cue dessine son seul contenu.'; rows.push(e); }
  $('pgLayers').replaceChildren(...rows);
  $('pgLayerAdd').disabled = p.layers.length >= pg.info.max.layers;
  $('pgLayerAdd').title = p.layers.length >= pg.info.max.layers ? `${pg.info.max.layers} couches au plus` : 'Ajoute ce contenu comme couche interne';
}
function pgRenderLfos() {
  const p = pg.prog, spin = new Set(pg.info.spin_targets);
  const targets = pg.info.targets.filter(t => !spin.has(t.id)).map(t => [t.id, pgTargetFr(t.id)]);
  const opts = (list, sel) => list.map(([v, t]) => `<option value="${v}"${String(v) === String(sel) ? ' selected' : ''}>${lfoEsc(t)}</option>`).join('');
  $('pgLfos').innerHTML = p.lfos.length ? p.lfos.map((m, i) => {
    const period = m.rate.beats !== undefined ? m.rate.beats : 'hz';
    return `<div class="lfo" data-i="${i}">
      <div class="row"><select data-f="target" title="Paramètre">${opts(targets, m.target)}</select>
        <button class="del" data-del="${i}" title="Supprimer ce modulateur" aria-label="Supprimer ce modulateur">×</button></div>
      <div class="row"><select data-f="wave" title="Forme">${opts(LFO_WAVES, m.wave)}</select>
        <select data-f="period" title="Période">${opts([...LFO_PERIODS, ['hz', 'Hz (libre)']], period)}</select>
        ${period === 'hz' ? `<input type="number" data-f="hz" min="0" max="20" step="0.05" value="${m.rate.hz}" title="Fréquence (Hz)">` : ''}</div>
      <label>Profondeur <span class="v">${Math.round(m.depth * 100)} %</span></label>
      <input type="range" data-f="depth" min="0" max="100" data-default="50" value="${Math.round(m.depth * 100)}">
      <label>Décalage <span class="v">${Math.round(m.offset * 100)} %</span></label>
      <input type="range" data-f="offset" min="-100" max="100" data-default="0" value="${Math.round(m.offset * 100)}">
      <label>Phase <span class="v">${Math.round(m.phase * 360)}°</span></label>
      <input type="range" data-f="phase" min="0" max="99" data-default="0" data-display-k="3.6" value="${Math.round(m.phase * 100)}">
      <label class="check u-justify-start"><input type="checkbox" data-f="enabled"${m.enabled ? ' checked' : ''}> Actif</label>
    </div>`;
  }).join('') : '<div class="hint">Aucun modulateur.</div>';
  $('pgLfoAdd').disabled = p.lfos.length >= pg.info.max.lfos;
}

// --- the curves canvas ---
function pgColors() {
  if (!pg.colors) {
    const cs = getComputedStyle(document.documentElement), c = n => cs.getPropertyValue(n).trim();
    pg.colors = { line: c('--line'), muted: c('--muted'), text: c('--text'), panel: c('--panel'), accent: c('--accent'), cr: c('--ws-creation') || '#b45cff' };
  }
  return pg.colors;
}
function pgGeom() {
  const w = $('pgCanvas').clientWidth || 600, x0 = PG_PAD, x1 = w - PG_PAD, len = pgLen();
  return { w, x0, x1, len, bx: b => x0 + b / len * (x1 - x0), xb: x => (x - x0) / (x1 - x0) * len };
}
function pgLaneGeom(i, lane) {
  const t = pgTarget(lane.target), top = PG_RULER + i * PG_LANE, y0 = top + 8, y1 = top + PG_LANE - 8, span = (t.max - t.min) || 1;
  return {
    t, top,
    vy: v => y1 - (Math.max(t.min, Math.min(t.max, v)) - t.min) / span * (y1 - y0),
    yv: y => t.min + (y1 - Math.max(y0, Math.min(y1, y))) / (y1 - y0) * span,
  };
}
function pgDraw() {
  const cv = $('pgCanvas'), p = pg.prog;
  if (!p || $('pgEditor').hidden) return;
  const h = PG_RULER + Math.max(1, p.lanes.length) * PG_LANE, dpr = window.devicePixelRatio || 1, G = pgGeom(), C = pgColors();
  cv.style.height = h + 'px';
  if (cv.width !== Math.round(G.w * dpr) || cv.height !== Math.round(h * dpr)) { cv.width = Math.round(G.w * dpr); cv.height = Math.round(h * dpr); }
  const g = cv.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, G.w, h);
  // Ruler and beat grid: a line per beat, a stronger one and its number per bar.
  const bpb = (pgTempo && pgTempo.beats_per_bar) || 4;
  g.font = '10px system-ui, sans-serif'; g.textBaseline = 'middle'; g.lineWidth = 1;
  for (let b = 0; b <= G.len + 1e-6; b++) {
    const x = Math.round(G.bx(b)) + 0.5, bar = b % bpb === 0;
    g.strokeStyle = bar ? C.muted : C.line; g.globalAlpha = bar ? 0.7 : 0.8;
    g.beginPath(); g.moveTo(x, bar ? 4 : PG_RULER - 8); g.lineTo(x, h); g.stroke();
    g.globalAlpha = 1;
    if (bar && b < G.len - 1e-6) { g.fillStyle = C.muted; g.fillText(String(b / bpb + 1), x + 3, 10); }
  }
  p.lanes.forEach((lane, i) => {
    const L = pgLaneGeom(i, lane);
    g.strokeStyle = C.line; g.beginPath(); g.moveTo(0, L.top + 0.5); g.lineTo(G.w, L.top + 0.5); g.stroke();
    // The value without a curve, dashed.
    const yd = Math.round(L.vy(L.t.default)) + 0.5;
    g.setLineDash([3, 4]); g.strokeStyle = C.muted; g.globalAlpha = 0.5;
    g.beginPath(); g.moveTo(G.x0, yd); g.lineTo(G.x1, yd); g.stroke();
    g.setLineDash([]); g.globalAlpha = 1;
    // The curve, through program::curve_at's twin (tlCurveAt).
    const keys = lane.keys.map(k => ({ at: k.beat, value: k.value, ease: k.ease }));
    if (keys.length) {
      g.strokeStyle = C.cr; g.lineWidth = 2; g.beginPath();
      for (let x = G.x0; x <= G.x1 + 0.5; x += 2) {
        const y = L.vy(tlCurveAt(keys, G.xb(x)));
        if (x === G.x0) g.moveTo(x, y); else g.lineTo(x, y);
      }
      g.stroke(); g.lineWidth = 1;
    }
    for (const k of lane.keys) {
      const sel = pg.sel && pg.sel.key === k;
      g.beginPath(); g.arc(G.bx(k.beat), L.vy(k.value), sel ? PG_KEY_R + 1.5 : PG_KEY_R, 0, 2 * Math.PI);
      g.fillStyle = sel ? C.text : C.cr; g.fill();
      g.strokeStyle = sel ? C.cr : C.panel; g.lineWidth = 2; g.stroke(); g.lineWidth = 1;
    }
  });
  if (pg.pv) {
    const x = Math.round(G.bx(Math.min(G.len, pg.pvBeat))) + 0.5;
    g.strokeStyle = C.accent; g.beginPath(); g.moveTo(x, 0); g.lineTo(x, h); g.stroke();
  }
}
// The lane, key and geometry under the mouse.
function pgHit(e) {
  const r = $('pgCanvas').getBoundingClientRect(), x = e.clientX - r.left, y = e.clientY - r.top;
  const i = Math.floor((y - PG_RULER) / PG_LANE), lane = y >= PG_RULER && pg.prog ? pg.prog.lanes[i] : null;
  if (!lane) return { lane: null };
  const L = pgLaneGeom(i, lane), G = pgGeom();
  let key = null, best = 8;
  for (const k of lane.keys) {
    const d = Math.hypot(G.bx(k.beat) - x, L.vy(k.value) - y);
    if (d < best) { best = d; key = k; }
  }
  return { x, y, lane, key, L, G };
}
// Snapped to the beat unless Alt is held.
const pgSnap = (b, free, len) => Math.max(0, Math.min(len, free ? Math.round(b * 1000) / 1000 : Math.round(b)));
$('pgCanvas').addEventListener('pointerdown', e => {
  if (e.button !== 0 || !pg.prog) return;
  const h = pgHit(e);
  if (!h.lane) return;
  e.preventDefault();
  $('pgCanvas').setPointerCapture(e.pointerId);
  if (h.key) {
    pg.sel = { lane: h.lane, key: h.key };
    pg.drag = { lane: h.lane, key: h.key, id: e.pointerId };
    pgRenderKeyRow(); pgDraw();
    return;
  }
  if (h.lane.keys.length >= pg.info.max.keys) { pgMsg(`${pg.info.max.keys} clés par courbe au plus.`, true); return; }
  const key = { beat: pgSnap(h.G.xb(h.x), e.altKey, h.G.len), value: +h.L.yv(h.y).toFixed(4), ease: 'linear' };
  pg.sel = { lane: h.lane, key };
  pg.drag = { lane: h.lane, key, id: e.pointerId };
  pgChange(() => h.lane.keys.push(key));
});
$('pgCanvas').addEventListener('pointermove', e => {
  const d = pg.drag;
  if (!d || d.id !== e.pointerId) return;
  const i = pg.prog.lanes.indexOf(d.lane);
  if (i < 0) return;
  const r = $('pgCanvas').getBoundingClientRect(), G = pgGeom(), L = pgLaneGeom(i, d.lane);
  const beat = pgSnap(G.xb(e.clientX - r.left), e.altKey, G.len), value = +L.yv(e.clientY - r.top).toFixed(4);
  if (beat === d.key.beat && value === d.key.value) return;
  pgChange(() => { d.key.beat = beat; d.key.value = value; }, { light: true });
});
const pgDragEnd = e => { if (pg.drag && pg.drag.id === e.pointerId) { pg.drag = null; if (pg.prog) pgRender(); } };
$('pgCanvas').addEventListener('pointerup', pgDragEnd);
$('pgCanvas').addEventListener('pointercancel', pgDragEnd);
$('pgCanvas').addEventListener('contextmenu', e => {
  e.preventDefault();
  const h = pgHit(e);
  if (h.key) pgChange(() => h.lane.keys.splice(h.lane.keys.indexOf(h.key), 1));
});
addEventListener('resize', () => pgDraw());

// --- live preview: pictures of the program at the tempo, never the laser ---
function pgSchedulePreview(delay = 150) {
  clearTimeout(pg.pvDebounce);
  pg.pvDebounce = setTimeout(pgFetchPreview, delay);
}
async function pgFetchPreview() {
  if (!pg.prog || !pg.id || $('pgEditor').hidden) return;
  const seq = ++pg.pvSeq;
  const r = await pgPost('/api/cues/program/preview', { id: pg.id, program: pg.prog });
  if (seq !== pg.pvSeq) return;
  if (!r.ok) { pgMsg(pgRefusal(r.status, r.text, 'Aperçu impossible'), true); return; }
  let pv;
  try { pv = JSON.parse(r.text); } catch { return; }
  if ($('pgMsg').classList.contains('err')) pgMsg('');
  pg.pv = pv; pg.pvAt = 0;
  pgPlayPreview();
}
// Aller-retour plays the pictures back too; the others loop.
function pgFrameIndex() {
  const n = pg.pv.frames.length;
  if (pg.prog.play !== 'ping_pong' || n < 2) return pg.pvAt % n;
  const k = pg.pvAt % (2 * n - 2);
  return k < n ? k : 2 * n - 2 - k;
}
function pgPlayPreview() {
  clearTimeout(pg.pvTimer);
  if (!pg.pv || !pg.pv.frames.length || $('pgEditor').hidden) return;
  const step = () => {
    if ($('pgEditor').hidden || !pg.pv) return;
    const i = pgFrameIndex(), bpm = Math.max(20, +$('bpm').value || 120);
    drawThumb($('pgPreview'), pg.pv.frames[i]);
    pg.pvBeat = i * pg.pv.step_beats;
    $('pgPvInfo').textContent = `Aperçu à ${+bpm.toFixed(1)} BPM · temps ${(pg.pvBeat + 1).toFixed(1).replace('.', ',')} / ${+pgLen().toFixed(3)} · jamais envoyé au laser`;
    if (!pg.drag) pgDraw();
    pg.pvAt++;
    pg.pvTimer = setTimeout(step, 1000 * pg.pv.step_beats * 60 / bpm);
  };
  step();
}
function pgStopPreview() { clearTimeout(pg.pvTimer); clearTimeout(pg.pvDebounce); }

// --- controls ---
$('pgCue').addEventListener('change', () => pgOpen($('pgCue').value));
$('pgLen').addEventListener('change', () => pgChange(q => { q.length_beats = +$('pgLen').value; }));
document.querySelectorAll('#pgPlay [data-play]').forEach(b => b.addEventListener('click', () => pgChange(q => { q.play = b.dataset.play; })));
$('pgAddLane').onclick = () => {
  const target = $('pgAddTarget').value;
  if (!target || !pg.prog || pg.prog.lanes.length >= pg.info.max.lanes || pg.prog.lanes.some(l => l.target === target)) return;
  pgChange(q => q.lanes.push({ target, keys: [] }));
  pgMsg(`Courbe « ${pgTargetFr(target)} » ajoutée : cliquez dedans pour poser des clés.`);
};
$('pgKeyEase').innerHTML = TL_EASES.map(([v, t]) => `<option value="${v}">${t}</option>`).join('');
$('pgKeyBeat').addEventListener('change', () => {
  const s = pg.sel;
  if (s) pgChange(() => { s.key.beat = Math.max(0, Math.min(pgLen(), +$('pgKeyBeat').value || 0)); });
});
$('pgKeyValue').addEventListener('change', () => {
  const s = pg.sel;
  if (!s) return;
  const t = pgTarget(s.lane.target);
  pgChange(() => { s.key.value = Math.max(t.min, Math.min(t.max, +$('pgKeyValue').value || 0)); });
});
$('pgKeyEase').addEventListener('change', () => { const s = pg.sel; if (s) pgChange(() => { s.key.ease = $('pgKeyEase').value; }); });
$('pgKeyDel').onclick = () => { const s = pg.sel; if (s) pgChange(() => s.lane.keys.splice(s.lane.keys.indexOf(s.key), 1)); };
$('pgLayerAdd').onclick = async () => {
  const v = $('pgLayerSrc').value;
  if (!v || !pg.prog) return;
  let content = null;
  if (v.startsWith('gen:')) content = { kind: 'generator', generator: v.slice(4) };
  else if (v.startsWith('shape:')) content = { kind: 'shape', shape: v.slice(6) };
  else if (v === 'wave') content = { kind: 'wave' };
  else if (v.startsWith('cue:')) {
    try {
      const r = await api('GET', '/api/cues/program?id=' + encodeURIComponent(v.slice(4)), undefined, { cache: 'no-store' });
      if (r.ok) content = (await r.json()).content;
    } catch { /* nothing added */ }
  }
  if (content && pg.prog.layers.length < pg.info.max.layers) pgChange(q => q.layers.push(content));
};
$('pgLfoAdd').onclick = () => {
  if (!pg.prog || pg.prog.lfos.length >= pg.info.max.lfos) return;
  pgChange(q => q.lfos.push({ target: 'size', wave: 'sine', rate: { beats: 4 }, depth: 0.5, phase: 0, offset: 0, enabled: true }));
};
$('pgLfos').addEventListener('click', e => {
  const del = e.target.dataset.del;
  if (del !== undefined) pgChange(q => q.lfos.splice(+del, 1));
});
// Sliders move while dragged (the list stays); menus and the box re-render.
function pgLfoEdit(e, final) {
  const f = e.target.dataset.f, box = e.target.closest('.lfo');
  if (!f || !box || !pg.prog) return;
  const m = pg.prog.lfos[+box.dataset.i], v = e.target.value;
  if (!m) return;
  if (['depth', 'offset', 'phase'].includes(f)) {
    pgChange(() => { m[f] = +v / 100; }, { light: true });
    e.target.previousElementSibling.querySelector('.v').textContent = f === 'phase' ? Math.round(m.phase * 360) + '°' : Math.round(m[f] * 100) + ' %';
    return;
  }
  pgChange(() => {
    if (f === 'target' || f === 'wave') m[f] = v;
    else if (f === 'period') m.rate = v === 'hz' ? { hz: 1 } : { beats: +v };
    else if (f === 'hz') m.rate = { hz: Math.min(20, Math.max(0, +v || 0)) };
    else if (f === 'enabled') m.enabled = e.target.checked;
  }, { light: !final });
}
$('pgLfos').addEventListener('input', e => { if (e.target.type === 'range') pgLfoEdit(e, false); });
$('pgLfos').addEventListener('change', e => { if (e.target.type !== 'range') pgLfoEdit(e, true); });

// Saves the program on its cue; false when refused (the panel says why).
async function pgSaveNow() {
  if (!pg.prog) return false;
  if (showMode) { pgMsg(LOCKED_FR, true); return false; }
  const program = JSON.parse(JSON.stringify(pg.prog));
  const r = await pgPost('/api/cues/program', { id: pg.id, program });
  if (!r.ok) { pgMsg(pgRefusal(r.status, r.text, 'Programme refusé'), true); return false; }
  pg.saved = JSON.stringify(program);
  pg.info.own = true; pg.info.program = program;
  pgRender();
  pgMsg(`Programme enregistré sur « ${pg.info.name} » : il joue au prochain lancement du cue.`);
  loadSlots();
  return true;
}
$('pgSave').onclick = pgSaveNow;
$('pgCancel').onclick = () => {
  if (!pg.prog) return;
  const was = pgDirty();
  pg.prog = JSON.parse(pg.saved); pg.sel = null; pg.drag = null;
  pgRender();
  pgSchedulePreview(0);
  pgMsg(was ? 'Modifications annulées : le cue garde son programme enregistré.' : 'Rien à annuler.');
};
$('pgStore').onclick = async () => {
  if (!pg.prog) return;
  if (showMode) { pgMsg(LOCKED_FR, true); return; }
  const r = await pgPost('/api/cues/program/store', { id: pg.id, program: pg.prog });
  if (!r.ok) { pgMsg(pgRefusal(r.status, r.text, 'Enregistrement refusé'), true); return; }
  const g = JSON.parse(r.text), page = g.pages[g.cell[0]];
  await loadPresets();
  await pgOpen(g.cue, true);
  pgMsg(`Enregistré dans la page « ${page ? page.name : g.cell[0] + 1} », case ${g.cell[1] + 1} : nouveau cue « ${pg.info.name} ».`);
};
$('pgRemove').onclick = async () => {
  if (!pg.id) return;
  const r = await pgPost('/api/cues/program', { id: pg.id, program: null });
  if (!r.ok) { pgMsg(pgRefusal(r.status, r.text, 'Refusé'), true); return; }
  await pgOpen(pg.id, true);
  pgMsg('Programme retiré de la case.');
  loadSlots();
};
// « Propriétés du cue » › « Modifier le programme… ».
$('cueMenuProgram').onclick = () => {
  const id = menuCue;
  $('cueMenu').hidden = true;
  if (!id) return;
  setWorkspace('creation');
  setCreationTab('program');
  pgOpen(id);
};

// ---------- CRÉATION › Abstraits: stacked oscillators (T-440) ----------
// An abstract is a curve made of up to six oscillators on X, Y and the
// hue (abstracts.rs). The editor keeps its own look, here in the page:
// every change is drawn by the engine (POST /api/abstracts/preview, its own
// animator, never the laser, never what plays in LIVE) and « Enregistrer
// comme cue… » makes a user cue of it (POST /api/abstracts/store, in the
// history, refused in show mode).
const ABS_KEY = 'laserStudio.abstract';
const ABS_MAX = 6;
const ABS_WAVES = [['sine', 'Sinus'], ['triangle', 'Triangle'], ['square', 'Carré'], ['saw_up', 'Dent de scie ↑'], ['saw_down', 'Dent de scie ↓']];
const ABS_TARGETS = [['x', 'X'], ['y', 'Y'], ['color', 'Couleur']];
const absOsc = (target = 'x', ratio = 1, phase_deg = 0) => ({ target, ratio, phase_deg, amp: 1, wave: 'sine', drift: 0, enabled: true });
const absDefault = () => ({ content: { kind: 'abstract', oscs: [absOsc('x', 1, 90), absOsc('y', 1, 0)], points: 400, turns: 1, auto_turns: true, closed: true }, color: [0, 255, 0], scale: 0.6, brightness: 1 });
const ab = { look: absDefault(), pv: null, pvAt: 0, pvTimer: 0, pvDebounce: 0, pvSeq: 0 };
try { const saved = JSON.parse(remembered(ABS_KEY) || 'null'); if (saved && saved.content && saved.content.kind === 'abstract') ab.look = saved; } catch { /* a fresh editor */ }
const absShape = () => ab.look.content;
const absNum = (v, def) => Number.isFinite(+v) ? +v : def;
function absMsg(text, err = false) {
  $('absMsg').textContent = text;
  $('absMsg').classList.toggle('err', err);
}

function absSync() {
  const on = activeWs === 'creation' && $('workspace').dataset.ctab === 'abstract';
  if (!on) { clearTimeout(ab.pvTimer); clearTimeout(ab.pvDebounce); return; }
  absRender();
  absSchedule(0);
}

// One row per oscillator: Cible, Rapport, Phase, Amplitude, Forme, Dérive, Actif, Supprimer.
function absRender() {
  const sh = absShape(), box = $('absOscs');
  box.replaceChildren();
  sh.oscs.forEach((o, i) => {
    const n = i + 1, row = document.createElement('div');
    row.className = 'abs-osc' + (o.enabled ? '' : ' off');
    row.dataset.osc = String(i);
    const head = document.createElement('div');
    head.className = 'row';
    const num = document.createElement('span');
    num.className = 'abs-n'; num.textContent = String(n);
    const seg = document.createElement('div');
    seg.className = 'seg'; seg.setAttribute('role', 'group'); seg.setAttribute('aria-label', `Cible de l'oscillateur ${n}`);
    for (const [t, fr] of ABS_TARGETS) {
      const b = document.createElement('button');
      b.textContent = fr; b.dataset.target = t;
      b.classList.toggle('active', o.target === t);
      b.setAttribute('aria-pressed', String(o.target === t));
      b.onclick = () => absEdit(s => { s.oscs[i].target = t; }, true);
      seg.append(b);
    }
    const on = document.createElement('label');
    on.className = 'check';
    const cb = document.createElement('input');
    cb.type = 'checkbox'; cb.checked = o.enabled; cb.className = 'abs-on';
    cb.onchange = () => absEdit(s => { s.oscs[i].enabled = cb.checked; }, true);
    const onTxt = document.createElement('span'); onTxt.textContent = 'Actif';
    on.append(cb, onTxt);
    const del = document.createElement('button');
    del.className = 'abs-del'; del.textContent = 'Supprimer';
    del.setAttribute('aria-label', `Supprimer l'oscillateur ${n}`);
    del.onclick = () => absEdit(s => { s.oscs.splice(i, 1); }, true);
    head.append(num, seg, on, del);
    row.append(head);
    // Rapport: typed (entier ou fractionnaire), 0,25 à 16.
    const field = (label, el, key) => {
      el.id = `abs${key}${n}`;
      const l = document.createElement('label');
      l.htmlFor = el.id; l.textContent = label;
      row.append(l, el);
    };
    const ratio = document.createElement('input');
    ratio.type = 'number'; ratio.min = '0.25'; ratio.max = '16'; ratio.step = 'any'; ratio.value = String(o.ratio);
    ratio.className = 'abs-ratio';
    ratio.onchange = () => absEdit(s => { s.oscs[i].ratio = Math.min(16, Math.max(0.25, absNum(ratio.value, 1))); }, true);
    field('Rapport', ratio, 'Ratio');
    const slider = (label, key, min, max, step, def, value, unit, set) => {
      const el = document.createElement('input');
      el.type = 'range'; el.min = String(min); el.max = String(max); el.step = String(step);
      el.dataset.default = String(def); el.value = String(value);
      el.className = 'abs-' + key.toLowerCase();
      el.id = `abs${key}${n}`;
      const l = document.createElement('label');
      l.htmlFor = el.id; l.textContent = label + ' ';
      const v = document.createElement('span');
      v.className = 'v'; v.id = el.id + 'V';
      const show = () => { v.textContent = unit(+el.value); };
      show();
      l.append(v);
      el.addEventListener('input', () => { show(); absEdit(s => set(s.oscs[i], +el.value)); });
      row.append(l, el);
    };
    const fr = (x, d = 2) => (+x.toFixed(d)).toString().replace('.', ',');
    slider('Phase', 'Phase', 0, 360, 1, 0, o.phase_deg, x => fr(x, 0) + '°', (oo, x) => { oo.phase_deg = x; });
    slider('Amplitude', 'Amp', 0, 1, 0.01, 1, o.amp, x => fr(x), (oo, x) => { oo.amp = x; });
    const wave = document.createElement('select');
    for (const [w, label] of ABS_WAVES) wave.append(new Option(label, w));
    wave.value = o.wave === 'random' ? 'sine' : o.wave;
    wave.className = 'abs-wave';
    wave.onchange = () => absEdit(s => { s.oscs[i].wave = wave.value; });
    field('Forme', wave, 'Wave');
    slider('Dérive (tours par temps)', 'Drift', -4, 4, 0.03125, 0, o.drift, x => fr(x, 3), (oo, x) => { oo.drift = x; });
    box.append(row);
  });
  $('absAdd').disabled = sh.oscs.length >= ABS_MAX;
  setVal('absPoints', sh.points); $('absPointsV').textContent = String(sh.points);
  setVal('absTurns', sh.turns); $('absTurnsV').textContent = String(sh.turns).replace('.', ',');
  $('absTurns').disabled = !!sh.auto_turns;
  setCheck('absAuto', !!sh.auto_turns);
  setVal('absColor', hex(ab.look.color));
  $('absLoad').disabled = !(settings && settings.content && settings.content.kind === 'abstract');
}

// An edit of the editor's look: kept in the page, previewed by the engine.
function absEdit(fn, rerender = false) {
  fn(absShape());
  remember(ABS_KEY, JSON.stringify(ab.look));
  if (rerender) absRender();
  absSchedule();
}

function absSchedule(delay = 120) {
  clearTimeout(ab.pvDebounce);
  ab.pvDebounce = setTimeout(absFetch, delay);
}
async function absFetch() {
  if (activeWs !== 'creation' || $('workspace').dataset.ctab !== 'abstract') return;
  const seq = ++ab.pvSeq;
  let r, text;
  try {
    r = await api('POST', '/api/abstracts/preview', { look: ab.look }, { quiet: true });
    text = await r.text();
  } catch { return; }
  if (seq !== ab.pvSeq) return;
  if (!r.ok) { absMsg(r.status === 423 ? LOCKED_FR : `Aperçu impossible (erreur ${r.status}).`, true); return; }
  let pv;
  try { pv = JSON.parse(text); } catch { return; }
  if ($('absMsg').classList.contains('err')) absMsg('');
  ab.pv = pv; ab.pvAt = 0;
  $('absBudget').textContent = `${pv.samples} points (budget ${pv.budget})`;
  $('absLimited').hidden = !pv.limited;
  absPlay();
}
function absPlay() {
  clearTimeout(ab.pvTimer);
  if (!ab.pv || !ab.pv.frames.length) return;
  const step = () => {
    if (activeWs !== 'creation' || $('workspace').dataset.ctab !== 'abstract' || !ab.pv) return;
    const n = ab.pv.frames.length, i = ab.pvAt % n, bpm = Math.max(20, +$('bpm').value || 120);
    drawThumb($('absPreview'), ab.pv.frames[i]);
    $('absPvInfo').textContent = n > 1
      ? `Aperçu à ${+bpm.toFixed(1)} BPM · ${ab.pv.points} points tracés · jamais envoyé au laser`
      : `Figure fixe · ${ab.pv.points} points tracés · jamais envoyée au laser`;
    ab.pvAt++;
    if (n > 1) ab.pvTimer = setTimeout(step, 1000 * ab.pv.step_beats * 60 / bpm);
  };
  step();
}

$('absAdd').onclick = () => {
  if (absShape().oscs.length >= ABS_MAX) return;
  absEdit(s => { s.oscs.push(absOsc(s.oscs.some(o => o.target === 'y') ? 'x' : 'y', 1, 0)); }, true);
};
$('absPoints').addEventListener('input', () => { $('absPointsV').textContent = $('absPoints').value; absEdit(s => { s.points = +$('absPoints').value; }); });
$('absTurns').addEventListener('input', () => { $('absTurnsV').textContent = $('absTurns').value.replace('.', ','); absEdit(s => { s.turns = +$('absTurns').value; }); });
$('absAuto').addEventListener('change', () => absEdit(s => { s.auto_turns = $('absAuto').checked; }, true));
$('absColor').addEventListener('input', () => { ab.look.color = rgb($('absColor').value); absEdit(() => {}); });
$('absReset').onclick = () => { ab.look = absDefault(); absEdit(() => {}, true); absMsg('Revenu au cercle de départ.'); };
$('absLoad').onclick = () => {
  if (!settings || settings.content.kind !== 'abstract') return;
  ab.look = { content: structuredClone(settings.content), color: settings.color.slice(), scale: settings.scale, brightness: 1 };
  absEdit(() => {}, true);
  absMsg('Abstrait du look en cours repris.');
};
$('absStore').onclick = async () => {
  if (showMode) { absMsg(LOCKED_FR, true); return; }
  const name = prompt('Nom du nouveau cue :', 'Mon abstrait');
  if (name === null) return;
  let r, text;
  try {
    r = await api('POST', '/api/abstracts/store', { look: ab.look, name: name.trim() || null }, { quiet: true });
    text = await r.text();
  } catch { absMsg('Le studio ne répond pas.', true); return; }
  if (!r.ok) { absMsg(r.status === 423 ? LOCKED_FR : `Enregistrement refusé : ${text}`, true); return; }
  const g = JSON.parse(text), page = g.pages[g.cell[0]];
  await loadPresets();
  absMsg(`Enregistré dans la page « ${page ? page.name : g.cell[0] + 1} », case ${g.cell[1] + 1}.`);
};

setFigTool('line');
showFigProps();
setCreationTab(remembered(CTAB_KEY));

export { absSync, assignPgTempo, pg, pgDirty, pgSaveNow, pgSync };
