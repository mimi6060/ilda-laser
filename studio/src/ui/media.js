// Laser Studio — media.js (T-435).
// Sections (as in the former index.html script):
//   import (T-297): SVG, DXF (T-497) or picture → figure, previewed before it is created
//   CRÉATION › Objets 3D: wireframe objects from OBJ files (T-462)
//   CRÉATION › Médiathèque: ILDA files (T-011)
//   Enregistrer une animation ILDA (T-442)
//   RÉGLAGES › Carte SD ShowNET (T-302)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { FIG_NAME_RE, drawStrokes, fe, figChange, figMarkSaved, figMsg, loadFigLib, showFigProps,
  stopFigPreview } from './figures.js';
import { $, api, apiJson, post, setCheck, setVal } from './core.js';
import { flash, remember, remembered } from './workspaces.js';
import { loadPresets, refreshLook } from './cues.js';
import { TL } from './timeline-editor.js';
import { loadLfos } from './modulators.js';

// ----- import (T-297): SVG, DXF (T-497) or picture → figure, previewed before it is created -----
// The file is sent to /api/figures/import (settings in the query), which
// only answers with a preview; « Créer la figure » saves it like
// « Enregistrer » and opens it in the editor. The file itself is not kept.
const IMP_MAX_BYTES = 20 * 1024 * 1024;
const imp = { bytes: null, name: '', raster: false, dxf: false, result: null, timer: null, seq: 0, dirty: false, inflight: null };
const IMP_LABELS = {
  impSimplify: v => v + ' %', impSize: v => v + ' %', impSmooth: v => v, impColors: v => v,
  impRes: v => v + ' px', impMin: v => v + ' px', impThreshold: v => $('impAuto').checked ? 'auto' : v,
};
function impLabels() { for (const [id, f] of Object.entries(IMP_LABELS)) $(id + 'V').textContent = f($(id).value); }
function impMsg(text, err = false) { $('impMsg').textContent = text; $('impMsg').classList.toggle('err', err); }
function impParams() {
  const p = new URLSearchParams({
    name: imp.name, simplify: $('impSimplify').value, size: $('impSize').value, budget: $('impBudget').value,
    color: $('impColor').value.slice(1), fills: $('impFills').checked ? '1' : '0', layers: $('impLayers').checked ? '1' : '0',
    mode: $('impMode').value,
    invert: $('impInvert').checked ? '1' : '0', smooth: $('impSmooth').value, colors: $('impColors').value,
    resolution: $('impRes').value, min_size: $('impMin').value,
  });
  if (!$('impAuto').checked) p.set('threshold', $('impThreshold').value);
  return p.toString();
}
function impRender() {
  const c = $('impCanvas'), g = c.getContext('2d'), W = c.width;
  g.fillStyle = '#000'; g.fillRect(0, 0, W, W);
  g.strokeStyle = '#ffffff14'; g.lineWidth = 1;
  g.beginPath(); g.moveTo(W / 2, 0); g.lineTo(W / 2, W); g.moveTo(0, W / 2); g.lineTo(W, W / 2); g.stroke();
  if (imp.result) drawStrokes(g, imp.result.figure.frames[0].strokes, W, 1, { travel: true });
}
function impFail(text) {
  imp.result = null; $('impCreate').disabled = true;
  impShowParts(null);
  $('impStats').textContent = ''; $('impStats').classList.remove('bad');
  impMsg(text, true); impRender();
}
function impPreview() {
  clearTimeout(imp.timer);
  imp.dirty = false;
  if (!imp.bytes) return Promise.resolve();
  imp.inflight = impFetch().finally(() => { imp.inflight = null; });
  return imp.inflight;
}
async function impFetch() {
  const seq = ++imp.seq;
  impMsg('Analyse du fichier…');
  let r, body;
  try {
    r = await api('POST', '/api/figures/import?' + impParams(), imp.bytes, { quiet: true });
    body = r.ok ? await r.json() : await r.text();
  } catch { if (seq === imp.seq) impFail('Le studio ne répond pas.'); return; }
  if (seq !== imp.seq) return; // a newer preview is on its way
  if (!r.ok) { impFail('Import impossible : ' + (body || r.status)); return; }
  imp.result = body;
  const s = body.stats;
  if ($('impName').dataset.auto === '1') { $('impName').value = body.figure.name; }
  if (s.threshold != null && $('impAuto').checked) $('impThreshold').value = s.threshold;
  impLabels();
  $('impStats').textContent = `${s.strokes} tracé(s), ${s.points} point(s) ; ≈ ${s.laser_points} points laser par image pour un budget de ${s.budget}`
    + ` ; trajets éteints ${s.travel_before.toFixed(1)} → ${s.travel_after.toFixed(1)} (ordre optimisé)`
    + (s.threshold != null ? ` ; seuil ${s.threshold}` : '') + '.';
  $('impStats').classList.toggle('bad', s.over_budget);
  impShowParts(body.figure);
  impMsg(body.warnings.length ? 'Attention : ' + body.warnings.join(' ; ') + '.' : 'Aperçu prêt. Vérifiez-le puis « Créer la figure ».');
  $('impCreate').disabled = false;
  impRender();
}
// Named SVG groups (T-441) or DXF layers (« Par calque », T-497): one box per group, « Garder comme partie ».
// An unticked group's strokes go to « Aucune » when the figure is created.
function impShowParts(fig) {
  const box = $('impParts'), parts = (fig && fig.parts) || [];
  const kept = new Map([...box.querySelectorAll('input[data-part-name]')].map(c => [c.dataset.partName, c.checked]));
  box.replaceChildren();
  box.hidden = !parts.length;
  if (!parts.length) return;
  const head = document.createElement('div'); head.className = 'hint';
  head.textContent = 'Parties détectées : ' + parts.map(p => p.name).join(', ');
  box.appendChild(head);
  for (const p of parts) {
    const l = document.createElement('label'); l.className = 'check';
    const c = document.createElement('input'); c.type = 'checkbox'; c.dataset.part = p.id; c.dataset.partName = p.name;
    c.checked = kept.get(p.name) ?? true;
    const t = document.createElement('span'); t.textContent = `Garder « ${p.name} » comme partie`;
    l.append(c, t); box.appendChild(l);
  }
}
function impApplyParts(fig) {
  const drop = new Set([...$('impParts').querySelectorAll('input[data-part]')].filter(c => !c.checked).map(c => +c.dataset.part));
  if (!drop.size) return;
  fig.parts = (fig.parts || []).filter(p => !drop.has(p.id));
  fig.frames.forEach(f => f.strokes.forEach(s => { if (drop.has(s.part)) delete s.part; }));
}
const impSoon = () => { clearTimeout(imp.timer); imp.dirty = true; imp.timer = setTimeout(impPreview, 250); };
async function impLoad(file) {
  if (!file) return;
  imp.result = null; imp.bytes = null; $('impCreate').disabled = true;
  $('impFileName').textContent = `${file.name} · ${Math.max(1, Math.round(file.size / 1024))} Ko`;
  if (file.size > IMP_MAX_BYTES) { impFail(`Fichier trop gros : ${IMP_MAX_BYTES / 1024 / 1024} Mo au plus.`); return; }
  let buf;
  try { buf = await file.arrayBuffer(); } catch { impFail('Fichier illisible.'); return; }
  const b = new Uint8Array(buf.slice(0, 4));
  imp.raster = (b[0] === 0x89 && b[1] === 0x50) || (b[0] === 0xff && b[1] === 0xd8);
  // The studio sniffs the content; the name only picks the options shown.
  imp.dxf = !imp.raster && /\.dxf$/i.test(file.name);
  $('impRasterOpts').hidden = !imp.raster;
  $('impSvgOpts').hidden = imp.raster || imp.dxf;
  $('impDxfOpts').hidden = !imp.dxf;
  imp.bytes = buf; imp.name = file.name;
  $('impName').dataset.auto = '1';
  await impPreview();
}
$('impFile').onchange = () => impLoad($('impFile').files[0]);
const drop = $('impDrop');
['dragenter', 'dragover'].forEach(t => drop.addEventListener(t, e => { e.preventDefault(); drop.classList.add('over'); }));
drop.addEventListener('dragleave', () => drop.classList.remove('over'));
drop.addEventListener('drop', e => { e.preventDefault(); drop.classList.remove('over'); impLoad(e.dataTransfer?.files?.[0]); });
Object.keys(IMP_LABELS).forEach(id => $(id).addEventListener('input', () => { impLabels(); impSoon(); }));
['impBudget', 'impColor', 'impFills', 'impLayers', 'impMode', 'impInvert'].forEach(id => $(id).addEventListener('change', impSoon));
// Only the settings of the chosen mode are active.
function impModeOpts() {
  const m = $('impMode').value;
  $('impColors').disabled = m !== 'colors';
  $('impAuto').disabled = $('impInvert').disabled = m === 'colors';
  $('impThreshold').disabled = m === 'colors' || $('impAuto').checked;
}
$('impMode').addEventListener('change', impModeOpts);
$('impAuto').onchange = () => { impModeOpts(); impLabels(); impSoon(); };
$('impName').oninput = () => { $('impName').dataset.auto = '0'; };
function impCloseDlg() { clearTimeout(imp.timer); imp.seq++; $('figImportDlg').close(); }
$('impClose').onclick = impCloseDlg;
$('impCancel').onclick = impCloseDlg;
$('figImport').onclick = () => {
  imp.bytes = null; imp.result = null; imp.name = '';
  $('impFile').value = ''; $('impFileName').textContent = ''; $('impName').value = ''; $('impName').dataset.auto = '1';
  $('impStats').textContent = ''; impMsg(''); $('impCreate').disabled = true;
  $('impParts').replaceChildren(); $('impParts').hidden = true;
  $('impBudget').value = fe.budget;
  impModeOpts();
  impLabels(); impRender();
  $('figImportDlg').showModal();
};
$('impCreate').onclick = async () => {
  // Settings just changed: create what they show, not the previous preview.
  if (imp.dirty) await impPreview(); else if (imp.inflight) await imp.inflight;
  if (!imp.result) return;
  const name = $('impName').value.trim();
  if (!FIG_NAME_RE.test(name)) { impMsg('Nom invalide : lettres, chiffres, espaces, - et _ seulement, 64 caractères au plus.', true); $('impName').focus(); return; }
  if (fe.lib.some(f => f.name === name) && !confirm(`Une figure « ${name} » existe déjà : la remplacer ?`)) return;
  const fig = structuredClone(imp.result.figure);
  fig.name = name;
  impApplyParts(fig);
  const r = await api('POST', '/api/figures', fig, { quiet: true });
  if (!r.ok) { impMsg(await r.text(), true); return; }
  impCloseDlg();
  stopFigPreview();
  figChange(() => { fe.fig = fig; fe.frame = 0; fe.sel.clear(); fe.draft = null; });
  figMarkSaved();
  showFigProps();
  figMsg(`« ${name} » créée depuis l'import (cue de la page Figures) : modifiable ici.`);
  await loadFigLib(); await loadPresets(); loadLfos(); // the parts a modulator may target (T-441)
};
impLabels();

// ---------- CRÉATION › Objets 3D: wireframe objects from OBJ files (T-462) ----------
// The file goes to /api/objects/import with `preview=1` (options in the
// query): the studio reads it on its own thread and answers with the
// object and a report; nothing is saved until « Importer ». The preview
// turns in this page only, never on the laser. Each object is a cue of
// the « Objets 3D » page; playing it never arms the laser.
const ob = { list: [], budget: 750, maxBytes: 8 * 1024 * 1024, maxEdges: 2000, sel: null, bytes: null, name: '', result: null, seq: 0, timer: null, inflight: null, dirty: false, raf: 0 };
const objNum = n => Number(n).toLocaleString('fr-FR');
function objMsg(text, err = false) { $('objMsg').textContent = text; $('objMsg').classList.toggle('err', err); }
function objImpMsg(text, err = false) { $('objImpMsg').textContent = text; $('objImpMsg').classList.toggle('err', err); }
const objHex = c => '#' + (c || [0, 255, 0]).map(v => v.toString(16).padStart(2, '0')).join('');
const objRgb = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16) || 0);
async function loadObjLib() {
  let r;
  try { r = await apiJson('GET', '/api/objects', undefined, { cache: 'no-store' }); } catch { return; }
  ob.list = r.objects; ob.budget = r.budget; ob.maxBytes = r.max_bytes; ob.maxEdges = r.max_edges;
  if (ob.sel && !ob.list.some(o => o.name === ob.sel)) ob.sel = null;
  const box = $('objLib');
  box.replaceChildren();
  if (!ob.list.length) box.innerHTML = '<div class="hint">Aucun objet 3D importé.</div>';
  for (const o of ob.list) {
    const row = document.createElement('div'); row.className = 'objitem'; row.dataset.obj = o.name;
    row.classList.toggle('sel', o.name === ob.sel);
    const n = document.createElement('button'); n.className = 'n'; n.title = `Propriétés de « ${o.name} »`;
    n.textContent = o.name + ' ';
    const meta = document.createElement('small');
    meta.textContent = `· ${objNum(o.edges)} arêtes · ${objNum(o.est_points)} points`;
    n.appendChild(meta);
    n.onclick = () => { ob.sel = o.name; loadObjLib(); };
    const play = document.createElement('button'); play.textContent = 'Jouer';
    play.title = 'Jouer comme un cue (page Objets 3D) ; n\'arme pas le laser';
    play.onclick = () => objPlay(o);
    row.append(n, play); box.appendChild(row);
  }
  objShowProps();
}
function objShowProps() {
  const o = ob.list.find(x => x.name === ob.sel);
  $('objProps').hidden = !o;
  if (!o) return;
  setVal('objName', o.name);
  setVal('objColor', objHex(o.color));
  setVal('objPeriod', o.period_beats);
  setVal('objRatio', o.axis_ratio);
  setVal('objPersp', o.perspective);
  setCheck('objDepth', o.depth_color);
  $('objStats').textContent = `${objNum(o.vertices)} sommets · ${objNum(o.edges)} arêtes · ${objNum(o.strokes)} tracé(s) · `
    + `Points estimés : ${objNum(o.est_points)} / ${objNum(ob.budget)}`;
  $('objStats').classList.toggle('bad', o.est_points > ob.budget);
}
async function objPlay(o) {
  if (await post('/api/presets/play', { id: o.id })) { objMsg(`« ${o.name} » joué (cue de la page Objets 3D).`); refreshLook(); }
  else objMsg(`Impossible de jouer « ${o.name} ».`, true);
}
$('objSave').onclick = async () => {
  const o = ob.list.find(x => x.name === ob.sel);
  if (!o) return;
  const rename = $('objName').value.trim();
  if (!FIG_NAME_RE.test(rename)) { objMsg('Nom invalide : lettres, chiffres, espaces, - et _ seulement, 64 caractères au plus.', true); $('objName').focus(); return; }
  const body = {
    name: o.name, rename, color: objRgb($('objColor').value), period_beats: +$('objPeriod').value,
    axis_ratio: +$('objRatio').value, perspective: +$('objPersp').value, depth_color: $('objDepth').checked,
  };
  const r = await api('POST', '/api/objects', body, { quiet: true });
  if (!r.ok) { objMsg(await r.text(), true); return; }
  ob.sel = rename;
  objMsg(`« ${rename} » enregistré.`);
  await loadObjLib(); await loadPresets();
};
$('objPlay').onclick = () => { const o = ob.list.find(x => x.name === ob.sel); if (o) objPlay(o); };
$('objDelete').onclick = async () => {
  const name = ob.sel;
  if (!name || !confirm(`Supprimer l'objet « ${name} » ? (Annuler avec Cmd+Z)`)) return;
  if (await post('/api/objects/delete', { name })) { ob.sel = null; objMsg(`« ${name} » supprimé.`); await loadObjLib(); await loadPresets(); }
};
// The import dialog.
function objParams() {
  const p = new URLSearchParams({ name: ob.name, up: $('objUp').value, coplanar: $('objCoplanar').checked ? $('objAngle').value : '0' });
  const keep = Math.trunc(+$('objKeep').value);
  if (keep >= 1) p.set('keep', keep);
  return p;
}
function objDraw() {
  const c = $('objCanvas'), g = c.getContext('2d'), W = c.width;
  g.fillStyle = '#000'; g.fillRect(0, 0, W, W);
  const o = ob.result && ob.result.object;
  if (o) {
    // The engine's projection (wire.rs): one turn every 4 s here.
    const t = performance.now() / 4000, yaw = 2 * Math.PI * (t % 1), pitch = 0.45 + 2 * Math.PI * ((t * o.axis_ratio) % 1);
    const [sy, cy, sp, cp] = [Math.sin(yaw), Math.cos(yaw), Math.sin(pitch), Math.cos(pitch)], cam = o.perspective, s = 0.8 * W / 2;
    const pt = v => {
      const x = v[0] * cy + v[2] * sy, z0 = -v[0] * sy + v[2] * cy;
      const y = v[1] * cp - z0 * sp, z = v[1] * sp + z0 * cp, k = (cam - 1) / (cam - Math.min(z, 1));
      return [W / 2 + x * k * s, W / 2 - y * k * s];
    };
    g.strokeStyle = objHex(o.color); g.lineWidth = 2; g.beginPath();
    let end = -1;
    for (const [a, b] of o.edges) {
      const pa = pt(o.vertices[a]), pb = pt(o.vertices[b]);
      if (a !== end) g.moveTo(pa[0], pa[1]);
      g.lineTo(pb[0], pb[1]); end = b;
    }
    g.stroke();
  }
  ob.raf = $('objImportDlg').open && o ? requestAnimationFrame(objDraw) : 0;
}
function objFail(text) {
  ob.result = null; $('objImpOk').disabled = true;
  $('objReport').textContent = ''; $('objReport').classList.remove('bad');
  objImpMsg(text, true); objDraw();
}
function objPreview() {
  clearTimeout(ob.timer);
  ob.dirty = false;
  if (!ob.bytes) return Promise.resolve();
  ob.inflight = objFetch().finally(() => { ob.inflight = null; });
  return ob.inflight;
}
async function objFetch() {
  const seq = ++ob.seq;
  objImpMsg('Lecture du fichier…');
  const p = objParams(); p.set('preview', '1');
  let r, body;
  try {
    r = await api('POST', '/api/objects/import?' + p, ob.bytes, { quiet: true });
    body = r.ok ? await r.json() : await r.text();
  } catch { if (seq === ob.seq) objFail('Le studio ne répond pas.'); return; }
  if (seq !== ob.seq) return; // a newer preview is on its way
  if (!r.ok) { objFail(body || `erreur ${r.status}`); return; }
  ob.result = body;
  const rep = body.report, over = rep.est_points > body.budget;
  if ($('objImpName').dataset.auto === '1') $('objImpName').value = body.object.name;
  $('objReport').textContent = `Sommets : ${objNum(rep.vertices)} · Faces : ${objNum(rep.faces + rep.lines)} · Arêtes : ${objNum(rep.edges)} · `
    + `Points estimés : ${objNum(rep.est_points)} / ${objNum(body.budget)}`
    + (rep.coplanar ? ` · ${objNum(rep.coplanar)} arête(s) entre faces coplanaires masquée(s)` : '')
    + (rep.ignored ? ` · ${objNum(rep.ignored)} ligne(s) ignorée(s) (sommet absent ou illisible)` : '');
  $('objReport').classList.toggle('bad', over);
  objImpMsg(over ? 'Au-delà du budget de points : l\'objet scintillera. Augmentez le seuil ou gardez moins d\'arêtes.'
    : 'Aperçu prêt. Vérifiez-le puis « Importer ».');
  $('objImpOk').disabled = false;
  if (!ob.raf) objDraw();
}
const objSoon = () => { clearTimeout(ob.timer); ob.dirty = true; ob.timer = setTimeout(objPreview, 250); };
async function objLoad(file) {
  if (!file) return;
  ob.result = null; ob.bytes = null; $('objImpOk').disabled = true;
  $('objFileName').textContent = `${file.name} · ${kib(file.size)}`;
  if (file.size > ob.maxBytes) { objFail(`Fichier OBJ trop gros : ${ob.maxBytes / 1024 / 1024} Mo au plus. Exportez l'objet avec moins de détails.`); return; }
  try { ob.bytes = await file.arrayBuffer(); } catch { objFail('Fichier illisible.'); return; }
  ob.name = file.name;
  $('objImpName').dataset.auto = '1';
  await objPreview();
}
$('objFile').onchange = () => objLoad($('objFile').files[0]);
{
  const drop = $('objDrop');
  ['dragenter', 'dragover'].forEach(t => drop.addEventListener(t, e => { e.preventDefault(); drop.classList.add('over'); }));
  drop.addEventListener('dragleave', () => drop.classList.remove('over'));
  drop.addEventListener('drop', e => { e.preventDefault(); drop.classList.remove('over'); objLoad(e.dataTransfer?.files?.[0]); });
}
['objCoplanar', 'objUp'].forEach(id => $(id).addEventListener('change', () => { $('objAngle').disabled = !$('objCoplanar').checked; objSoon(); }));
['objAngle', 'objKeep'].forEach(id => $(id).addEventListener('input', objSoon));
$('objImpName').oninput = () => { $('objImpName').dataset.auto = '0'; };
function objCloseDlg() { clearTimeout(ob.timer); ob.seq++; $('objImportDlg').close(); }
$('objImportDlg').addEventListener('close', () => { cancelAnimationFrame(ob.raf); ob.raf = 0; });
$('objImpClose').onclick = objCloseDlg;
$('objImpCancel').onclick = objCloseDlg;
$('objImport').onclick = () => {
  ob.bytes = null; ob.result = null; ob.name = '';
  $('objFile').value = ''; $('objFileName').textContent = ''; $('objImpName').value = ''; $('objImpName').dataset.auto = '1';
  $('objReport').textContent = ''; objImpMsg(''); $('objImpOk').disabled = true;
  $('objAngle').disabled = !$('objCoplanar').checked;
  $('objImportDlg').showModal();
  objDraw();
};
$('objImpOk').onclick = async () => {
  // Settings just changed: import what they show, not the previous preview.
  if (ob.dirty) await objPreview(); else if (ob.inflight) await ob.inflight;
  if (!ob.result) return;
  const name = $('objImpName').value.trim();
  if (!FIG_NAME_RE.test(name)) { objImpMsg('Nom invalide : lettres, chiffres, espaces, - et _ seulement, 64 caractères au plus.', true); $('objImpName').focus(); return; }
  const p = objParams(); p.set('name', name);
  const r = await api('POST', '/api/objects/import?' + p, ob.bytes, { quiet: true });
  if (!r.ok) { objImpMsg(await r.text(), true); return; }
  const body = await r.json();
  objCloseDlg();
  ob.sel = body.name;
  objMsg(`« ${body.name} » importé : ${objNum(body.report.edges)} arêtes (cue de la page Objets 3D).`);
  await loadObjLib(); await loadPresets();
};

// ---------- CRÉATION › Médiathèque: ILDA files (T-011) ----------
// The user's own .ild files: imported (parsed by the studio first, a
// damaged file is refused), listed, played as cues of the « ILDA » page
// through the normal look path, deleted. And the look on show exported as
// ILDA format 5 for the ShowNET SD card. Nothing here arms the laser.
const ilda = { files: [], maxBytes: 64 * 1024 * 1024, timer: null };
function ildaMsg(text, err = false) { $('ildaMsg').textContent = text; $('ildaMsg').classList.toggle('err', err); }
function ildaExpMsg(text, err = false) { $('ildaExpMsg').textContent = text; $('ildaExpMsg').classList.toggle('err', err); }
const kib = n => n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} Ko` : `${(n / 1024 / 1024).toFixed(1)} Mo`;
async function loadIldaLib() {
  let r;
  try { r = await apiJson('GET', '/api/media/ilda', undefined, { cache: 'no-store' }); } catch { return; }
  ilda.files = r.files; ilda.maxBytes = r.max_bytes;
  clearTimeout(ilda.timer);
  // Read on its own thread at start-up: look again until it is done.
  if (r.loading) ilda.timer = setTimeout(loadIldaLib, 1000);
  const box = $('ildaLib');
  box.innerHTML = '';
  if (!r.files.length) {
    box.innerHTML = `<div class="hint">${r.loading ? 'Chargement de la médiathèque…' : 'Aucun fichier ILDA importé.'}</div>`;
    return;
  }
  for (const f of r.files) {
    const row = document.createElement('div'); row.className = 'ildaitem'; row.dataset.file = f.file;
    const n = document.createElement('span'); n.className = 'n';
    n.textContent = f.file + ' ';
    const meta = document.createElement('small');
    meta.textContent = `· ${f.frames} image${f.frames > 1 ? 's' : ''} · ${f.points} points · ${kib(f.size)}`;
    n.appendChild(meta); n.title = f.file;
    const play = document.createElement('button'); play.className = 'play'; play.textContent = 'Jouer';
    play.title = 'Jouer comme un cue (page ILDA) ; n\'arme pas le laser';
    play.onclick = async () => {
      if (await post('/api/presets/play', { id: f.id })) { ildaMsg(`« ${f.file} » joué (cue de la page ILDA).`); refreshLook(); }
      else ildaMsg(`Impossible de jouer « ${f.file} ».`, true);
    };
    const del = document.createElement('button'); del.className = 'del'; del.textContent = '×';
    del.title = `Supprimer « ${f.file} » de la médiathèque`;
    del.onclick = async () => {
      if (!confirm(`Supprimer le fichier « ${f.file} » de la médiathèque ?`)) return;
      if (await post('/api/media/ilda/delete', { name: f.file })) { ildaMsg(`« ${f.file} » supprimé.`); await loadIldaLib(); await loadPresets(); }
    };
    row.append(n, play, del); box.appendChild(row);
  }
}
async function ildaImport(file) {
  if (!file) return;
  if (!/\.ild$/i.test(file.name)) { ildaMsg('Fichier .ild attendu.', true); return; }
  if (file.size > ilda.maxBytes) { ildaMsg(`Fichier trop gros : ${ilda.maxBytes / 1024 / 1024} Mo au plus.`, true); return; }
  ildaMsg(`Lecture de « ${file.name} »…`);
  let r, body;
  try {
    r = await api('POST', '/api/media/ilda?name=' + encodeURIComponent(file.name), file, { quiet: true });
    body = r.ok ? await r.json() : await r.text();
  } catch { ildaMsg('Le studio ne répond pas.', true); return; }
  if (!r.ok) { ildaMsg('Import impossible : ' + (body || r.status), true); return; }
  ildaMsg(body.existing ? `« ${body.file} » est déjà dans la médiathèque.`
    : `« ${body.file} » importé : ${body.frames} image${body.frames > 1 ? 's' : ''}, ${body.points} points (cue de la page ILDA).`);
  $('ildaFile').value = '';
  await loadIldaLib(); await loadPresets();
}
$('ildaFile').onchange = () => ildaImport($('ildaFile').files[0]);
{
  const drop = $('ildaDrop');
  ['dragenter', 'dragover'].forEach(t => drop.addEventListener(t, e => { e.preventDefault(); drop.classList.add('over'); }));
  drop.addEventListener('dragleave', () => drop.classList.remove('over'));
  drop.addEventListener('drop', e => { e.preventDefault(); drop.classList.remove('over'); ildaImport(e.dataTransfer?.files?.[0]); });
}
const ildaSdName = () => String(Math.trunc(+$('ildaNum').value) || 0).padStart(3, '0') + '.ild';
function showIldaSdName() { $('ildaExport').textContent = `Exporter ${ildaSdName()}`; }
$('ildaNum').addEventListener('input', showIldaSdName);
$('ildaExport').onclick = async () => {
  const p = new URLSearchParams({ number: $('ildaNum').value, frames: $('ildaFrames').value, fps: $('ildaExpFps').value, output: $('ildaExpOut').value || 'main' });
  ildaExpMsg('Export en cours…');
  let r;
  try { r = await api('GET', '/api/media/ilda/export?' + p, undefined, { cache: 'no-store', quiet: true }); } catch { ildaExpMsg('Le studio ne répond pas.', true); return; }
  if (!r.ok) { ildaExpMsg('Export impossible : ' + await r.text(), true); return; }
  const blob = await r.blob(), name = ildaSdName();
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob); a.download = name;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 10000);
  ildaExpMsg(`${name} exporté (${kib(blob.size)}), sécurité de la sortie « ${p.get('output')} » incluse. À copier sur la carte SD du ShowNET.`
    + (blob.size > 6 * 1024 * 1024 ? ' Plus de 6 Mo : passez par un lecteur de carte.' : ''));
};
showIldaSdName();

// ---------- Enregistrer une animation ILDA (T-442) ----------
// What plays live (n seconds, sampled at a fixed rate) or a whole show
// (rendered offline, the same file every time), written into the ILDA
// library through the output safety chain of the chosen output. The
// server does the work on its own thread; the dialog follows its state.
// Nothing here arms the laser; Space and Escape keep their roles.
const rec = { timer: null, show: null, info: null, status: null, wasBusy: false };
function recMsg(text, err = false) { $('recMsg').textContent = text; $('recMsg').classList.toggle('err', err); }
const recMo = n => n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} Ko` : `${(n / 1024 / 1024).toFixed(1).replace('.', ',')} Mo`;
async function openRecord(show) {
  rec.show = show || null;
  const sel = $('recSource');
  sel.replaceChildren(new Option('Ce qui joue en direct', 'live'));
  if (rec.show) sel.add(new Option(`Timeline : ${rec.show}`, 'timeline'));
  sel.value = rec.show ? 'timeline' : 'live';
  recMsg('');
  $('recDialog').hidden = false;
  await recRefresh();
  recSourceChanged();
}
function recSourceChanged() {
  const tl = $('recSource').value === 'timeline', sh = rec.info && rec.info.show;
  $('recSeconds').disabled = tl;
  if (tl && sh) $('recSeconds').value = (+sh.seconds).toFixed(1);
  $('recAudio').hidden = !(tl && sh && sh.audio);
}
$('recSource').onchange = recSourceChanged;
async function recRefresh() {
  let r;
  try { r = await apiJson('GET', '/api/ilda/record' + (rec.show ? '?show=' + encodeURIComponent(rec.show) : ''), undefined, { cache: 'no-store', quiet: true }); }
  catch { return; }
  rec.info = r;
  recShow(r.status);
}
function recShow(st) {
  const busy = st.state === 'recording' || st.state === 'writing';
  $('recStart').disabled = busy;
  $('recCancel').hidden = !busy;
  $('recDownload').hidden = st.state !== 'done';
  if (st.state === 'recording') recMsg(`Enregistrement… ${st.frames} / ${st.total} images`);
  else if (st.state === 'writing') recMsg(`Écriture du fichier… (${st.total} images)`);
  else if (st.state === 'done') recMsg(`Fichier prêt : ${st.file} (${st.frames} images, ${recMo(st.bytes)}), sécurité de la sortie « ${st.output} » incluse.`
    + (st.warnings.length ? ' ' + st.warnings.join(' · ') + '.' : ''));
  else if (st.state === 'failed') recMsg('Enregistrement impossible : ' + (st.error || ''), true);
  else if (st.state === 'cancelled') recMsg('Enregistrement annulé : aucun fichier écrit.');
  clearTimeout(rec.timer);
  if (busy) rec.timer = setTimeout(recRefresh, 250);
  // A new file in the library: its list and the « ILDA » cue page.
  if (st.state === 'done' && rec.wasBusy) { loadIldaLib(); loadPresets(); }
  rec.wasBusy = busy;
  rec.status = st;
}
$('recStart').onclick = async () => {
  const tl = $('recSource').value === 'timeline';
  const body = {
    source: tl ? { kind: 'timeline', show: rec.show } : { kind: 'live' },
    seconds: +$('recSeconds').value,
    fps: Math.max(0, Math.min(255, Math.trunc(+$('recFps').value) || 0)),
    name: $('recName').value.trim(),
    output: $('recOut').value || 'main',
  };
  const sd = $('recSd').value.trim();
  if (sd) body.sd_number = Math.max(0, Math.trunc(+sd) || 0);
  let r;
  try { r = await api('POST', '/api/ilda/record', body, { quiet: true }); } catch { recMsg('Le studio ne répond pas.', true); return; }
  if (!r.ok) {
    const t = await r.text();
    if (r.status === 423) flash(t);
    recMsg(r.status === 409 ? 'Un enregistrement est déjà en cours.' : t, true);
    return;
  }
  rec.wasBusy = true;
  recShow(await r.json());
};
$('recCancel').onclick = async () => { await post('/api/ilda/record/cancel'); await recRefresh(); };
$('recDownload').onclick = async () => {
  const f = rec.status && rec.status.file;
  if (!f) return;
  let r;
  try { r = await api('GET', '/api/ilda/record/file?name=' + encodeURIComponent(f), undefined, { cache: 'no-store', quiet: true }); } catch { recMsg('Le studio ne répond pas.', true); return; }
  if (!r.ok) { recMsg('Téléchargement impossible : ' + await r.text(), true); return; }
  const blob = await r.blob();
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob); a.download = f;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 10000);
};
function closeRecord() { $('recDialog').hidden = true; }
$('recClose').onclick = closeRecord;
$('recDialog').addEventListener('click', e => { if (e.target === $('recDialog')) closeRecord(); });
$('ildaRecord').onclick = () => openRecord(null);
$('tlRecord').onclick = () => openRecord(TL.show && TL.show.name);

// ---------- RÉGLAGES › Carte SD ShowNET (T-302) ----------
// Which cue the ShowNET plays for which number (001-229), and the export
// of the whole bank into a folder ready to copy to the card. Every file
// carries the safety chain of the chosen output (T-355). Nothing here
// arms the laser or talks to the ShowNET.
const SD_DIR_KEY = 'laserStudio.sdDir';
let sdBank = { slots: [] };
function sdMsg(text, err = false) { $('sdMsg').textContent = text; $('sdMsg').classList.toggle('err', err); }
async function sdRequest(path, body) {
  let r;
  try { r = await api('POST', path, body ?? {}, { quiet: true }); }
  catch { sdMsg('Le studio ne répond pas.', true); return null; }
  if (!r.ok) {
    const t = await r.text();
    if (r.status === 423) flash(t);
    sdMsg(t, true);
    return null;
  }
  return r.json();
}
// Points per frame baked into the files (T-476): the output's own range
// (RÉGLAGES › Sorties › balayage), applied before the safety chain.
function sdPointsText(o) {
  const min = o.min_points || 0, max = o.max_points;
  if (max) return min ? `${min} à ${max} points par image` : `${max} points par image au plus`;
  return min ? `${min} points par image au moins` : 'points par image sans limite';
}
function fillOutputs(sel, outputs) {
  const keep = sel.value;
  sel.innerHTML = '';
  for (const o of outputs) {
    const opt = document.createElement('option');
    opt.value = o.id;
    opt.textContent = `${o.name ? o.name + ' (' + o.id + ')' : o.id} · puissance max ${Math.round(o.max_power * 100)} % · ${sdPointsText(o)}`;
    sel.appendChild(opt);
  }
  if (outputs.some(o => o.id === keep)) sel.value = keep;
}
function showSdBank(bank) {
  sdBank = bank;
  $('sdFps').textContent = bank.fps;
  document.querySelectorAll('.sd-output').forEach(sel => fillOutputs(sel, bank.outputs));
  const rows = $('sdRows');
  rows.innerHTML = '';
  $('sdEmpty').hidden = bank.slots.length > 0;
  bank.slots.forEach((s, i) => {
    const tr = document.createElement('tr'); tr.dataset.number = s.number;
    const num = document.createElement('input');
    Object.assign(num, { type: 'number', min: bank.first, max: bank.last, step: 1, value: s.number, title: `Fichier ${s.file}` });
    num.className = 'sd-num';
    num.onchange = () => saveSdBank(i, { number: Math.trunc(+num.value) });
    const cue = document.createElement('td'); cue.className = 'cue' + (s.name == null ? ' missing' : '');
    cue.textContent = s.name ?? `${s.cue_id} (introuvable)`; cue.title = s.cue_id;
    const secs = document.createElement('input');
    Object.assign(secs, { type: 'number', min: bank.min_seconds, max: bank.max_seconds, step: 0.1, value: s.seconds });
    secs.className = 'sd-secs';
    secs.onchange = () => saveSdBank(i, { seconds: +secs.value });
    const del = document.createElement('button'); del.textContent = '×'; del.title = `Retirer ${s.file} de la banque`;
    del.className = 'sd-del';
    del.onclick = () => saveSdBank(i, null);
    const cells = [num, cue, secs, del].map(el => {
      if (el.tagName === 'TD') return el;
      const td = document.createElement('td'); td.appendChild(el); return td;
    });
    tr.append(...cells);
    rows.appendChild(tr);
  });
}
async function loadSdBank() {
  try { showSdBank(await apiJson('GET', '/api/sdbank', undefined, { cache: 'no-store' })); } catch { /* studio stopped: kept as is */ }
}
// One row changed (or removed: `change` null): the whole bank is sent and
// checked by the studio; a refused change is undone on screen.
async function saveSdBank(i, change) {
  const slots = sdBank.slots.map(({ number, cue_id, seconds }) => ({ number, cue_id, seconds }));
  if (change) Object.assign(slots[i], change); else slots.splice(i, 1);
  const r = await sdRequest('/api/sdbank', { slots });
  if (r) { showSdBank(r); sdMsg(change ? 'Banque enregistrée.' : 'Retiré de la banque.'); } else await loadSdBank();
}
$('sdAdd').onclick = async () => {
  const r = await sdRequest('/api/sdbank/add', {});
  if (r) { showSdBank(r); sdMsg(`Cue ajouté en ${String(r.added).padStart(3, '0')}.ild.`); }
};
$('sdRenumber').onclick = async () => {
  const r = await sdRequest('/api/sdbank/renumber');
  if (r) { showSdBank(r); sdMsg('Banque renumérotée à partir de 001.'); }
};
$('sdDir').value = remembered(SD_DIR_KEY) || '';
$('sdExport').onclick = async () => {
  const dir = $('sdDir').value.trim();
  if (!dir) { sdMsg('Indiquez le dossier d\'export (par exemple ~/Desktop/carte-shownet).', true); $('sdDir').focus(); return; }
  remember(SD_DIR_KEY, dir);
  $('sdWarn').innerHTML = '';
  sdMsg('Export en cours… (rendu de chaque cue)');
  $('sdExport').disabled = true;
  const r = await sdRequest('/api/sdbank/export', { dir, output: $('sdOutput').value || 'main' });
  $('sdExport').disabled = false;
  if (!r) return;
  const total = r.files.reduce((n, f) => n + f.bytes, 0);
  sdMsg(`${r.files.length} fichier${r.files.length > 1 ? 's' : ''} exporté${r.files.length > 1 ? 's' : ''} dans ${r.dir} (${kib(total)}), `
    + `sécurité de la sortie « ${r.output} » incluse. Copiez-les à la racine de la carte microSD.`);
  for (const w of r.warnings) { const li = document.createElement('li'); li.textContent = w; $('sdWarn').appendChild(li); }
};
loadSdBank();

export { loadIldaLib, loadObjLib, loadSdBank };
