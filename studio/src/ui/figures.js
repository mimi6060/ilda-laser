// Laser Studio — figures.js (T-435).
// Sections (as in the former index.html script):
//   CRÉATION › Figures: figure editor (T-296)
//   geometry
//   pointer
//   tools, properties, transforms
//   parts (T-441)
//   frames (animation)
//   drawing
//   library
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './project.js';
import { $, api, apiJson, hex, post, pps, rgb, setCheck, setVal } from './core.js';
import { shiftMod } from './presence.js';
import { activeWs, remember } from './workspaces.js';
import { loadPresets, refreshLook } from './cues.js';
import { fillScanSelect } from './presets.js';
import { loadMacros, loadScripts, macLoadCommands } from './macros.js';
import { loadLfos } from './modulators.js';
import { loadIldaLib, loadObjLib } from './media.js';
import { absSync, pgSync } from './program.js';
import { emptyState } from './command.js';

// ---------- CRÉATION › Figures: figure editor (T-296) ----------
// The figure is edited here, in the page; « Enregistrer » sends it to the
// library (studio-data/figures), « Jouer » saves it and plays its cue
// (page Figures) through the normal path: layers, live, calibration,
// safety and the laser button. Coordinates are the laser's: -1..1, y up.
const CTAB_KEY = 'laserStudio.creationTab';
const FIG_UNDO_MAX = 200, FIG_GRID = 0.1, FIG_HIT = 0.04;
const FIG_NAME_RE = /^[\p{L}\p{N} _-]{1,64}$/u;
const newFigure = () => ({ name: '', frames: [{ strokes: [] }], rate: 1, per: 'beat', loop_mode: 'loop' });
const fe = {
  fig: newFigure(), frame: 0, tool: 'line', sel: new Set(), draft: null, drag: null,
  undo: [], redo: [], color: [0, 255, 0], lit: true, playing: null, bpm: 120, budget: 750, lib: [],
  hoverPart: 0,
};
// T-433: the figure as last opened, saved or started; a drawn one that
// differs is unsaved work (closing the page asks).
let figBaseline = JSON.stringify(fe.fig);
const figMarkSaved = () => { figBaseline = JSON.stringify(fe.fig); };
const figureDirty = () => JSON.stringify(fe.fig) !== figBaseline && fe.fig.frames.some(f => f.strokes.length > 0);
const figFrame = () => fe.fig.frames[fe.frame];
const figStrokes = () => figFrame().strokes;

function setCreationTab(name) {
  if (name !== 'figures' && name !== 'media' && name !== 'program' && name !== 'macros' && name !== 'scripts' && name !== 'abstract' && name !== 'objects') name = 'look';
  document.querySelectorAll('#creationTabs [data-ctab]').forEach(b => {
    b.classList.toggle('active', b.dataset.ctab === name);
    b.setAttribute('aria-selected', String(b.dataset.ctab === name));
  });
  document.querySelectorAll('#creationPanel [data-cpanel]').forEach(el => { el.hidden = el.dataset.cpanel !== name; });
  $('workspace').dataset.ctab = name;
  remember(CTAB_KEY, name);
  syncFigView();
  if (name === 'macros') setTimeout(() => { loadMacros(true); macLoadCommands(); }, 0);
  if (name === 'scripts') setTimeout(() => loadScripts(true), 0);
}
function syncFigView() {
  const on = activeWs === 'creation' && $('workspace').dataset.ctab === 'figures';
  $('figEditor').hidden = !on;
  if (on) { loadFigLib(); figRender(true); }
  if (activeWs === 'creation' && $('workspace').dataset.ctab === 'media') loadIldaLib();
  if (activeWs === 'creation' && $('workspace').dataset.ctab === 'objects') loadObjLib(); // T-462
  pgSync(); // CRÉATION › Cue évolutif (T-164)
  absSync(); // CRÉATION › Abstraits (T-440)
}
document.querySelectorAll('#creationTabs [data-ctab]').forEach(b => b.onclick = () => setCreationTab(b.dataset.ctab));

// Undo / redo: whole-figure snapshots, taken before every change.
function figSnapshot() { return JSON.stringify({ fig: fe.fig, frame: fe.frame }); }
function figRestore(json) {
  const st = JSON.parse(json);
  fe.fig = st.fig; fe.frame = Math.min(st.frame, fe.fig.frames.length - 1);
  fe.sel.clear(); fe.draft = null; fe.drag = null;
  showFigProps(); figRender(true);
}
function pushUndo(snapshot) {
  fe.undo.push(snapshot);
  if (fe.undo.length > FIG_UNDO_MAX) fe.undo.shift();
  fe.redo = [];
}
function figChange(fn) {
  pushUndo(figSnapshot());
  fn();
  figRender(true);
}
function figUndo() {
  if (fe.draft) { fe.draft = null; figRender(); return; }
  if (!fe.undo.length) return;
  fe.redo.push(figSnapshot()); figRestore(fe.undo.pop());
}
function figRedo() {
  if (!fe.redo.length) return;
  fe.undo.push(figSnapshot()); figRestore(fe.redo.pop());
}

// ----- geometry -----
const round4 = v => Math.round(v * 10000) / 10000;
const clamp1 = v => Math.max(-1, Math.min(1, v));
function figPos(e) {
  const r = $('figCanvas').getBoundingClientRect();
  let x = clamp1((e.clientX - r.left) / r.width * 2 - 1), y = clamp1(1 - (e.clientY - r.top) / r.height * 2);
  if ($('figSnap').checked) { x = Math.round(x / FIG_GRID) * FIG_GRID; y = Math.round(y / FIG_GRID) * FIG_GRID; }
  return [round4(x), round4(y)];
}
function segDist(p, a, b) {
  const dx = b[0] - a[0], dy = b[1] - a[1], l2 = dx * dx + dy * dy;
  const t = l2 ? Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2)) : 0;
  return Math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy);
}
function strokeDist(p, s) {
  if (s.points.length === 1) return Math.hypot(p[0] - s.points[0][0], p[1] - s.points[0][1]);
  let d = Infinity;
  for (let i = 1; i < s.points.length; i++) d = Math.min(d, segDist(p, s.points[i - 1], s.points[i]));
  return d;
}
// The stroke under p (the last drawn wins on a tie), or -1.
function figHit(p) {
  let best = -1, bestD = FIG_HIT;
  figStrokes().forEach((s, i) => { const d = strokeDist(p, s); if (d <= bestD) { best = i; bestD = d; } });
  return best;
}
const ellipsePts = (cx, cy, rx, ry, n = 48) => Array.from({ length: n + 1 }, (_, i) => {
  const a = i / n * 2 * Math.PI; return [cx + rx * Math.cos(a), cy + ry * Math.sin(a)];
});
const polygonPts = (c, v, sides) => {
  const r = Math.hypot(v[0] - c[0], v[1] - c[1]), a0 = Math.atan2(v[1] - c[1], v[0] - c[0]);
  return Array.from({ length: sides + 1 }, (_, i) => { const a = a0 + i / sides * 2 * Math.PI; return [c[0] + r * Math.cos(a), c[1] + r * Math.sin(a)]; });
};
function bezierPts([p0, p1, p2, p3], n = 24) {
  return Array.from({ length: n + 1 }, (_, i) => {
    const t = i / n, u = 1 - t;
    return [0, 1].map(k => u * u * u * p0[k] + 3 * u * u * t * p1[k] + 3 * u * t * t * p2[k] + t * t * t * p3[k]);
  });
}
// Shape tools: the stroke for a drag from a to b.
function shapeFor(tool, a, b) {
  if (tool === 'rect') return [a, [b[0], a[1]], b, [a[0], b[1]], a];
  if (tool === 'ellipse') return ellipsePts((a[0] + b[0]) / 2, (a[1] + b[1]) / 2, Math.abs(b[0] - a[0]) / 2, Math.abs(b[1] - a[1]) / 2);
  if (tool === 'polygon') return polygonPts(a, b, Math.max(3, Math.min(24, Math.round(+$('figSides').value) || 5)));
  return [a, b];
}
const cleanPts = pts => pts.map(p => [round4(clamp1(p[0])), round4(clamp1(p[1]))])
  .filter((p, i, all) => i === 0 || Math.hypot(p[0] - all[i - 1][0], p[1] - all[i - 1][1]) > 1e-4);
function addStroke(points, lit = fe.lit) {
  points = cleanPts(points);
  if (!points.length) return;
  figChange(() => { figStrokes().push({ points, color: [...fe.color], lit }); fe.sel.clear(); });
  showFigProps();
}

// Polyline / blanked move / curve being drawn: finished by double-click,
// right-click, « Terminer », or picking another tool.
function finishDraft() {
  const d = fe.draft;
  fe.draft = null;
  if (!d) return;
  const pts = cleanPts(d.points);
  if (d.tool !== 'bezier' && pts.length >= 2) addStroke(pts, d.tool !== 'move');
  else figRender();
}

// ----- pointer -----
const cvF = $('figCanvas');
cvF.addEventListener('pointerdown', e => {
  if (e.button !== 0) return;
  e.preventDefault();
  const p = figPos(e), t = fe.tool;
  if (t === 'point') { addStroke([p]); return; }
  if (t === 'line' || t === 'move' || t === 'bezier') {
    if (!fe.draft || fe.draft.tool !== t) fe.draft = { tool: t, points: [] };
    fe.draft.points.push(p);
    if (t === 'bezier' && fe.draft.points.length === 4) { const pts = bezierPts(fe.draft.points); fe.draft = null; addStroke(pts); } else figRender();
    return;
  }
  if (t === 'rect' || t === 'ellipse' || t === 'polygon') {
    cvF.setPointerCapture(e.pointerId);
    fe.drag = { tool: t, from: p, to: p }; return;
  }
  if (t === 'text') { placeText(p); return; }
  if (t === 'erase') {
    const i = figHit(p);
    if (i >= 0) { figChange(() => { figStrokes().splice(i, 1); fe.sel.clear(); }); showFigProps(); }
    return;
  }
  // Sélection
  const i = figHit(p);
  if (i < 0) { if (!shiftMod(e)) fe.sel.clear(); showFigProps(); figRender(); return; }
  if (shiftMod(e)) { if (fe.sel.has(i)) fe.sel.delete(i); else fe.sel.add(i); }
  else if (!fe.sel.has(i)) fe.sel = new Set([i]);
  showFigProps();
  cvF.setPointerCapture(e.pointerId);
  fe.drag = { tool: 'select', from: p, before: figSnapshot(), orig: [...fe.sel].map(k => [k, structuredClone(figStrokes()[k].points)]), moved: false };
  figRender();
});
cvF.addEventListener('pointermove', e => {
  const d = fe.drag;
  if (!d) { if (fe.draft) { fe.draft.hover = figPos(e); figRender(); } return; }
  const p = figPos(e);
  if (d.tool === 'select') {
    const dx = p[0] - d.from[0], dy = p[1] - d.from[1];
    if (!dx && !dy && !d.moved) return;
    d.moved = true;
    for (const [k, pts] of d.orig) figStrokes()[k].points = pts.map(q => [round4(clamp1(q[0] + dx)), round4(clamp1(q[1] + dy))]);
  } else d.to = p;
  figRender();
});
cvF.addEventListener('pointerup', () => {
  const d = fe.drag;
  fe.drag = null;
  if (!d) return;
  if (d.tool === 'select') {
    if (d.moved) pushUndo(d.before);
    figRender(true); return;
  }
  if (Math.hypot(d.to[0] - d.from[0], d.to[1] - d.from[1]) < 0.01) { figRender(); return; }
  addStroke(shapeFor(d.tool, d.from, d.to));
});
cvF.addEventListener('dblclick', e => { e.preventDefault(); finishDraft(); });
cvF.addEventListener('contextmenu', e => { if (fe.draft) { e.preventDefault(); finishDraft(); } });

async function placeText(p) {
  const text = $('figText').value.trim();
  if (!text) return;
  const r = await api('POST', '/api/figures/text', { text, size: $('figTextSize').value / 100, font: $('figTextFont').value });
  if (!r.ok) return;
  const { strokes } = await r.json();
  if (!strokes.length) return;
  figChange(() => {
    for (const pts of strokes) figStrokes().push({ points: cleanPts(pts.map(q => [q[0] + p[0], q[1] + p[1]])), color: [...fe.color], lit: true });
    fe.sel.clear();
  });
  showFigProps();
}

// ----- tools, properties, transforms -----
function setFigTool(t) {
  if (fe.draft && fe.draft.tool !== t) finishDraft();
  fe.tool = t;
  document.querySelectorAll('#figTools [data-tool]').forEach(b => b.classList.toggle('active', b.dataset.tool === t));
  cvF.style.cursor = t === 'select' ? 'default' : 'crosshair';
}
document.querySelectorAll('#figTools [data-tool]').forEach(b => b.onclick = () => setFigTool(b.dataset.tool));
$('figFinish').onclick = finishDraft;
$('figUndo').onclick = figUndo;
$('figRedo').onclick = figRedo;
['figSnap', 'figOrder', 'figOnion'].forEach(id => $(id).onchange = () => figRender());
$('figTextSize').oninput = () => { $('figTextSizeV').textContent = $('figTextSize').value + ' %'; };
$('figTextSizeV').textContent = $('figTextSize').value + ' %';

const selected = () => [...fe.sel].filter(i => i < figStrokes().length).sort((a, b) => a - b);
$('figColor').onchange = () => {
  fe.color = rgb($('figColor').value);
  const sel = selected();
  if (sel.length) figChange(() => sel.forEach(i => { figStrokes()[i].color = [...fe.color]; }));
};
$('figLit').onchange = () => {
  fe.lit = $('figLit').checked;
  const sel = selected();
  if (sel.length) figChange(() => sel.forEach(i => { figStrokes()[i].lit = fe.lit; }));
};
function moveInOrder(dir) {
  const sel = selected();
  if (!sel.length) return;
  const s = figStrokes();
  if ((dir < 0 && sel[0] === 0) || (dir > 0 && sel[sel.length - 1] === s.length - 1)) return;
  figChange(() => {
    const order = dir < 0 ? sel : [...sel].reverse();
    for (const i of order) [s[i], s[i + dir]] = [s[i + dir], s[i]];
    fe.sel = new Set(sel.map(i => i + dir));
  });
}
$('figUp').onclick = () => moveInOrder(-1);
$('figDown').onclick = () => moveInOrder(1);
$('figReverse').onclick = () => { const sel = selected(); if (sel.length) figChange(() => sel.forEach(i => figStrokes()[i].points.reverse())); };
function deleteSelected() {
  const sel = selected();
  if (!sel.length) return;
  figChange(() => { figFrame().strokes = figStrokes().filter((_, i) => !sel.includes(i)); fe.sel.clear(); });
  showFigProps();
}
$('figDelete').onclick = deleteSelected;
// Duplicate the selection, a little to the right and down: colour, lit
// and part kept (T-441). The copies become the selection.
$('figDup').onclick = () => {
  const sel = selected();
  if (!sel.length) return;
  figChange(() => {
    const start = figStrokes().length;
    for (const i of sel) {
      const c = structuredClone(figStrokes()[i]);
      c.points = cleanPts(c.points.map(q => [q[0] + 0.05, q[1] - 0.05]));
      figStrokes().push(c);
    }
    fe.sel = new Set(sel.map((_, k) => start + k));
  });
  showFigProps();
};

// ----- parts (T-441) -----
// Each stroke carries a part number (absent = « Aucune »); the figure
// names its parts (1 to 8), common to every frame. LIVE can then set or
// modulate one part of the figure cue alone.
const FIG_MAX_PARTS = 8;
const figParts = () => fe.fig.parts || [];
const figPartCount = id => fe.fig.frames.reduce((n, f) => n + f.strokes.filter(s => (s.part || 0) === id).length, 0);
function figAddPart() {
  const used = new Set(figParts().map(p => p.id));
  for (let id = 1; id <= FIG_MAX_PARTS; id++) {
    if (used.has(id)) continue;
    fe.fig.parts = [...figParts(), { id, name: `Partie ${id}` }].sort((a, b) => a.id - b.id);
    return id;
  }
  return 0;
}
function fillFigPart() {
  const sel = selected(), el = $('figPart');
  const parts = new Set(sel.map(i => figStrokes()[i].part || 0));
  el.replaceChildren();
  const add = (v, t) => { const o = document.createElement('option'); o.value = String(v); o.textContent = t; el.appendChild(o); };
  if (parts.size > 1) add('mixed', 'Plusieurs');
  add(0, 'Aucune');
  figParts().forEach(p => add(p.id, p.name));
  if (figParts().length < FIG_MAX_PARTS) add('new', 'Nouvelle partie…');
  el.value = parts.size > 1 ? 'mixed' : String(sel.length ? [...parts][0] : 0);
  el.disabled = !sel.length;
}
$('figPart').onchange = () => {
  const sel = selected(), v = $('figPart').value;
  if (!sel.length || v === 'mixed') return;
  figChange(() => {
    const id = v === 'new' ? figAddPart() : +v;
    sel.forEach(i => { const s = figStrokes()[i]; if (id) s.part = id; else delete s.part; });
  });
  showFigProps();
};
function renderFigParts() {
  const box = $('figParts');
  box.replaceChildren();
  if (!figParts().length) {
    const h = document.createElement('div'); h.className = 'hint';
    h.textContent = 'Aucune partie : sélectionnez des tracés, puis Partie › « Nouvelle partie… ».';
    box.appendChild(h);
    return;
  }
  for (const p of figParts()) {
    const row = document.createElement('div'); row.className = 'figpart' + (fe.hoverPart === p.id ? ' hover' : ''); row.dataset.part = p.id;
    const lab = document.createElement('label'); lab.htmlFor = 'figPartName' + p.id; lab.textContent = p.id;
    const name = document.createElement('input'); name.type = 'text'; name.id = 'figPartName' + p.id; name.maxLength = 32; name.value = p.name;
    name.autocomplete = 'off'; name.title = 'Nom de la partie';
    name.onchange = () => figChange(() => { p.name = name.value.trim().slice(0, 32) || `Partie ${p.id}`; });
    const n = document.createElement('small'); const c = figPartCount(p.id); n.textContent = `${c} tracé${c > 1 ? 's' : ''}`;
    const pick = document.createElement('button'); pick.textContent = 'Sélectionner les traits';
    pick.title = `Sélectionner les tracés de « ${p.name} » dans cette image`;
    pick.onclick = () => {
      fe.sel = new Set(figStrokes().map((s, i) => [s, i]).filter(([s]) => (s.part || 0) === p.id).map(([, i]) => i));
      showFigProps(); figRender();
    };
    const del = document.createElement('button'); del.textContent = 'Supprimer la partie';
    del.title = `Supprimer « ${p.name} » : ses tracés restent, sans partie`;
    del.onclick = () => {
      figChange(() => {
        fe.fig.parts = figParts().filter(q => q.id !== p.id);
        fe.fig.frames.forEach(f => f.strokes.forEach(s => { if (s.part === p.id) delete s.part; }));
      });
      fe.hoverPart = 0;
      showFigProps();
    };
    row.onmouseenter = () => { fe.hoverPart = p.id; row.classList.add('hover'); figRender(); };
    row.onmouseleave = () => { fe.hoverPart = 0; row.classList.remove('hover'); figRender(); };
    row.append(lab, name, n, pick, del);
    box.appendChild(row);
  }
}
// Transforms act on the selection, or the whole image, around its centre.
function transform(f) {
  const idx = selected().length ? selected() : figStrokes().map((_, i) => i);
  if (!idx.length) return;
  const all = idx.flatMap(i => figStrokes()[i].points);
  const xs = all.map(p => p[0]), ys = all.map(p => p[1]);
  const c = [(Math.min(...xs) + Math.max(...xs)) / 2, (Math.min(...ys) + Math.max(...ys)) / 2];
  figChange(() => idx.forEach(i => {
    const s = figStrokes()[i];
    s.points = cleanPts(s.points.map(p => f(p[0] - c[0], p[1] - c[1]).map((v, k) => v + c[k])));
  }));
}
const rotateBy = deg => { const a = deg * Math.PI / 180, cs = Math.cos(a), sn = Math.sin(a); transform((x, y) => [x * cs - y * sn, x * sn + y * cs]); };
$('figRotL').onclick = () => rotateBy(15);
$('figRotR').onclick = () => rotateBy(-15);
$('figSmaller').onclick = () => transform((x, y) => [x * 0.9, y * 0.9]);
$('figBigger').onclick = () => transform((x, y) => [x * 1.1, y * 1.1]);
$('figMirX').onclick = () => transform((x, y) => [-x, y]);
$('figMirY').onclick = () => transform((x, y) => [x, -y]);

// ----- frames (animation) -----
function goFrame(i) { fe.frame = Math.max(0, Math.min(fe.fig.frames.length - 1, i)); fe.sel.clear(); fe.draft = null; showFigProps(); figRender(true); }
$('figAddFrame').onclick = () => figChange(() => { fe.fig.frames.splice(fe.frame + 1, 0, { strokes: [] }); fe.frame++; fe.sel.clear(); });
$('figDupFrame').onclick = () => figChange(() => { fe.fig.frames.splice(fe.frame + 1, 0, structuredClone(figFrame())); fe.frame++; fe.sel.clear(); });
$('figDelFrame').onclick = () => figChange(() => {
  if (fe.fig.frames.length > 1) fe.fig.frames.splice(fe.frame, 1); else fe.fig.frames[0].strokes = [];
  fe.frame = Math.min(fe.frame, fe.fig.frames.length - 1); fe.sel.clear();
});
function shiftFrame(dir) {
  const j = fe.frame + dir;
  if (j < 0 || j >= fe.fig.frames.length) return;
  figChange(() => { const f = fe.fig.frames; [f[fe.frame], f[j]] = [f[j], f[fe.frame]]; fe.frame = j; });
}
$('figFrameLeft').onclick = () => shiftFrame(-1);
$('figFrameRight').onclick = () => shiftFrame(1);
$('figRate').onchange = () => {
  const v = Math.max(0.05, Math.min(60, +$('figRate').value || 1));
  figChange(() => { fe.fig.rate = v; });
  showFigProps();
};
$('figPer').onchange = () => figChange(() => { fe.fig.per = $('figPer').value; });
$('figLoop').onchange = () => figChange(() => { fe.fig.loop_mode = $('figLoop').value; });
$('figScan').onchange = () => figChange(() => { const v = $('figScan').value; if (v) fe.fig.scan = v; else delete fe.fig.scan; });
// « Lire »: the animation in the editor, at the studio's tempo.
function stopFigPreview() {
  if (!fe.playing) return;
  clearInterval(fe.playing.timer); fe.playing = null;
  $('figPreview').classList.remove('active'); figRender(true);
}
$('figPreview').onclick = () => {
  if (fe.playing) { stopFigPreview(); return; }
  const t0 = performance.now();
  fe.playing = { frame: 0, timer: setInterval(() => {
    const secs = (performance.now() - t0) / 1000;
    fe.playing.frame = frameAt(fe.fig, fe.fig.per === 'beat' ? secs * fe.bpm / 60 : secs);
    figRender();
  }, 30) };
  $('figPreview').classList.add('active');
};
// Same stepping as figures.rs `frame_index`.
function frameAt(fig, pos) {
  const n = fig.frames.length;
  if (n <= 1) return 0;
  const step = Math.floor(Math.max(0, pos) * fig.rate);
  if (fig.loop_mode === 'once') return Math.min(step, n - 1);
  if (fig.loop_mode === 'ping_pong') { const period = 2 * n - 2, k = step % period; return k < n ? k : period - k; }
  return step % n;
}

// ----- drawing -----
const figCol = (c, a = 1) => `rgba(${c.join(',')},${a})`;
function drawStrokes(g, strokes, W, alpha, opts = {}) {
  const px = p => [(p[0] + 1) / 2 * W, (1 - p[1]) / 2 * W];
  let prev = null;
  strokes.forEach((s, i) => {
    const pts = s.points.map(px);
    if (opts.travel && prev && pts.length) {
      // Implicit blanked travel from the previous stroke.
      g.setLineDash([2, 6]); g.strokeStyle = '#ffffff30'; g.lineWidth = 1;
      g.beginPath(); g.moveTo(...prev); g.lineTo(...pts[0]); g.stroke(); g.setLineDash([]);
    }
    const sel = opts.sel && opts.sel.has(i);
    if (opts.hoverPart && (s.part || 0) === opts.hoverPart && pts.length) {
      // The strokes of the part under the pointer (T-441).
      g.save(); g.strokeStyle = '#22e3ff80'; g.lineWidth = 9; g.lineCap = 'round'; g.lineJoin = 'round';
      g.beginPath(); g.moveTo(...pts[0]); pts.slice(1).forEach(q => g.lineTo(...q)); if (pts.length === 1) g.lineTo(pts[0][0] + 0.1, pts[0][1]); g.stroke(); g.restore();
    }
    if (!s.lit) { g.setLineDash([6, 5]); g.strokeStyle = sel ? '#fff' : `rgba(150,150,150,${alpha})`; g.lineWidth = sel ? 2 : 1.2; }
    else { g.strokeStyle = figCol(s.color, alpha); g.lineWidth = sel ? 4 : 2.5; }
    if (sel && s.lit) { g.shadowColor = '#fff'; g.shadowBlur = 8; }
    if (pts.length === 1) {
      g.fillStyle = s.lit ? figCol(s.color, alpha) : g.strokeStyle;
      g.beginPath(); g.arc(pts[0][0], pts[0][1], sel ? 6 : 4, 0, 2 * Math.PI); g.fill();
    } else if (pts.length) {
      g.beginPath(); g.moveTo(...pts[0]); pts.slice(1).forEach(q => g.lineTo(...q)); g.stroke();
    }
    g.shadowBlur = 0; g.setLineDash([]);
    if (opts.order && pts.length) {
      // Drawing order: number at the start, arrow on the first segment.
      g.fillStyle = '#fff'; g.font = `${Math.round(W / 45)}px system-ui`;
      g.fillText(String(i + 1), pts[0][0] + 5, pts[0][1] - 5);
      if (pts.length > 1) {
        const [a, b] = [pts[0], pts[1]], m = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2], ang = Math.atan2(b[1] - a[1], b[0] - a[0]);
        g.save(); g.translate(...m); g.rotate(ang); g.beginPath(); g.moveTo(7, 0); g.lineTo(-5, -5); g.lineTo(-5, 5); g.closePath();
        g.fillStyle = s.lit ? figCol(s.color) : '#999'; g.fill(); g.restore();
      }
    }
    if (pts.length) prev = pts[pts.length - 1];
  });
}
function figRender(full = false) {
  const g = cvF.getContext('2d'), W = cvF.width;
  g.fillStyle = '#000'; g.fillRect(0, 0, W, W);
  g.lineWidth = 1;
  for (let k = -10; k <= 10; k++) {
    const v = (k / 10 + 1) / 2 * W;
    g.strokeStyle = k === 0 ? '#ffffff30' : $('figSnap').checked ? '#ffffff1c' : '#ffffff0c';
    g.beginPath(); g.moveTo(v, 0); g.lineTo(v, W); g.moveTo(0, v); g.lineTo(W, v); g.stroke();
  }
  const shown = fe.playing ? Math.min(fe.playing.frame, fe.fig.frames.length - 1) : fe.frame;
  if (!fe.playing && $('figOnion').checked && shown > 0) drawStrokes(g, fe.fig.frames[shown - 1].strokes, W, 0.22);
  drawStrokes(g, fe.fig.frames[shown].strokes, W, 1, fe.playing ? {} : { travel: true, order: $('figOrder').checked, sel: fe.sel, hoverPart: fe.hoverPart });
  // What is being drawn.
  const ghost = [];
  if (fe.draft) ghost.push(fe.draft.points.concat(fe.draft.hover ? [fe.draft.hover] : []));
  if (fe.drag && fe.drag.tool !== 'select') ghost.push(shapeFor(fe.drag.tool, fe.drag.from, fe.drag.to));
  g.setLineDash([5, 4]); g.strokeStyle = '#3ddc84'; g.lineWidth = 1.5;
  for (const pts of ghost) {
    const px = pts.map(p => [(p[0] + 1) / 2 * W, (1 - p[1]) / 2 * W]);
    g.beginPath(); px.forEach((q, i) => (i ? g.lineTo(...q) : g.moveTo(...q))); g.stroke();
    px.forEach(q => g.strokeRect(q[0] - 3, q[1] - 3, 6, 6));
  }
  g.setLineDash([]);
  $('figUndo').disabled = !fe.undo.length && !fe.draft;
  $('figRedo').disabled = !fe.redo.length;
  if (full) { renderFrames(); showFigStats(); }
}
function renderFrames() {
  const box = $('figFrames');
  box.innerHTML = '';
  fe.fig.frames.forEach((f, i) => {
    const b = document.createElement('button');
    b.className = 'figframe' + (i === fe.frame ? ' active' : '');
    b.title = `Image ${i + 1}`;
    const c = document.createElement('canvas');
    c.width = c.height = 112;
    const g = c.getContext('2d');
    g.fillStyle = '#000'; g.fillRect(0, 0, 112, 112);
    drawStrokes(g, f.strokes, 112, 1);
    const n = document.createElement('span'); n.textContent = i + 1;
    b.append(c, n);
    b.onclick = () => goFrame(i);
    box.appendChild(b);
  });
  $('figFrameInfo').textContent = `Image ${fe.frame + 1} / ${fe.fig.frames.length}`;
}

// Points the laser gets for one image: figures.rs then the engine's
// `densify` (steps of 0.03, 3 extra points on ends and corners). An estimate.
function laserPoints(frame) {
  const pts = [];
  for (const s of frame.strokes) {
    if (!s.points.length) continue;
    const lit = s.lit && s.color.some(v => v > 0);
    if (lit) pts.push(s.points[0]);
    if (lit && s.points.length === 1) for (let k = 0; k < 8; k++) pts.push(s.points[0]);
    else pts.push(...s.points);
  }
  let n = 0;
  pts.forEach((p, i) => {
    if (i) n += Math.max(0, Math.ceil(Math.hypot(p[0] - pts[i - 1][0], p[1] - pts[i - 1][1]) / 0.03) - 1);
    n += 1;
    if (i === 0 || i === pts.length - 1) { n += 3; return; }
    const a = pts[i - 1], c = pts[i + 1];
    const d1 = [p[0] - a[0], p[1] - a[1]], d2 = [c[0] - p[0], c[1] - p[1]], l1 = Math.hypot(...d1), l2 = Math.hypot(...d2);
    if (l1 > 1e-6 && l2 > 1e-6 && Math.acos(Math.max(-1, Math.min(1, (d1[0] * d2[0] + d1[1] * d2[1]) / (l1 * l2)))) > Math.PI / 6) n += 3;
  });
  return n;
}
function showFigStats() {
  const f = figFrame(), raw = f.strokes.reduce((a, s) => a + s.points.length, 0), laser = laserPoints(f);
  const worst = Math.max(...fe.fig.frames.map(laserPoints));
  const fps = laser ? pps / laser : 0;
  const over = worst > fe.budget;
  $('figStats').textContent = `Image : ${f.strokes.length} tracé(s), ${raw} point(s) dessiné(s), ≈ ${laser} points laser`
    + (laser ? ` (≈ ${fps.toFixed(0)} images/s à ${Math.round(pps / 1000)} kpps)` : '')
    + `. Image la plus lourde : ≈ ${worst} / budget ${fe.budget}.`
    + (over ? ' Au-delà du budget : les points seront espacés à la lecture.' : '');
  $('figStats').classList.toggle('bad', over || (laser > 0 && fps < 15));
}
// Tempo and point budget, from the preview's /api/frame.
function figFrameInfo(f) {
  if (f.tempo?.bpm) fe.bpm = f.tempo.bpm;
  const b = f.layers?.mixer?.point_budget;
  if (b && b !== fe.budget) { fe.budget = b; if (!$('figEditor').hidden) showFigStats(); }
}

function showFigProps() {
  const sel = selected();
  if (sel.length) {
    const s = figStrokes()[sel[0]];
    setVal('figColor', hex(s.color)); setCheck('figLit', s.lit);
  }
  setVal('figName', fe.fig.name);
  setVal('figRate', fe.fig.rate); setVal('figPer', fe.fig.per); setVal('figLoop', fe.fig.loop_mode);
  if (document.activeElement !== $('figScan')) fillScanSelect($('figScan'), 'Par défaut de la sortie', fe.fig.scan || '');
  ['figUp', 'figDown', 'figReverse', 'figDup', 'figDelete'].forEach(id => { $(id).disabled = !sel.length; });
  fillFigPart();
  if (!$('figParts').contains(document.activeElement)) renderFigParts();
}

// ----- library -----
function figMsg(text, err = false) { $('figMsg').textContent = text; $('figMsg').classList.toggle('err', err); }
async function loadFigLib() {
  try { fe.lib = await apiJson('GET', '/api/figures'); } catch { return; }
  const box = $('figLib');
  box.innerHTML = '';
  if (!fe.lib.length) {
    // T-437: the first figure, drawn or imported.
    box.appendChild(emptyState('Aucune figure enregistrée. Dessinez vos propres figures ou importez un SVG.',
      [['Nouvelle figure', () => $('figNew').click()], ['Importer…', () => $('figImport').click()]]));
    return;
  }
  for (const f of fe.lib) {
    const row = document.createElement('div'); row.className = 'figitem';
    const open = document.createElement('button'); open.className = 'n';
    open.textContent = `${f.name} · ${f.frames} image${f.frames > 1 ? 's' : ''}`;
    open.title = 'Ouvrir dans l\'éditeur'; open.dataset.fig = f.name;
    open.onclick = () => openFigure(f.name);
    const del = document.createElement('button'); del.className = 'del'; del.textContent = '×';
    del.title = `Supprimer « ${f.name} » de la bibliothèque`;
    del.onclick = async () => {
      if (!confirm(`Supprimer la figure « ${f.name} » ?`)) return;
      if (await post('/api/figures/delete', { name: f.name })) { figMsg(`« ${f.name} » supprimée.`); await loadFigLib(); await loadPresets(); loadLfos(); }
    };
    row.append(open, del); box.appendChild(row);
  }
}
async function openFigure(name) {
  const r = await api('POST', '/api/figures/load', { name }, { quiet: true });
  if (!r.ok) { figMsg(await r.text(), true); return; }
  const fig = await r.json();
  stopFigPreview();
  figChange(() => { fe.fig = fig; fe.frame = 0; fe.sel.clear(); fe.draft = null; });
  figMarkSaved();
  showFigProps(); figMsg(`« ${name} » ouverte.`);
}
async function saveFigure() {
  finishDraft();
  const name = $('figName').value.trim();
  if (!FIG_NAME_RE.test(name)) { figMsg('Nom invalide : lettres, chiffres, espaces, - et _ seulement, 64 caractères au plus.', true); $('figName').focus(); return null; }
  fe.fig.name = name;
  const r = await api('POST', '/api/figures', fe.fig, { quiet: true });
  if (!r.ok) { figMsg(await r.text(), true); return null; }
  const saved = await r.json();
  figMarkSaved();
  figMsg(`« ${saved.name} » enregistrée (cue de la page Figures).`);
  await loadFigLib(); await loadPresets(); loadLfos(); // the parts a modulator may target (T-441)
  return saved;
}
$('figName').onchange = () => { fe.fig.name = $('figName').value.trim(); };
$('figSave').onclick = saveFigure;
$('figPlay').onclick = async () => {
  const saved = await saveFigure();
  if (saved && await post('/api/presets/play', { id: saved.id })) refreshLook();
};
$('figNew').onclick = () => {
  stopFigPreview();
  figChange(() => { fe.fig = newFigure(); fe.frame = 0; fe.sel.clear(); fe.draft = null; });
  figMarkSaved();
  showFigProps(); figMsg('');
};

export { CTAB_KEY, FIG_NAME_RE, deleteSelected, drawStrokes, fe, figChange, figFrameInfo,
  figMarkSaved, figMsg, figRedo, figUndo, figureDirty, loadFigLib, saveFigure, setCreationTab,
  setFigTool, showFigProps, stopFigPreview, syncFigView };
