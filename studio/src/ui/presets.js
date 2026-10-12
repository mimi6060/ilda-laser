// Laser Studio — presets.js (T-435).
// Sections (as in the former index.html script):
//   scan presets (T-415)
//   referenced presets (T-422)
//   « En cours » (T-423)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { TRANS_LENGTHS, assignPoolRev, assignPoolSel, assignPools, deck, grid, gridPage, lengthIndex,
  loadSlots, poolRev, poolSel, pools, refreshLook, selectPage, slots, tagKey } from './cues.js';
import { $, api, apiJson, hex, post, rgb, setVal } from './core.js';
import { LOCKED_FR, activeWs, flash, showMode } from './workspaces.js';
import { proj } from './zones.js';
import { macWaitText } from './macros.js';
import { fillShowSelect, fmtTime } from './shows.js';
import { spreadRead, spreadRender, spreadWire } from './modulators.js';
import { assignMenuControl, esc, menuControl } from './midi.js';

// ---------- scan presets (T-415) ----------
// GET /api/scan-presets: built-in and user presets, the output's default,
// each one's effective values (bounded by the output) and the cells or
// figures whose preset was deleted.
let scanPresets = { builtin: [], user: [], default: null, effective: {}, dangling: [] }, scanRev = 0, spSel = 'graphics';
const scanAll = () => [...scanPresets.builtin, ...scanPresets.user];
async function loadScanPresets(v) {
  try { scanPresets = v || await apiJson('GET', '/api/scan-presets', undefined, { cache: 'no-store' }); } catch { return; }
  scanRev++;
  showScanPresets();
}
// A preset list for a cue, a figure or an event; a deleted preset still
// referenced shows as such (it draws with the output's default).
function fillScanSelect(sel, none, value) {
  const opt = (v, t) => `<option value="${esc(v)}">${esc(t)}</option>`;
  sel.innerHTML = opt('', none) + scanAll().map(p => opt(p.id, p.name)).join('');
  if (value && !scanAll().some(p => p.id === value)) sel.insertAdjacentHTML('beforeend', opt(value, 'Préréglage supprimé (défaut de la sortie)'));
  sel.value = value;
}
async function postScan(body) {
  const r = await api('POST', '/api/scan-presets', body, { quiet: true });
  if (!r.ok) { $('spMsg').textContent = await r.text(); return false; }
  await loadScanPresets(await r.json());
  return true;
}
function showScanPresets() {
  if (!scanAll().some(p => p.id === spSel)) spSel = 'graphics';
  fillScanSelect($('spDefault'), 'Auto (réglages de l\'optimiseur)', scanPresets.default || '');
  const list = $('spList');
  list.innerHTML = scanAll().map(p => `<option value="${esc(p.id)}">${esc(p.name)}${p.builtin ? ' (intégré)' : ''}</option>`).join('');
  list.value = spSel;
  const p = scanAll().find(x => x.id === spSel), eff = scanPresets.effective[spSel] || {};
  if (!p) return;
  const ro = !!p.builtin;
  ['spLit', 'spBlank', 'spCorner', 'spEnd', 'spScan', 'spName', 'spDelete'].forEach(id => { $(id).disabled = ro; });
  if (document.activeElement !== $('spName')) $('spName').value = p.name;
  setVal('spLit', p.params.lit_step); $('spLitV').textContent = p.params.lit_step.toFixed(3);
  setVal('spBlank', p.params.blank_step); $('spBlankV').textContent = p.params.blank_step.toFixed(2);
  setVal('spCorner', p.params.max_corner_dwell); $('spCornerV').textContent = p.params.max_corner_dwell;
  setVal('spEnd', p.params.endpoint_dwell); $('spEndV').textContent = p.params.endpoint_dwell;
  setVal('spScan', p.scan_pct);
  $('spScanV').textContent = Math.round(p.scan_pct) + (eff.clamped ? ` (limité par la sortie : ${Math.round(eff.scan_pct)})` : '');
  const d = scanPresets.dangling || [];
  $('spMsg').textContent = d.length ? `${d.length} cue(s) ou figure(s) utilisent un préréglage supprimé : ils prennent le balayage par défaut de la sortie.` : '';
}
$('spList').addEventListener('change', () => { spSel = $('spList').value; showScanPresets(); });
$('spDefault').addEventListener('change', () => postScan({ default: $('spDefault').value }));
$('spDup').addEventListener('click', async () => {
  const p = scanAll().find(x => x.id === spSel);
  if (!p) return;
  const n = 1 + Math.max(0, ...scanPresets.user.map(u => +u.id.slice(2) || 0));
  const copy = { ...JSON.parse(JSON.stringify(p)), id: 'u-' + n, name: (p.name + ' (copie)').slice(0, 40), builtin: false };
  if (await postScan({ user: [...scanPresets.user, copy] })) { spSel = copy.id; showScanPresets(); }
});
$('spDelete').addEventListener('click', async () => {
  if (!scanPresets.user.some(u => u.id === spSel)) return;
  const id = spSel;
  if (await postScan({ user: scanPresets.user.filter(u => u.id !== id) })) { spSel = 'graphics'; showScanPresets(); }
});
// Edit the selected user preset: one field, then the whole user list. The
// local copy changes at once, so two quick edits never undo each other.
let spQueue = Promise.resolve();
function spEdit(fn) {
  const p = scanPresets.user.find(u => u.id === spSel);
  if (!p) return;
  fn(p);
  const user = JSON.parse(JSON.stringify(scanPresets.user));
  spQueue = spQueue.then(() => postScan({ user }));
}
$('spName').addEventListener('change', () => { const v = $('spName').value.trim(); if (v) spEdit(p => { p.name = v; }); });
[['spLit', 'lit_step'], ['spBlank', 'blank_step'], ['spCorner', 'max_corner_dwell'], ['spEnd', 'endpoint_dwell']].forEach(([id, key]) => {
  $(id).addEventListener('input', () => { $(id + 'V').textContent = $(id).value; });
  $(id).addEventListener('change', () => spEdit(p => { p.params[key] = +$(id).value; }));
});
$('spScan').addEventListener('input', () => { $('spScanV').textContent = $('spScan').value; });
$('spScan').addEventListener('change', () => spEdit(p => { p.scan_pct = +$('spScan').value; }));

// « Propriétés du cue » (right click): its own click mode and group.
let menuCue = null;
function assignMenuCue(v) { return (menuCue = v); }
let menuSlot = null;
function openCueMenu(p, x, y, at) {
  if (showMode) return flash(LOCKED_FR); // T-283: no cue editing
  menuCue = p.id; menuSlot = at;
  const pg = grid.pages[gridPage];
  const own = !!(pg && pg.editable && !pg.auto);
  // T-405: a user cue is renamed, recoloured or deleted; a built-in one is
  // only taken off the page, or copied into an editable user cue.
  $('cueMenuRemove').hidden = !own || !!p.user;
  $('cueMenuDelete').hidden = !own || !p.user;
  $('cueMenuCopy').hidden = !own;
  $('cueMenuToUser').hidden = !!p.user;
  $('cueMenuUser').hidden = !p.user;
  if (p.user) {
    $('cueMenuNameInput').value = p.name;
    $('cueMenuColor').value = hex(p.color || [0, 255, 0]);
  }
  const slot = slots[p.id] || {};
  $('cueMenuName').textContent = p.name;
  $('cueMenuMode').value = slot.mode || '';
  $('cueMenuGroup').value = slot.group ? String(slot.group) : '';
  $('cueMenuLayer').value = String(slot.layer || 1);
  $('cueMenuSpeed').value = slot.speed_group ? String(slot.speed_group) : '';
  fillScanSelect($('cueMenuScan'), 'Par défaut de la sortie', slot.scan || '');
  // T-422: the presets this cue refers to; « Valeurs du cue » = none.
  $('cueMenuRefs').querySelectorAll('select').forEach(sel => fillPoolSelect(sel, 'Valeurs du cue', slot.refs?.[sel.dataset.family] || ''));
  // T-158: the cue's own transition, else the grid's.
  $('cueMenuTransition').value = slot.transition ? slot.transition.kind : '';
  const li = lengthIndex((slot.transition || deck.transition || {}).length);
  $('cueMenuTransLen').value = String(li >= 0 ? li : 2);
  $('cueMenuTransLen').disabled = !slot.transition;
  $('cueMenuProtStop').checked = !!slot.protect_stop;
  $('cueMenuProtSolo').checked = !!slot.protect_solo;
  $('cueMenuVljSkip').checked = !!slot.vlj_skip;
  // T-430: the cell's colour (none = category or user cue), tags, notes.
  menuColor = slot.color || null;
  $('cueMenuCellColor').value = hex(slot.color || (p.user && p.color) || [0, 255, 0]);
  $('cueMenuCellColor').classList.toggle('unset', !slot.color);
  $('cueMenuCellColorNone').disabled = !slot.color;
  menuTags = [...(slot.tags || [])];
  renderMenuTags();
  $('cueMenuTagInput').value = '';
  $('cueMenuNotes').value = slot.notes || '';
  fillShowSelect($('cueMenuShow'), 'Aucun', slot.show || '');
  // Projection zones (T-012): none ticked = the default zones.
  $('cueMenuZones').replaceChildren(...(proj ? proj.zones : []).map(z => {
    const l = document.createElement('label'), c = document.createElement('input');
    c.type = 'checkbox'; c.value = z.id; c.checked = (slot.zones || []).includes(z.id);
    c.addEventListener('change', saveCueMenu);
    l.append(c, `${z.id} · ${z.name}`);
    return l;
  }));
  // Zone chaser tied to the cue (T-170); one that no longer exists shows as such.
  const chSel = $('cueMenuChaser'), chasers = (proj && proj.chasers) || [];
  chSel.replaceChildren(new Option('Aucun (chenillards libres)', ''), ...chasers.map((c, i) => new Option(`${i + 1} · ${c.name}`, String(i))));
  if (slot.zone_chaser != null && slot.zone_chaser >= chasers.length) chSel.add(new Option(`${slot.zone_chaser + 1} · (supprimé)`, String(slot.zone_chaser)));
  chSel.value = slot.zone_chaser != null ? String(slot.zone_chaser) : '';
  // Spread across a zone group (T-421).
  delete $('cueMenuSpread').dataset.seed;
  spreadRender($('cueMenuSpread'), slot.spread);
  const cell = document.querySelector(`#cues .cue[data-id="${CSS.escape(p.id)}"]`);
  assignMenuControl(cell?.dataset.control || null);
  $('cueMenuMidi').hidden = !menuControl;
  const m = $('cueMenu');
  m.hidden = false;
  m.style.left = Math.max(8, Math.min(x, innerWidth - m.offsetWidth - 8)) + 'px';
  m.style.top = Math.max(8, Math.min(y, innerHeight - m.offsetHeight - 8)) + 'px';
}
async function saveCueMenu() {
  if (!menuCue) return;
  const mode = $('cueMenuMode').value || null, group = $('cueMenuGroup').value ? +$('cueMenuGroup').value : null;
  const layer = +$('cueMenuLayer').value > 1 ? +$('cueMenuLayer').value : null;
  const show = $('cueMenuShow').value || null;
  const zones = [...$('cueMenuZones').querySelectorAll('input:checked')].map(c => +c.value);
  const kind = $('cueMenuTransition').value;
  const own = slots[menuCue] && slots[menuCue].transition;
  const transition = kind ? { kind, length: TRANS_LENGTHS[+$('cueMenuTransLen').value], in_flash: !!(own && own.in_flash) } : null;
  $('cueMenuTransLen').disabled = !kind;
  const protect_stop = $('cueMenuProtStop').checked, protect_solo = $('cueMenuProtSolo').checked, vlj_skip = $('cueMenuVljSkip').checked;
  const scan = $('cueMenuScan').value || null;
  const notes = $('cueMenuNotes').value, tags = menuTags, color = menuColor;
  const speed_group = $('cueMenuSpeed').value ? +$('cueMenuSpeed').value : null;
  const zone_chaser = $('cueMenuChaser').value !== '' ? +$('cueMenuChaser').value : null;
  const refs = {};
  $('cueMenuRefs').querySelectorAll('select').forEach(sel => { if (sel.value) refs[sel.dataset.family] = sel.value; });
  const spread = spreadRead($('cueMenuSpread'));
  await post('/api/cues/slot', { id: menuCue, mode, group, layer, show, zones, transition, protect_stop, protect_solo, vlj_skip, scan, notes, tags, color, speed_group, zone_chaser, refs, spread });
  if (Object.keys(refs).length || Object.keys(slots[menuCue]?.refs || {}).length) loadPools(false);
  await loadSlots();
}
// ---------- referenced presets (T-422) ----------
// Position, size, colour and motion presets that cues and timeline events
// refer to: change one, and every cue using it follows at the next frame.
// They are edits (refused in show mode, undone with Annuler); they never
// set the brightness, a cap or a safety setting.
const POOL_FAMILIES = [['position', 'Position'], ['size', 'Taille'], ['color', 'Couleur'], ['motion', 'Mouvement']];
const poolName = id => pools.presets.find(p => p.id === id)?.name;
async function loadPools(render = true) {
  try { assignPools(await apiJson('GET', '/api/pools', undefined, { quiet: true })); } catch { return; }
  assignPoolRev(poolRev + 1);
  renderPools();
  if (render) loadSlots();
}
function fillPoolSelect(sel, none, value) {
  const fam = sel.dataset.family;
  const opt = (v, t) => `<option value="${esc(v)}">${esc(t)}</option>`;
  sel.innerHTML = opt('', none) + pools.presets.filter(p => p.family === fam).map(p => opt(p.id, p.name)).join('');
  if (value && !pools.presets.some(p => p.id === value && p.family === fam)) sel.insertAdjacentHTML('beforeend', opt(value, 'Préréglage inconnu (valeurs propres)'));
  sel.value = value;
}
function renderPools() {
  const box = $('poolFamilies');
  if (poolSel && !pools.presets.some(p => p.id === poolSel)) assignPoolSel(null);
  const open = new Set([...box.querySelectorAll('details[open]')].map(d => d.dataset.family));
  const first = !box.children.length;
  box.innerHTML = POOL_FAMILIES.map(([f, label]) => {
    const list = pools.presets.filter(p => p.family === f);
    const chips = list.map(p => {
      const users = pools.uses[p.id] || [];
      return `<button class="pool-chip" data-pool="${esc(p.id)}" aria-pressed="${p.id === poolSel}" title="${esc(users.length ? 'Utilisé par :\n' + users.join('\n') : 'Aucun cue ne l\'utilise')}">${esc(p.name)}<small>${users.length} cue${users.length > 1 ? 's' : ''}</small></button>`;
    }).join('');
    return `<details class="pool-fam" data-family="${f}"${first || open.has(f) ? ' open' : ''}><summary>${label} (${list.length})</summary>`
      + `<div class="pool-chips">${chips || '<span class="hint">Aucun préréglage</span>'}</div>`
      + `<div class="row lock-edit"><button data-pool-add="${f}" title="Crée un préréglage « ${label} » avec les valeurs actuelles du direct (cette famille seulement), nommé comme saisi ci-dessus">+ Depuis le direct</button></div></details>`;
  }).join('');
  $('poolEdit').hidden = !poolSel;
}
function poolMsg(text) { $('poolMsg').textContent = text; }
// The answer's message: `{ error, users }` or plain text.
async function poolRefusal(r) {
  const t = await r.text();
  try { const j = JSON.parse(t); return j.users?.length ? 'Préréglage utilisé par : ' + j.users.join(' ; ') : j.error || t; } catch { return t; }
}
async function poolSend(path, body) {
  const r = await api('POST', path, body, { quiet: true });
  if (!r.ok) { poolMsg(await poolRefusal(r)); return null; }
  const text = await r.text();
  await loadPools();
  try { return JSON.parse(text); } catch { return {}; }
}
$('poolFamilies').addEventListener('click', async e => {
  const chip = e.target.closest('[data-pool]');
  if (chip) {
    assignPoolSel(chip.dataset.pool === poolSel ? null : chip.dataset.pool);
    if (poolSel) $('poolName').value = poolName(poolSel) || '';
    poolMsg('');
    renderPools();
    return;
  }
  const add = e.target.closest('[data-pool-add]');
  if (!add) return;
  const family = add.dataset.poolAdd;
  const label = POOL_FAMILIES.find(([f]) => f === family)[1];
  const taken = new Set(pools.presets.filter(p => p.family === family).map(p => p.name.toLowerCase()));
  let name = $('poolName').value.trim();
  if (!name || (poolSel && name === poolName(poolSel))) name = [...Array(65).keys()].map(n => `${label} ${n + 1}`).find(n => !taken.has(n.toLowerCase()));
  const p = await poolSend('/api/pools/capture', { family, name });
  if (p) { assignPoolSel(p.id); $('poolName').value = p.name; poolMsg(`« ${p.name} » enregistré depuis le direct`); renderPools(); }
});
$('poolUpdate').onclick = async () => {
  const p = pools.presets.find(x => x.id === poolSel);
  if (!p) return;
  if (await poolSend('/api/pools/capture', { family: p.family, id: p.id })) poolMsg(`« ${p.name} » mis à jour depuis le direct`);
};
$('poolRename').onclick = async () => {
  const name = $('poolName').value.trim();
  if (!poolSel || !name) return;
  const list = pools.presets.map(p => p.id === poolSel ? { ...p, name } : p);
  if (await poolSend('/api/pools', list)) poolMsg(`Renommé en « ${name} »`);
};
$('poolDelete').onclick = async () => {
  const p = pools.presets.find(x => x.id === poolSel);
  if (!p) return;
  if (await poolSend('/api/pools/delete', { id: p.id })) { assignPoolSel(null); $('poolName').value = ''; renderPools(); poolMsg(`« ${p.name} » supprimé`); }
};

// Tags in the properties menu (T-430): chips, typed with Enter or a comma;
// 8 at most, repeats (without case or accents) ignored, like cues.rs.
let menuTags = [], menuColor = null;
function renderMenuTags() {
  const box = $('cueMenuTags'), input = $('cueMenuTagInput');
  box.querySelectorAll('.tag-chip').forEach(c => c.remove());
  menuTags.forEach((t, i) => {
    const c = document.createElement('span');
    c.className = 'tag-chip';
    c.textContent = t;
    const x = document.createElement('button');
    x.type = 'button'; x.textContent = '×'; x.title = 'Retirer l’étiquette';
    x.onclick = () => { menuTags.splice(i, 1); renderMenuTags(); saveCueMenu(); };
    c.appendChild(x);
    box.insertBefore(c, input);
  });
  input.disabled = menuTags.length >= 8;
  input.placeholder = menuTags.length >= 8 ? '8 étiquettes au plus' : 'intro, drop…';
}
function addMenuTags(text) {
  let changed = false;
  for (const raw of text.split(',')) {
    const t = raw.trim().slice(0, 24).trim();
    if (!t || menuTags.length >= 8 || menuTags.some(x => tagKey(x) === tagKey(t))) continue;
    menuTags.push(t); changed = true;
  }
  if (changed) { renderMenuTags(); saveCueMenu(); }
}
$('cueMenuTagInput').addEventListener('keydown', e => {
  const input = e.target;
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault();
    addMenuTags(input.value); input.value = '';
  } else if (e.key === 'Backspace' && !input.value && menuTags.length) {
    menuTags.pop(); renderMenuTags(); saveCueMenu();
  }
});
// A pasted list, or a suggestion picked from the list, is taken at once.
$('cueMenuTagInput').addEventListener('input', e => {
  const input = e.target;
  if (input.value.includes(',') || e.inputType === 'insertReplacementText') { addMenuTags(input.value); input.value = ''; }
});
$('cueMenuTagInput').addEventListener('blur', e => { if (e.target.value.trim()) { addMenuTags(e.target.value); e.target.value = ''; } });
$('cueMenuNotes').addEventListener('change', saveCueMenu);
$('cueMenuCellColor').addEventListener('change', () => {
  menuColor = rgb($('cueMenuCellColor').value);
  $('cueMenuCellColor').classList.remove('unset');
  $('cueMenuCellColorNone').disabled = false;
  saveCueMenu();
});
$('cueMenuCellColorNone').addEventListener('click', () => {
  menuColor = null;
  $('cueMenuCellColor').classList.add('unset');
  $('cueMenuCellColorNone').disabled = true;
  saveCueMenu();
});
['cueMenuRefPosition', 'cueMenuRefSize', 'cueMenuRefColor', 'cueMenuRefMotion'].forEach(id => $(id).addEventListener('change', saveCueMenu));
['cueMenuMode', 'cueMenuGroup', 'cueMenuLayer', 'cueMenuSpeed', 'cueMenuChaser', 'cueMenuScan', 'cueMenuShow', 'cueMenuTransition', 'cueMenuTransLen', 'cueMenuProtStop', 'cueMenuProtSolo', 'cueMenuVljSkip'].forEach(id => $(id).addEventListener('change', saveCueMenu));
spreadWire($('cueMenuSpread'), () => saveCueMenu());

// ---------- « En cours » (T-423) ----------
// The tab's count comes with every frame (cues playing + 1 for a running
// timeline); the panel itself reads GET /api/running 4 times a second,
// only while it is on screen.
const LOCK_SVG = '<svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="7" width="10" height="7" rx="1.5"/><path d="M5 7V5a3 3 0 0 1 6 0v2"/></svg>';
function showRunCount(f) {
  const n = ((f.cues && f.cues.active) || []).length + (f.timeline && (f.timeline.playing || f.timeline.paused) ? 1 : 0)
    + (f.cuelist && (f.cuelist.index != null || f.cuelist.waiting != null) ? 1 : 0)
    + ((f.macros && f.macros.running) || []).length
    + ((f.scripts && f.scripts.running) || []).length;
  const b = $('runCount');
  b.hidden = !n;
  b.textContent = n;
}
let runTimer = 0, runBusy = false;
function pollRunning() {
  clearTimeout(runTimer);
  if ($('runPanel').hidden || activeWs !== 'live') return;
  runTimer = setTimeout(pollRunning, 250);
  if (runBusy || document.hidden) return;
  runBusy = true;
  apiJson('GET', '/api/running', undefined, { cache: 'no-store' }).then(showRunning).catch(() => {}).finally(() => { runBusy = false; });
}
function runRow(name, detail, opts = {}) {
  const row = document.createElement('div');
  row.className = 'run-row' + (opts.dim ? ' dim' : '') + (opts.warn ? ' warn' : '');
  const n = document.createElement(opts.onName ? 'button' : 'span');
  n.className = 'n'; n.textContent = name;
  if (opts.onName) { n.title = opts.nameTitle || ''; n.onclick = opts.onName; }
  row.appendChild(n);
  if (opts.lock) row.insertAdjacentHTML('beforeend', LOCK_SVG.replace('<svg', `<svg><title>${opts.lock}</title`));
  if (detail) { const d = document.createElement('span'); d.className = 'd'; d.textContent = detail; row.appendChild(d); }
  if (opts.source) { const s = document.createElement('span'); s.className = 'src'; s.textContent = opts.source; s.title = 'Source du dernier changement'; row.appendChild(s); }
  if (opts.stop) { const b = document.createElement('button'); b.textContent = opts.stopLabel || 'Arrêter'; b.onclick = opts.stop; row.appendChild(b); }
  return row;
}
function fillRun(id, rows) {
  const box = $(id);
  if (!rows.length) { const e = document.createElement('div'); e.className = 'run-empty'; e.textContent = 'Rien ne joue'; rows = [e]; }
  box.replaceChildren(...rows);
}
let runKey = '';
function showRunning(r) {
  const key = JSON.stringify(r);
  if (key === runKey) return;
  runKey = key;
  // Phases (T-158/T-159): « en attente » of its beat, « démarrage », « en fondu ».
  const phase = c => c.phase === 'playing' ? '' : c.phase_label + (c.progress != null ? ` ${Math.round(c.progress * 100)} %` : '');
  fillRun('runCues', r.cues.map(c => runRow(c.name,
    [phase(c), c.page ? 'p. ' + c.page : '', 'C' + c.layer, c.mode_label].filter(Boolean).join(' · '),
    { source: c.source_label, dim: !c.shown,
      lock: c.protect_stop ? 'Protégé de Tout arrêter' : c.protect_solo ? 'Protégé du solo' : '',
      nameTitle: c.page ? `Afficher la page ${c.page_name || c.page}` : '',
      onName: c.page ? () => locateCue(c.cue, c.page - 1) : null,
      stop: () => runStop(c.instance == null ? { kind: 'pending', cue: c.cue } : { kind: 'cue', instance: c.instance }) })));
  const t = r.timeline, rows = [];
  if (t) rows.push(runRow(t.name || 'Show', `${t.playing ? (t.waiting ? 'attend la mesure' : 'lecture') : 'pause'} · ${t.time_base === 'beats' ? t.position.toFixed(1) + ' temps' : fmtTime(t.position)}`,
    { source: 'Timeline', stop: () => runStop({ kind: 'timeline' }) }));
  if (r.playlist) rows.push(runRow('Playlist', r.playlist.scene || '', { stop: () => runStop({ kind: 'timeline' }) }));
  // The cue list (T-410): its step, phase and who drove it; Arrêter = Début.
  const cl = r.cuelist;
  if (cl) rows.push(runRow('Liste « ' + cl.name + ' »', [cl.index != null ? `pas ${cl.index + 1}/${cl.steps}${cl.label ? ' ' + cl.label : ''}` : '',
    cl.paused ? 'pause' : cl.phase.phase === 'fading' ? `fondu ${Math.round(cl.phase.t * 100)} %` : cl.phase.phase === 'delay' ? 'délai' : '',
    'C' + cl.layer].filter(Boolean).join(' · '), { source: cl.source_label, stop: () => runStop({ kind: 'cuelist' }) }));
  fillRun('runTimeline', rows);
  // Macros (T-425): the step reached, what it waits for, who launched it.
  fillRun('runMacros', (r.macros || []).map(m => runRow(m.name,
    [`étape ${Math.min(m.step, m.steps)}/${m.steps}`, macWaitText(m.waiting), m.error ? '⚠ ' + m.error : ''].filter(Boolean).join(' · '),
    { source: m.source_label, warn: !!m.error, stop: () => runStop({ kind: 'macro', id: m.id }) })));
  // Scripts (T-458): runs in flight and automatic triggers armed.
  const TRIG_FR = { manual: 'à la demande', every_beat: 'à chaque temps', every_bar: 'à chaque mesure' };
  fillRun('runScripts', (r.scripts || []).map(x => runRow(x.name, [x.running ? 'en cours' : '', TRIG_FR[x.trigger] || ''].filter(Boolean).join(' · '),
    { stop: () => runStop({ kind: 'script', id: String(x.id) }) })));
  fillRun('runLfos', r.lfos.map(m => runRow(m.target_label, `${m.wave_label} · ${m.rate}`, { stop: () => runStop({ kind: 'lfo', index: m.index }), stopLabel: 'Couper' })));
  fillRun('runAudio', r.audio_links.map(a => runRow(a.target_label, a.source_label, { stop: () => runStop({ kind: 'audio_link', index: a.index }), stopLabel: 'Couper' })));
  const fmt = v => typeof v === 'boolean' ? (v ? 'oui' : 'non') : typeof v === 'number' ? (Number.isInteger(v) ? v : v.toFixed(2)) : v;
  fillRun('runMasters', r.masters.map(m => runRow(m.label, `${fmt(m.value)} (neutre ${fmt(m.default)})`, { source: m.source_label || '' })));
  $('runStatus').replaceChildren(...r.status.map(x => runRow(x.label, x.value, { warn: x.warn })));
}
// Show a cue's page and ring its cell, without playing it.
function locateCue(id, page) {
  if (page !== gridPage) selectPage(page);
  const cell = document.querySelector(`#cues .cue[data-id="${CSS.escape(id)}"]`);
  if (!cell) return;
  cell.classList.add('located');
  cell.scrollIntoView({ block: 'nearest' });
  setTimeout(() => cell.classList.remove('located'), 2000);
}
async function runStop(body) {
  const r = await api('POST', '/api/running/stop', body, { quiet: true });
  $('runMsg').textContent = r.ok ? '' : await r.text();
  runKey = '';
  if (r.ok) refreshLook();
  pollRunning();
}
document.querySelectorAll('#runPanel [data-run-stop]').forEach(b => { b.onclick = () => runStop({ kind: b.dataset.runStop }); });
$('runAll').onclick = () => runStop({ kind: 'all' });

export { LOCK_SVG, assignMenuCue, fillPoolSelect, fillScanSelect, loadPools, loadScanPresets,
  menuCue, menuSlot, openCueMenu, pollRunning, poolName, scanRev, showRunCount };
