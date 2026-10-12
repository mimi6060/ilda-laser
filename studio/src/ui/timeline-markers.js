// Laser Studio — timeline-markers.js (T-435).
// Sections (as in the former index.html script):
//   markers
//   selection, copy / paste / duplicate / delete
//   Modèles de phrases (T-165)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { TL, TL_LAYER_COLORS, TL_MARK_COLORS, TL_MIN_LEN, tlBarAt, tlChange, tlEvents, tlHeads,
  tlIgnoredMsg, tlLockedTi, tlMsg, tlNextId, tlPlayhead, tlRedo, tlUAtBar, tlUndo } from './timeline-editor.js';
import { $, api, apiJson, hex, rgb } from './core.js';
import { loadProjection, proj } from './zones.js';
import { poolRev, presets, tagKey } from './cues.js';
import { fillPoolSelect, fillScanSelect, scanRev } from './presets.js';
import { spreadGroups, spreadRender, spreadWire } from './modulators.js';
import { dmxOutNames } from './outputs.js';
import { esc } from './midi.js';

// --- markers ---
function tlAddMarker(at) {
  if (!TL.show) return;
  tlChange(() => {
    const n = TL.show.markers.length;
    TL.show.markers.push({ at: Math.max(0, at), name: `Marqueur ${n + 1}`, color: TL_MARK_COLORS[n % TL_MARK_COLORS.length] });
    TL.show.markers.sort((a, b) => a.at - b.at);
    TL.marker = TL.show.markers.findIndex(m => m.at === Math.max(0, at));
  });
  tlMarkRow();
}
$('tlAddMarker').onclick = () => tlAddMarker(tlPlayhead());

// --- markers as CSV (T-443): export, and import read by the server
// (`markers/parse`, which changes nothing) then applied here through
// tlChange, so one Cmd+Z undoes the whole import. ---
const TL_CSV_MAX = 1 << 20, TL_MAX_MARKERS = 500;
const tlCsv = { parsed: null };
function tlCsvMenu(open) { $('tlCsvMenu').hidden = !open; }
$('tlCsvBtn').onclick = e => { e.stopPropagation(); tlCsvMenu($('tlCsvMenu').hidden); };
document.addEventListener('click', e => { if (!e.target.closest('.tl-csv')) tlCsvMenu(false); });
$('tlCsvExport').onclick = async () => {
  tlCsvMenu(false);
  if (!TL.show) return;
  let r;
  try { r = await api('GET', '/api/timeline/markers.csv', undefined, { cache: 'no-store', quiet: true }); } catch { tlMsg('Studio injoignable : export impossible', true); return; }
  if (!r.ok) { tlMsg('Export impossible : ' + await r.text(), true); return; }
  const blob = await r.blob(), name = `${TL.show.name || 'show'}-marqueurs.csv`;
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob); a.download = name;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 10000);
  const n = TL.show.markers.length;
  tlMsg(`${n} marqueur${n > 1 ? 's' : ''} exporté${n > 1 ? 's' : ''} dans ${name}`);
};
$('tlCsvImport').onclick = () => { tlCsvMenu(false); if (TL.show) { $('tlCsvFile').value = ''; $('tlCsvFile').click(); } };
$('tlCsvFile').onchange = () => tlCsvRead($('tlCsvFile').files[0]);
async function tlCsvRead(file) {
  if (!file || !TL.show) return;
  if (file.size > TL_CSV_MAX) { tlMsg(`Fichier trop gros : ${TL_CSV_MAX >> 20} Mo au plus.`, true); return; }
  let r, body;
  try {
    r = await api('POST', '/api/timeline/markers/parse', file, { quiet: true });
    body = r.ok ? await r.json() : await r.text();
  } catch { tlMsg('Studio injoignable : fichier non lu', true); return; }
  if (!r.ok) { tlMsg('Import impossible : ' + (body || r.status), true); return; }
  tlCsv.parsed = body;
  const n = body.markers.length, k = body.skipped_count || 0;
  // Text only: names and reasons come from the file.
  $('tlCsvSummary').textContent = `« ${file.name} » : ${n} marqueur${n > 1 ? 's' : ''} lu${n > 1 ? 's' : ''}`
    + (k ? ` · ${k} ligne${k > 1 ? 's' : ''} ignorée${k > 1 ? 's' : ''}` : '') + (n ? '' : ' : rien à importer.');
  const list = $('tlCsvSkippedList');
  list.replaceChildren(...body.skipped.map(s => { const li = document.createElement('li'); li.textContent = `ligne ${s.line} : ${s.reason}`; return li; }));
  if (k > body.skipped.length) { const li = document.createElement('li'); li.textContent = `… et ${k - body.skipped.length} autres`; list.append(li); }
  $('tlCsvSkipped').hidden = !k;
  $('tlCsvSkipped').open = false;
  $('tlCsvReplace').disabled = $('tlCsvAppend').disabled = !n;
  $('tlCsvDlg').showModal();
}
function tlCsvApply(mode) {
  const parsed = tlCsv.parsed;
  $('tlCsvDlg').close();
  tlCsv.parsed = null;
  if (!parsed || !TL.show) return;
  // Under a millisecond apart (in the show's unit) with the same name: a duplicate.
  const bpm = (TL.show.time_base === 'beats' && TL.show.tempo_map && TL.show.tempo_map[0] && TL.show.tempo_map[0].bpm) || 120;
  const tol = TL.show.time_base === 'beats' ? 0.001 * bpm / 60 : 0.001;
  let added = 0, dup = 0, over = 0;
  tlChange(() => {
    const keep = mode === 'replace' ? [] : TL.show.markers.slice();
    for (const m of parsed.markers) {
      if (keep.some(o => o.name === m.name && Math.abs(o.at - m.at) < tol)) { dup++; continue; }
      if (keep.length >= TL_MAX_MARKERS) { over++; continue; }
      const black = !m.color || m.color.every(c => c === 0);
      keep.push({ at: m.at, name: m.name, color: black ? TL_MARK_COLORS[keep.length % TL_MARK_COLORS.length] : m.color });
      added++;
    }
    keep.sort((a, b) => a.at - b.at);
    TL.show.markers = keep;
  });
  TL.marker = -1; tlMarkRow();
  const extra = [dup && `${dup} déjà présent${dup > 1 ? 's' : ''}`, over && `${over} au-delà de ${TL_MAX_MARKERS}`].filter(Boolean);
  tlMsg(`${added} marqueur${added > 1 ? 's' : ''} importé${added > 1 ? 's' : ''}` + (extra.length ? ` (${extra.join(', ')} ignoré${dup + over > 1 ? 's' : ''})` : '')
    + (added || mode === 'replace' ? ' (Cmd+Z pour annuler)' : ''));
}
$('tlCsvReplace').onclick = () => tlCsvApply('replace');
$('tlCsvAppend').onclick = () => tlCsvApply('append');
$('tlCsvCancel').onclick = () => { tlCsv.parsed = null; $('tlCsvDlg').close(); };
function tlMarkRow() {
  const m = TL.show && TL.show.markers[TL.marker];
  $('tlMarkRow').hidden = !m;
  if (!m) return;
  if (document.activeElement !== $('tlMarkName')) $('tlMarkName').value = m.name;
  $('tlMarkColor').value = hex(m.color);
}
$('tlMarkName').addEventListener('change', () => { const i = TL.marker; if (i < 0) return; tlChange(() => { TL.show.markers[i].name = $('tlMarkName').value.slice(0, 64); }); });
$('tlMarkColor').addEventListener('change', () => { const i = TL.marker; if (i < 0) return; tlChange(() => { TL.show.markers[i].color = rgb($('tlMarkColor').value); }); });
$('tlMarkDel').onclick = () => { const i = TL.marker; if (i < 0) return; tlChange(() => { TL.show.markers.splice(i, 1); }); TL.marker = -1; tlMarkRow(); };

// « Balayage » of the selected events (T-415): « Comme le cue » = none.
let tlScanKey = '';
function assignTlScanKey(v) { return (tlScanKey = v); }
function tlScanRow() {
  const items = TL.show ? tlSelected().filter(x => x.e.source.kind !== 'dmx') : [];
  const key = items.map(x => x.e.id + ':' + (x.e.scan || '')).join(',') + '|' + scanRev;
  if (key === tlScanKey) return;
  tlScanKey = key;
  $('tlScanRow').hidden = !items.length;
  if (!items.length || document.activeElement === $('tlScan')) return;
  const vals = [...new Set(items.map(x => x.e.scan || ''))];
  fillScanSelect($('tlScan'), 'Comme le cue', vals.length === 1 ? vals[0] : '');
}
$('tlScan').addEventListener('change', () => {
  const v = $('tlScan').value;
  const items = tlSelected(), free = items.filter(x => !x.t.locked);
  if (!free.length) { tlMsg('Piste verrouillée', true); tlScanKey = ''; return; }
  tlChange(() => { for (const x of free) { if (v) x.e.scan = v; else delete x.e.scan; } });
  if (free.length < items.length) tlMsg(tlIgnoredMsg(items.length - free.length));
});

// Referenced presets of the selected events (T-422): « Valeurs de
// l'événement » = none; a mixed selection shows that choice.
let tlRefKey = '';
function assignTlRefKey(v) { return (tlRefKey = v); }
const TL_REFS = ['tlRefPosition', 'tlRefSize', 'tlRefColor', 'tlRefMotion'];
function tlRefRow() {
  const items = TL.show ? tlSelected().filter(x => x.e.source.kind !== 'dmx') : [];
  const key = items.map(x => x.e.id + ':' + JSON.stringify(x.e.refs || {})).join(',') + '|' + poolRev;
  if (key === tlRefKey) return;
  tlRefKey = key;
  $('tlRefRow').style.visibility = items.length ? 'visible' : 'hidden';
  if (!items.length) return;
  for (const id of TL_REFS) {
    const sel = $(id);
    if (document.activeElement === sel) continue;
    const vals = [...new Set(items.map(x => x.e.refs?.[sel.dataset.family] || ''))];
    fillPoolSelect(sel, 'Valeurs de l\'événement', vals.length === 1 ? vals[0] : '');
  }
}
TL_REFS.forEach(id => $(id).addEventListener('change', () => {
  const sel = $(id), fam = sel.dataset.family, v = sel.value;
  const items = tlSelected(), free = items.filter(x => !x.t.locked);
  if (!free.length) { tlMsg('Piste verrouillée', true); tlRefKey = ''; return; }
  tlChange(() => {
    for (const x of free) {
      const refs = { ...(x.e.refs || {}) };
      if (v) refs[fam] = v; else delete refs[fam];
      if (Object.keys(refs).length) x.e.refs = refs; else delete x.e.refs;
    }
  });
  if (free.length < items.length) tlMsg(tlIgnoredMsg(items.length - free.length));
}));

// Spread of the selected event across a zone group (T-421): one event at
// a time; its own wins over its cue's cell.
let tlSpreadKey = '';
function tlSpreadRow() {
  const items = TL.show ? tlSelected().filter(x => x.e.source.kind !== 'dmx') : [];
  const e = items.length === 1 ? items[0].e : null;
  const key = e ? e.id + ':' + JSON.stringify(e.spread || null) + ':' + spreadGroups().map(g => g.name + g.zones.length).join(',') : '';
  if (key === tlSpreadKey || $('tlEvSpread').contains(document.activeElement)) return;
  tlSpreadKey = key;
  $('tlSpreadRow').hidden = !e;
  if (!e) return;
  delete $('tlEvSpread').dataset.seed;
  spreadRender($('tlEvSpread'), e.spread);
}
spreadWire($('tlEvSpread'), sp => {
  const items = tlSelected(), free = items.filter(x => !x.t.locked);
  if (items.length !== 1) return;
  if (!free.length) { tlMsg('Piste verrouillée', true); tlSpreadKey = ''; return; }
  tlChange(() => { if (sp) free[0].e.spread = sp; else delete free[0].e.spread; });
});
$('tlSpreadDet').addEventListener('toggle', () => { if ($('tlSpreadDet').open && !proj) loadProjection().then(() => { tlSpreadKey = ''; }); });

// Notes and tags of the selected event (T-430): one event at a time.
let tlNoteKey = '';
function assignTlNoteKey(v) { return (tlNoteKey = v); }
function tlNoteRow() {
  const items = TL.show ? tlSelected() : [];
  const e = items.length === 1 ? items[0].e : null;
  const key = e ? e.id + ':' + (e.notes || '') + ':' + (e.tags || []).join(',') : '';
  if (key === tlNoteKey) return;
  tlNoteKey = key;
  $('tlNoteRow').hidden = !e;
  if (!e) return;
  if (document.activeElement !== $('tlEvNotes')) $('tlEvNotes').value = e.notes || '';
  if (document.activeElement !== $('tlEvTags')) $('tlEvTags').value = (e.tags || []).join(', ');
}
$('tlEvNotes').addEventListener('change', () => {
  const items = tlSelected();
  if (items.length !== 1) return;
  const v = $('tlEvNotes').value.trim().slice(0, 500);
  tlChange(() => { if (v) items[0].e.notes = v; else delete items[0].e.notes; });
});
$('tlEvTags').addEventListener('change', () => {
  const items = tlSelected();
  if (items.length !== 1) return;
  const tags = [];
  for (const raw of $('tlEvTags').value.split(',')) {
    const t = raw.trim().slice(0, 24).trim();
    if (t && tags.length < 8 && !tags.some(x => tagKey(x) === tagKey(t))) tags.push(t);
  }
  tlChange(() => { if (tags.length) items[0].e.tags = tags; else delete items[0].e.tags; });
});
['tlEvNotes', 'tlEvTags'].forEach(id => $(id).addEventListener('keydown', e => { if (e.key === 'Enter') e.target.blur(); }));

// A DMX event's channel, value and fades (T-453): one event at a time.
let tlDmxKey = '';
function assignTlDmxKey(v) { return (tlDmxKey = v); }
// A fade in beats (an older edit) shown in seconds at the show's first tempo.
const tlDurS = d => d && typeof d.seconds === 'number' ? d.seconds : d && typeof d.beats === 'number' ? d.beats * 60 / (TL.show?.tempo_map?.[0]?.bpm || 120) : 0;
function tlDmxRow() {
  const items = TL.show ? tlSelected() : [];
  const e = items.length === 1 && items[0].e.source.kind === 'dmx' ? items[0].e : null;
  const names = dmxOutNames();
  const key = e ? JSON.stringify([e.id, e.source, e.fade_in, e.fade_out, names]) : '';
  if (key === tlDmxKey || $('tlDmxRow').contains(document.activeElement)) return;
  tlDmxKey = key;
  $('tlDmxRow').hidden = !e;
  if (!e) return;
  const list = names.includes(e.source.channel) ? names : [...names, e.source.channel];
  $('tlDmxCh').replaceChildren(...list.map(n => { const o = document.createElement('option'); o.value = n; o.textContent = names.includes(n) ? n : n + ' (inconnu)'; return o; }));
  $('tlDmxCh').value = e.source.channel;
  $('tlDmxVal').value = e.source.value;
  $('tlDmxIn').value = +tlDurS(e.fade_in).toFixed(2);
  $('tlDmxOut').value = +tlDurS(e.fade_out).toFixed(2);
}
function tlDmxEdit(f) {
  const items = tlSelected();
  if (items.length !== 1 || items[0].e.source.kind !== 'dmx') return;
  if (items[0].t.locked) { tlMsg('Piste verrouillée', true); tlDmxKey = ''; return; }
  tlChange(() => f(items[0].e));
}
const tlSecs = v => ({ seconds: Math.max(0, Math.min(600, Number.isFinite(+v) ? +v : 0)) });
$('tlDmxCh').addEventListener('change', () => tlDmxEdit(e => { e.source.channel = $('tlDmxCh').value; }));
$('tlDmxVal').addEventListener('change', () => tlDmxEdit(e => { const v = Math.round(+$('tlDmxVal').value); e.source.value = Number.isFinite(v) ? Math.max(0, Math.min(255, v)) : 0; }));
$('tlDmxIn').addEventListener('change', () => tlDmxEdit(e => { e.fade_in = tlSecs($('tlDmxIn').value); }));
$('tlDmxOut').addEventListener('change', () => tlDmxEdit(e => { e.fade_out = tlSecs($('tlDmxOut').value); }));
['tlDmxVal', 'tlDmxIn', 'tlDmxOut'].forEach(id => $(id).addEventListener('keydown', e => { if (e.key === 'Enter') e.target.blur(); }));

// --- selection, copy / paste / duplicate / delete ---
function tlSelected() { return tlEvents().filter(x => TL.sel.has(x.e.id)); }
function tlDelete() {
  if (!TL.sel.size) return;
  // Locked tracks keep theirs (T-445), still selected.
  const kept = tlSelected().filter(x => x.t.locked).map(x => x.e.id);
  if (kept.length && kept.length === TL.sel.size) { tlMsg('Piste verrouillée', true); return; }
  tlChange(() => { for (const t of TL.show.tracks) if (!t.locked) t.events = t.events.filter(e => !TL.sel.has(e.id)); });
  TL.sel = new Set(kept);
  if (kept.length) tlMsg(tlIgnoredMsg(kept.length));
}
// A phrase: events with their position in bars, so a paste moves them by whole bars.
function tlPhrase(items) {
  const fromBar = Math.floor(Math.min(...items.map(x => tlBarAt(x.e.start))) + 1e-6);
  const toBar = Math.ceil(Math.max(...items.map(x => tlBarAt(x.e.start + x.e.len))) - 1e-6);
  return { fromBar, bars: Math.max(1, toBar - fromBar), items: items.map(x => ({ ti: x.ti, e: JSON.parse(JSON.stringify(x.e)), bar: tlBarAt(x.e.start), endBar: tlBarAt(x.e.start + x.e.len) })) };
}
function tlCopy() {
  let items = tlSelected();
  const r = TL.show && TL.show.loop_region;
  if (!items.length && r) items = tlEvents().filter(x => x.e.start >= r[0] - 1e-9 && x.e.start < r[1]);
  if (!items.length) { tlMsg('Rien à copier : sélectionnez des événements (ou une région de boucle).', true); return; }
  TL.clip = tlPhrase(items);
  tlMsg(`Phrase copiée : ${items.length} événement(s), ${TL.clip.bars} mesure(s).`);
  $('tlPaste').disabled = false;
}
// Paste `phrase` shifted by `bars` whole bars; the new events become the
// selection. Nothing lands on a locked track (T-445): -1 if nothing did,
// else how many were left out.
function tlPasteShift(phrase, bars) {
  const target = it => Math.min(it.ti, TL.show.tracks.length - 1);
  const skipped = phrase.items.filter(it => tlLockedTi(target(it))).length;
  if (skipped === phrase.items.length) { tlMsg('Piste verrouillée', true); return -1; }
  tlChange(() => {
    let id = tlNextId();
    TL.sel.clear();
    for (const it of phrase.items) {
      const ti = target(it);
      if (ti < 0) return;
      if (tlLockedTi(ti)) continue;
      const start = tlUAtBar(it.bar + bars), end = tlUAtBar(it.endBar + bars);
      const e = { ...JSON.parse(JSON.stringify(it.e)), id: id++, start, len: Math.max(TL_MIN_LEN, end - start) };
      TL.show.tracks[ti].events.push(e);
      TL.sel.add(e.id);
    }
  });
  if (skipped) tlMsg(tlIgnoredMsg(skipped));
  return skipped;
}
function tlPaste() {
  if (!TL.clip || !TL.show) return;
  if (!TL.show.tracks.length) { tlMsg('Ajoutez une piste d\'abord.', true); return; }
  const to = Math.ceil(tlBarAt(tlPlayhead()) - 1e-6);
  if (tlPasteShift(TL.clip, to - TL.clip.fromBar) === 0) tlMsg(`Collé à la mesure ${to + 1}.`);
}
function tlDuplicate() {
  const items = tlSelected();
  if (!items.length) return;
  const p = tlPhrase(items);
  tlPasteShift(p, p.bars);
}
$('tlCopy').onclick = tlCopy;
$('tlPaste').onclick = tlPaste;
$('tlDup').onclick = tlDuplicate;
$('tlDel').onclick = tlDelete;
$('tlUndo').onclick = tlUndo;
$('tlRedo').onclick = tlRedo;

// --- Modèles de phrases (T-165) ---
// The server lays the template on the region (POST
// /api/timeline/templates/render, which changes nothing); the result goes
// out through tlChange like any edit: one undo step, locked tracks and
// show mode refused by /api/timeline/edit. Never touches the arm state.
const TPL = { list: [], categories: [], cur: null, shown: null, fit: 'repeat', region: null };
function tplMsg(text, err = false) { $('tlTplMsg').textContent = text; $('tlTplMsg').classList.toggle('err', err); }
async function openTlTemplates() {
  if (!TL.show) { tlMsg('Ouvrez ou créez un show d\'abord.', true); return; }
  $('tlTplDialog').hidden = false;
  tplMsg('');
  await tplLoad();
  tplRegions();
  const first = $('tlTplList').querySelector('button') || $('tlTplName');
  if (first) first.focus();
}
function closeTlTemplates() { $('tlTplDialog').hidden = true; }
async function tplLoad() {
  try {
    const j = await apiJson('GET', '/api/timeline/templates');
    TPL.list = j.templates || []; TPL.categories = j.categories || [];
  } catch { TPL.list = []; tplMsg('Modèles indisponibles.', true); }
  if (TPL.cur && !TPL.list.some(t => t.name === TPL.cur)) TPL.cur = null;
  $('tlTplList').innerHTML = TPL.list.map(t => `<button class="tl-ex" data-tpl="${esc(t.name)}" aria-pressed="${t.name === TPL.cur}">
      <b>${esc(t.name)}</b><span class="meta">${+t.length_bars} mesure(s) · ${t.slots.length} emplacement(s) · ${t.events.length} événement(s)${t.builtin ? ' <span class="tl-tpl-badge">Intégré</span>' : ''}</span>${t.description ? `<span class="desc">${esc(t.description)}</span>` : ''}</button>`).join('')
    || '<div class="hint">Aucun modèle pour l\'instant : sélectionnez des événements dans la timeline et enregistrez-les ci-dessous.</div>';
  tplShow();
}
const tplCurrent = () => TPL.list.find(t => t.name === TPL.cur) || null;
function tplShow() {
  const t = tplCurrent();
  $('tlTplApply').hidden = !t;
  if (!t) return;
  $('tlTplDel').hidden = !!t.builtin;
  $('tlTplInfo').textContent = `${t.length_bars} mesure(s) en ${t.beats_per_bar}/4 · emplacements ${t.slots.join(', ') || '—'}${t.bus_envelopes.length ? ' · enveloppes de bus' : ''}`;
  const autoOpts = TPL.categories.map(c => `<option value="auto:${esc(c)}">Auto : ${esc(c)}</option>`).join('');
  const cueOpts = TPL.categories.map(c => `<optgroup label="${esc(c)}">` + presets.filter(p => p.category === c).map(p => `<option value="cue:${esc(p.id)}">${esc(p.name)}</option>`).join('') + '</optgroup>').join('');
  const keep = {};
  $('tlTplSlots').querySelectorAll('select').forEach(s => { keep[s.dataset.slot] = s.value; });
  $('tlTplSlots').innerHTML = t.slots.map(s => `<label>${esc(s)} : <select data-slot="${esc(s)}" aria-label="Cue de l'emplacement ${esc(s)}"><option value="">— choisir un cue —</option><optgroup label="Auto (un cue de la page)">${autoOpts}</optgroup>${cueOpts}</select></label>`).join('');
  // On the same template the operator's choices stay; on another one the
  // page it suggests (built-ins, T-166) comes first, then the last choice.
  const pages = t.auto_pages || {}, same = TPL.shown === t.name;
  TPL.shown = t.name;
  $('tlTplSlots').querySelectorAll('select').forEach(s => {
    const page = pages[s.dataset.slot], kept = keep[s.dataset.slot];
    const suggested = page && TPL.categories.includes(page) ? 'auto:' + page : '';
    s.value = (same && kept) || suggested || kept || '';
  });
  $('tlTplTrack').innerHTML = TL.show.tracks.map((tr, i) => tr.kind !== 'cues' ? '' : `<option value="${i}">${esc(tr.name)}${tr.locked ? ' (verrouillée)' : ''}</option>`).join('') + `<option value="${TL.show.tracks.length}">Nouvelle piste</option>`;
  $('tlTplFit').querySelectorAll('[data-fit]').forEach(b => { b.classList.toggle('active', b.dataset.fit === TPL.fit); b.setAttribute('aria-pressed', String(b.dataset.fit === TPL.fit)); });
  tplPreview(t);
}
// A schematic: one lane per template track, events by slot, bar lines.
function tplPreview(t) {
  const c = $('tlTplPreview'), dpr = window.devicePixelRatio || 1, W = c.clientWidth || 560, H = c.clientHeight || 72;
  c.width = Math.round(W * dpr); c.height = Math.round(H * dpr);
  const g = c.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  const cs = getComputedStyle(document.documentElement), col = v => cs.getPropertyValue(v).trim();
  g.clearRect(0, 0, W, H);
  const beats = t.length_bars * t.beats_per_bar, lanes = Math.max(1, ...t.events.map(e => e.track + 1)) + (t.bus_envelopes.length ? 1 : 0);
  const x = b => 4 + b / beats * (W - 8), lh = (H - 8) / lanes;
  g.strokeStyle = col('--line');
  for (let bar = 0; bar <= t.length_bars; bar++) { g.beginPath(); g.moveTo(x(bar * t.beats_per_bar) + .5, 2); g.lineTo(x(bar * t.beats_per_bar) + .5, H - 2); g.stroke(); }
  g.font = '11px system-ui, sans-serif'; g.textBaseline = 'middle';
  for (const e of t.events) {
    const y = 4 + e.track * lh, si = Math.max(0, t.slots.indexOf(e.slot));
    g.fillStyle = TL_LAYER_COLORS[si % TL_LAYER_COLORS.length];
    g.globalAlpha = .75; g.fillRect(x(e.start_beat), y + 2, Math.max(2, x(e.start_beat + e.len_beats) - x(e.start_beat)), lh - 4); g.globalAlpha = 1;
    g.fillStyle = col('--bg'); g.fillText(e.slot, x(e.start_beat) + 4, y + lh / 2);
  }
  if (t.bus_envelopes.length) { g.fillStyle = col('--muted'); g.fillText('∿ bus', 6, 4 + (lanes - 1) * lh + lh / 2); }
}
// Regions: the loop, the selection, each pair of markers; else by hand.
function tplRegions() {
  const out = [];
  const r = TL.show.loop_region;
  if (r) out.push({ label: 'Région de boucle', from: r[0], to: r[1] });
  const sel = tlSelected();
  if (sel.length) out.push({ label: 'Sélection (mesures entières)', from: tlUAtBar(Math.floor(Math.min(...sel.map(s => tlBarAt(s.e.start))) + 1e-6)), to: tlUAtBar(Math.ceil(Math.max(...sel.map(s => tlBarAt(s.e.start + s.e.len))) - 1e-6)) });
  const m = TL.show.markers;
  for (let i = 0; i + 1 < m.length; i++) if (m[i + 1].at > m[i].at) out.push({ label: `${m[i].name || 'Marqueur ' + (i + 1)} → ${m[i + 1].name || 'Marqueur ' + (i + 2)}`, from: m[i].at, to: m[i + 1].at });
  TPL.regions = out;
  $('tlTplRegion').innerHTML = '<option value="">Mesures ci-contre</option>' + out.map((g, i) => `<option value="${i}">${esc(g.label)}</option>`).join('');
  $('tlTplRegion').value = out.length ? '0' : '';
  tplRegionPick();
}
function tplRegionPick() {
  const g = TPL.regions && TPL.regions[+$('tlTplRegion').value];
  TPL.region = $('tlTplRegion').value === '' ? null : g;
  if (!TPL.region) return;
  const from = tlBarAt(g.from), to = tlBarAt(g.to);
  $('tlTplFrom').value = +(from + 1).toFixed(2);
  $('tlTplBars').value = +(to - from).toFixed(2);
}
function tplRegion() {
  if (TPL.region) return [TPL.region.from, TPL.region.to];
  const from = Math.max(1, +$('tlTplFrom').value || 1) - 1, bars = Math.max(1, +$('tlTplBars').value || 1);
  return [tlUAtBar(from), tlUAtBar(from + bars)];
}
async function tplApply() {
  const t = tplCurrent();
  if (!t || !TL.show) return;
  if (TL.inflight) { tplMsg('Modification en cours, réessayez.', true); return; }
  const slots = {};
  for (const s of $('tlTplSlots').querySelectorAll('select')) {
    if (!s.value) { tplMsg(`Emplacement ${s.dataset.slot} : choisissez un cue.`, true); s.focus(); return; }
    const [kind, ...rest] = s.value.split(':');
    slots[s.dataset.slot] = { [kind]: rest.join(':') };
  }
  const [from, to] = tplRegion();
  const body = { name: t.name, from, to, fit: TPL.fit, track: +$('tlTplTrack').value || 0, replace: $('tlTplReplace').checked, slots, show: TL.show };
  const r = await api('POST', '/api/timeline/templates/render', body, { quiet: true });
  if (!r.ok) { tplMsg(await r.text(), true); return; }
  const j = await r.json();
  const known = new Set(tlEvents().map(x => x.e.id));
  if (!tlChange(() => { TL.show = j.show; })) { tplMsg('Rien n\'a été posé (piste verrouillée ?).', true); return; }
  TL.sel = new Set(tlEvents().filter(x => !known.has(x.e.id)).map(x => x.e.id));
  TL.headsKey = ''; tlHeads();
  const msg = `« ${t.name} » posé : ${j.added} événement(s)${j.removed ? `, ${j.removed} remplacé(s)` : ''}. Annuler le retire.`;
  tplMsg(msg); tlMsg(msg);
}
async function tplSave() {
  const name = $('tlTplName').value.trim();
  if (!name) { tplMsg('Donnez un nom au modèle.', true); $('tlTplName').focus(); return; }
  if (!TL.sel.size) { tplMsg('Sélectionnez d\'abord des événements dans la timeline (Maj+clic).', true); return; }
  const body = { name, description: $('tlTplDesc').value.trim(), events: [...TL.sel], show: TL.show };
  let r = await api('POST', '/api/timeline/templates', body, { quiet: true });
  if (r.status === 409 && confirm(`Un modèle s'appelle déjà « ${name} » : le remplacer ?`)) r = await api('POST', '/api/timeline/templates', { ...body, overwrite: true }, { quiet: true });
  if (!r.ok) { tplMsg(await r.text(), true); return; }
  const j = await r.json();
  TPL.cur = j.template.name;
  await tplLoad();
  tplMsg(`Modèle « ${j.template.name} » enregistré : ${j.template.length_bars} mesure(s), emplacements ${j.template.slots.join(', ') || '—'}.` + (j.warnings && j.warnings.length ? ' ' + j.warnings.join(' · ') : ''));
}
async function tplDelete() {
  const t = tplCurrent();
  if (!t || t.builtin || !confirm(`Supprimer le modèle « ${t.name} » ?`)) return;
  const r = await api('POST', '/api/timeline/templates/delete', { name: t.name }, { quiet: true });
  if (!r.ok) { tplMsg(await r.text(), true); return; }
  TPL.cur = null;
  await tplLoad();
  tplMsg(`Modèle « ${t.name} » supprimé.`);
}
$('tlTpl').onclick = openTlTemplates;
$('tlTplClose').onclick = closeTlTemplates;
$('tlTplDialog').addEventListener('click', e => { if (e.target === $('tlTplDialog')) closeTlTemplates(); });
$('tlTplList').addEventListener('click', e => {
  const b = e.target.closest('[data-tpl]');
  if (!b) return;
  TPL.cur = b.dataset.tpl;
  $('tlTplList').querySelectorAll('[data-tpl]').forEach(x => x.setAttribute('aria-pressed', String(x === b)));
  tplShow();
});
$('tlTplFit').addEventListener('click', e => {
  const b = e.target.closest('[data-fit]');
  if (!b) return;
  TPL.fit = b.dataset.fit;
  $('tlTplFit').querySelectorAll('[data-fit]').forEach(x => { x.classList.toggle('active', x === b); x.setAttribute('aria-pressed', String(x === b)); });
});
$('tlTplRegion').addEventListener('change', tplRegionPick);
['tlTplFrom', 'tlTplBars'].forEach(id => $(id).addEventListener('input', () => { $('tlTplRegion').value = ''; TPL.region = null; }));
$('tlTplGo').onclick = tplApply;
$('tlTplSave').onclick = tplSave;
$('tlTplDel').onclick = tplDelete;

export { assignTlDmxKey, assignTlNoteKey, assignTlRefKey, assignTlScanKey, tlAddMarker, tlCopy,
  tlDelete, tlDmxRow, tlDuplicate, tlMarkRow, tlNoteRow, tlPaste, tlRefRow, tlScanRow, tlSelected,
  tlSpreadRow };
