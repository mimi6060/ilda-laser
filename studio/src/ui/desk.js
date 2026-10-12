// Laser Studio — desk.js (T-435).
// Sections (as in the former index.html script):
//   PUPITRE (T-427)
//   play
//   edit
//   full screen
//   wiring
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './layers.js';
import { $, api, apiJson, controlLatest, hex, post, rgb } from './core.js';
import { afterEscape, emergencyStop } from './laser.js';
import { LOCKED_FR, activeWs, flash, remember, remembered, showMode } from './workspaces.js';
import { controlDefaults, controlKinds } from './direct.js';
import { cueDown, cueUp, presetById, presets } from './cues.js';
import { MAC, macRun } from './macros.js';
import { controlLabels, esc, labelOf, learnMode } from './midi.js';

// ---------- PUPITRE (T-427) ----------
// Tabs of components the operator lays out on a grid. « Modifier » adds,
// moves, resizes, binds and deletes (nothing acts); « Jouer » acts and
// nothing moves. Components drive the studio like the rest of the page:
// controls through POST /api/control (clamped, refused like a
// controller's: never `transport.arm`), cues through POST /api/cue with
// their click mode. The layout is an edit (history, locked in show
// mode); playing is not. Values come back from the server in every state
// (`desk.values`), so a fader moved by MIDI moves here too.
const DESK_KIND_FR = { button: 'Bouton', fader: 'Curseur', xy_pad: 'Pavé XY', label: 'Étiquette', preview: 'Aperçu' };
const DESK_SIZE = { button: [2, 1], fader: [1, 4], xy_pad: [3, 3], label: [3, 1], preview: [3, 3] };
const DESK_MAX_TABS = 8, DESK_MAX_ITEMS = 96, DESK_TAB_KEY = 'laserStudio.deskTab';
const DESK = { tabs: null, rev: -1, loading: null, tab: 0, edit: false, sel: null, values: {}, missing: new Set(), active: new Set(),
  regs: null, key: '', ptr: new Map(), local: new Map(), full: false, ownExit: false, raf: 0 };
// Never on a desk (desk.rs `forbidden` checks the same): arming, and
// safety controls other than the emergency stop.
const deskForbidden = id => id === 'transport.arm' || (id.startsWith('safety.') && id !== 'safety.estop');
const deskTab = () => DESK.tabs?.[DESK.tab] || null;
const deskBindKey = b => !b || b.type === 'none' ? null : `${b.type}:${b.id}`;
async function loadDeskControls() {
  if (DESK.regs) return DESK.regs;
  try {
    const list = await apiJson('GET', '/api/controls', undefined, { quiet: true });
    DESK.regs = new Map((list.controls || list).map(c => [c.id, c]));
  } catch { DESK.regs = new Map(); }
  return DESK.regs;
}
async function loadDesks() {
  let r;
  try { r = await apiJson('GET', '/api/desks', undefined, { quiet: true }); } catch { return; }
  takeDesks(r);
}
function takeDesks(r) {
  DESK.tabs = r.tabs || []; DESK.rev = r.rev;
  if (!DESK.loaded) { DESK.loaded = true; DESK.tab = +(remembered(DESK_TAB_KEY) ?? 0) || 0; }
  DESK.tab = Math.max(0, Math.min(DESK.tabs.length - 1, DESK.tab));
  if (DESK.sel != null && !deskTab()?.items.some(i => i.id === DESK.sel)) DESK.sel = null;
  DESK.key = '';
  buildDesk();
}
// From every state: the layout's revision, bound values, missing bindings.
function showDesk(f) {
  const d = f.desk;
  if (!d) return;
  if (d.rev !== DESK.rev && DESK.loading !== d.rev) { DESK.loading = d.rev; loadDesks(); }
  DESK.values = d.values || {};
  const miss = (d.missing || []).join('\n');
  if (miss !== DESK.missKey) { DESK.missKey = miss; DESK.missing = new Set(d.missing || []); DESK.key = ''; }
  DESK.active = new Set([...(f.cues?.active || []).map(a => a.cue), ...(f.cues?.pending || []).map(p => p.cue)]);
  if (activeWs === 'desk') { buildDesk(); paintDesk(); }
}
// Whole layout to the server (validated there); the answer is the truth.
async function saveDesks(tabs, msg) {
  const r = await api('POST', '/api/desks', { tabs }, { quiet: [400] });
  if (!r.ok) {
    const text = r.status === 400 ? await r.text() : '';
    if (text) { if (msg) $('deskMsg').textContent = text; else flash(text); }
    DESK.key = ''; buildDesk();
    return false;
  }
  $('deskMsg').textContent = '';
  takeDesks(await r.json());
  return true;
}
const deskCopy = () => JSON.parse(JSON.stringify(DESK.tabs || []));
// What a binding shows: the label, else the control's or the cue's name.
function deskBindName(b) {
  if (!b || b.type === 'none') return '';
  if (b.type === 'cue') return presetById.get(b.id)?.name || b.id;
  if (b.type === 'macro') return MAC.list.find(m => m.id === b.id)?.name || 'Macro ' + b.id;
  return labelOf(b.id);
}
const deskMissing = b => !!b && b.type !== 'none' && DESK.missing.has(deskBindKey(b));
function deskItemName(it) {
  return it.label || deskBindName(it.binding) || (it.kind === 'label' || it.kind === 'preview' ? '' : DESK_KIND_FR[it.kind]);
}
// A control's value as 0..1 (its registry range), or null.
function deskNorm(id) {
  const k = controlKinds[id], v = DESK.values[id];
  if (!k || k.type !== 'continuous' || typeof v !== 'number') return null;
  return k.max > k.min ? Math.min(1, Math.max(0, (v - k.min) / (k.max - k.min))) : 0;
}
function buildDesk() {
  if (!DESK.tabs) return;
  const tab = deskTab(), edit = DESK.edit && !showMode;
  const key = JSON.stringify([DESK.tabs.map(t => t.name), DESK.tab, tab, edit, DESK.sel, presets.length, DESK.missKey, Object.keys(controlLabels).length]);
  if (key === DESK.key) return;
  DESK.key = key;
  $('deskTabs').replaceChildren(...DESK.tabs.map((t, i) => {
    const b = document.createElement('button');
    b.setAttribute('role', 'tab');
    b.setAttribute('aria-selected', String(i === DESK.tab));
    b.classList.toggle('active', i === DESK.tab);
    b.dataset.deskTab = i;
    b.textContent = t.name;
    return b;
  }));
  $('deskMode').setAttribute('aria-pressed', String(edit));
  $('deskMode').textContent = edit ? 'Jouer' : 'Modifier';
  $('deskMode').title = edit ? 'Jouer : les composants agissent, rien ne bouge' : 'Modifier : ajouter, déplacer, redimensionner, lier et supprimer des composants (ils n\'agissent pas)';
  document.querySelectorAll('#deskRegion .desk-edit').forEach(el => { el.hidden = !edit; });
  $('deskTabAdd').disabled = DESK.tabs.length >= DESK_MAX_TABS;
  $('deskTabRename').disabled = $('deskTabDel').disabled = $('deskTabName').disabled = !tab;
  if (document.activeElement !== $('deskTabName')) $('deskTabName').value = tab?.name || '';
  const grid = $('deskGrid');
  grid.classList.toggle('editing', edit);
  grid.style.setProperty('--cols', tab?.cols || 12);
  grid.style.setProperty('--rows', tab?.rows || 8);
  grid.replaceChildren(...(tab?.items || []).map(it => deskItemEl(it, edit)));
  $('deskEmpty').hidden = !!tab?.items.length;
  $('deskEmpty').textContent = tab ? 'Aucun composant. Passez en mode Modifier et glissez un bouton ou un curseur.'
    : 'Aucun pupitre. Passez en mode Modifier, ajoutez un onglet, puis glissez un bouton ou un curseur.';
  showDeskInspector();
  paintDesk();
}
function deskItemEl(it, edit) {
  const b = it.binding || { type: 'none' }, missing = deskMissing(b) || (it.kind === 'xy_pad' && deskMissing(it.binding_y));
  const unbound = (it.kind === 'button' || it.kind === 'fader' || it.kind === 'xy_pad') && b.type === 'none';
  const el = document.createElement(!edit && it.kind === 'button' ? 'button' : 'div');
  el.className = `desk-item k-${it.kind}` + (missing ? ' missing' : '') + (unbound ? ' unbound' : '') + (it.kind === 'fader' && !it.vertical ? ' horizontal' : '');
  el.dataset.item = it.id;
  el.style.gridColumn = `${it.x + 1} / span ${it.w}`;
  el.style.gridRow = `${it.y + 1} / span ${it.h}`;
  if (it.color) el.style.setProperty('--dc', `rgb(${it.color.join(',')})`);
  const name = deskItemName(it);
  if (it.kind === 'fader') el.insertAdjacentHTML('beforeend', '<span class="fill"></span>');
  if (it.kind === 'xy_pad') el.insertAdjacentHTML('beforeend', '<span class="dot"></span>');
  if (it.kind === 'preview') el.insertAdjacentHTML('beforeend', '<canvas width="300" height="300"></canvas>');
  if (it.kind !== 'preview') {
    const lab = document.createElement('span');
    lab.className = 'lab';
    lab.textContent = name;
    el.appendChild(lab);
  }
  if (missing) el.insertAdjacentHTML('beforeend', '<span class="miss">Liaison manquante</span>');
  const where = `colonne ${it.x + 1}, rangée ${it.y + 1}`;
  if (edit) {
    el.tabIndex = 0;
    el.setAttribute('role', 'button');
    el.setAttribute('aria-pressed', String(DESK.sel === it.id));
    el.setAttribute('aria-label', `${DESK_KIND_FR[it.kind]}${name ? ' « ' + name + ' »' : ''}, ${where}, ${it.w} × ${it.h}`);
    el.classList.toggle('sel', DESK.sel === it.id);
    el.insertAdjacentHTML('beforeend', '<span class="desk-resize" aria-hidden="true"></span>');
    return el;
  }
  // Play: each component carries its control (right click: « Apprendre MIDI »).
  if (b.type === 'control' && !missing) el.dataset.control = b.id;
  const what = missing ? 'liaison manquante' : unbound ? 'non lié' : '';
  switch (it.kind) {
    case 'button':
      el.setAttribute('aria-label', (name || 'Bouton') + (what ? ` (${what})` : ''));
      if (missing || unbound) el.setAttribute('aria-disabled', 'true');
      el.title = missing ? 'Liaison manquante' : b.type === 'control' ? b.id : b.type === 'cue' ? 'Cue ' + name : '';
      break;
    case 'fader':
      el.tabIndex = 0;
      el.setAttribute('role', 'slider');
      el.setAttribute('aria-label', (name || 'Curseur') + (what ? ` (${what})` : ''));
      el.setAttribute('aria-orientation', it.vertical ? 'vertical' : 'horizontal');
      el.setAttribute('aria-valuemin', '0'); el.setAttribute('aria-valuemax', '100'); el.setAttribute('aria-valuenow', '0');
      el.title = missing ? 'Liaison manquante' : b.id || '';
      break;
    case 'xy_pad': {
      el.tabIndex = 0;
      el.setAttribute('role', 'group');
      const y = it.binding_y || { type: 'none' };
      el.setAttribute('aria-label', `Pavé XY ${name}`.trim() + ` : X ${deskBindName(b) || '—'}, Y ${deskBindName(y) || '—'}` + (what ? ` (${what})` : ''));
      if (y.type === 'control') el.dataset.controlY = y.id;
      el.title = missing ? 'Liaison manquante' : `X : ${b.id || '—'} · Y : ${y.id || '—'}${it.spring ? ' · retour au centre' : ''}`;
      break;
    }
    case 'preview': el.setAttribute('role', 'img'); el.setAttribute('aria-label', 'Aperçu de la sortie'); break;
    default: if (!name) el.setAttribute('aria-hidden', 'true');
  }
  return el;
}
// Values on the existing elements (60/s): no rebuild.
function paintDesk() {
  if (DESK.edit && !showMode) return;
  const tab = deskTab();
  if (!tab) return;
  for (const el of $('deskGrid').children) {
    const it = tab.items.find(i => i.id === +el.dataset.item);
    if (!it || el.classList.contains('missing')) continue;
    const b = it.binding || {};
    if (it.kind === 'button') {
      let on = false;
      if (b.type === 'cue') on = DESK.active.has(b.id);
      else if (b.type === 'control') on = DESK.values[b.id] === true || DESK.ptr.has(it.id);
      el.classList.toggle('on', on);
      if (b.type === 'control' && controlKinds[b.id]?.type === 'toggle') el.setAttribute('aria-pressed', String(on));
    } else if (it.kind === 'fader' && b.type === 'control') {
      const n = DESK.local.get(it.id) ?? deskNorm(b.id) ?? 0;
      const fill = el.querySelector('.fill');
      if (it.vertical) fill.style.height = (n * 100) + '%'; else fill.style.width = (n * 100) + '%';
      el.setAttribute('aria-valuenow', String(Math.round(n * 100)));
    } else if (it.kind === 'xy_pad') {
      const loc = DESK.local.get(it.id);
      const nx = loc ? loc[0] : (b.type === 'control' ? deskNorm(b.id) : null) ?? 0.5;
      const ny = loc ? loc[1] : (it.binding_y?.type === 'control' ? deskNorm(it.binding_y.id) : null) ?? 0.5;
      const dot = el.querySelector('.dot');
      dot.style.left = (nx * 100) + '%';
      dot.style.top = ((1 - ny) * 100) + '%';
    }
  }
}
// Previews: the main preview's pixels, drawn the same way, while on screen.
function deskPreviewLoop() {
  DESK.raf = 0;
  if (activeWs !== 'desk') return;
  const src = $('preview');
  document.querySelectorAll('#deskGrid .k-preview canvas').forEach(c => {
    const s = Math.max(60, Math.round(Math.min(c.clientWidth, c.clientHeight) * (devicePixelRatio || 1)));
    if (c.width !== s) { c.width = s; c.height = s; }
    const g = c.getContext('2d');
    g.fillStyle = '#000'; g.fillRect(0, 0, s, s);
    try { g.drawImage(src, 0, 0, s, s); } catch { /* not drawn yet */ }
  });
  DESK.raf = requestAnimationFrame(deskPreviewLoop);
}
function deskEnter() {
  if (!DESK.tabs) loadDesks();
  loadDeskControls();
  DESK.key = '';
  buildDesk();
  if (!DESK.raf) DESK.raf = requestAnimationFrame(deskPreviewLoop);
}
function deskLeave() {
  deskReleaseAll();
  if (DESK.full) deskExitFull();
}

// ----- play -----
// A fader or pad write, latest wins per control (T-484); the spring back
// to the centre goes the same way, after the drag's last value. Buttons
// (taps, holds, « Noir ») post directly: never coalesced.
function deskSend(id, norm) {
  if (norm != null) controlLatest({ id, norm });
}
function deskSendNow(id, value) {
  return post('/api/control', { id, value });
}
function deskPointerNorm(el, it, e) {
  const r = el.getBoundingClientRect();
  const nx = Math.min(1, Math.max(0, (e.clientX - r.left) / Math.max(1, r.width)));
  const ny = Math.min(1, Math.max(0, 1 - (e.clientY - r.top) / Math.max(1, r.height)));
  return it.kind === 'fader' ? (it.vertical ? ny : nx) : [nx, ny];
}
function deskPlayDown(e, el, it) {
  const b = it.binding || {};
  if (el.classList.contains('missing') || b.type === 'none' || !b.type) return;
  // One pointer per component (multi-touch: one finger on each).
  if ([...DESK.ptr.values()].some(p => p.it.id === it.id)) return;
  try { el.setPointerCapture(e.pointerId); } catch { /* synthetic pointer */ }
  DESK.ptr.set(e.pointerId, { it, el });
  e.preventDefault();
  if (it.kind === 'button') {
    if (b.type === 'cue') cueDown(b.id, null);
    // A macro (T-425): launched (again from its first step) on the press.
    else if (b.type === 'macro') macRun(b.id, 'ui');
    else if (b.type === 'control') {
      const k = controlKinds[b.id]?.type;
      if (k === 'toggle') deskSendNow(b.id, DESK.values[b.id] !== true);
      else deskSendNow(b.id, 1);
    }
    paintDesk();
    return;
  }
  deskPlayMove(e);
}
function deskPlayMove(e) {
  const p = DESK.ptr.get(e.pointerId);
  if (!p || p.it.kind === 'button') return;
  const { it, el } = p, n = deskPointerNorm(el, it, e);
  DESK.local.set(it.id, n);
  if (it.kind === 'fader') deskSend(it.binding.id, n);
  else {
    if (it.binding?.type === 'control') deskSend(it.binding.id, n[0]);
    if (it.binding_y?.type === 'control') deskSend(it.binding_y.id, n[1]);
  }
  paintDesk();
}
function deskPlayUp(e) {
  const p = DESK.ptr.get(e.pointerId);
  if (!p) return;
  DESK.ptr.delete(e.pointerId);
  const { it } = p, b = it.binding || {};
  if (it.kind === 'button') {
    if (b.type === 'cue') cueUp(b.id);
    else if (b.type === 'control' && controlKinds[b.id]?.type === 'momentary') deskSendNow(b.id, 0);
  } else if (it.kind === 'xy_pad' && it.spring) {
    // « Retour au centre »: both controls back to their registry default.
    for (const bb of [it.binding, it.binding_y]) {
      if (bb?.type === 'control' && typeof controlDefaults[bb.id] === 'number') controlLatest({ id: bb.id, value: controlDefaults[bb.id] });
    }
  }
  // The server's value takes over once it has the last write.
  setTimeout(() => { DESK.local.delete(it.id); paintDesk(); }, 250);
  paintDesk();
}
function deskReleaseAll() {
  for (const id of [...DESK.ptr.keys()]) deskPlayUp({ pointerId: id });
}
// Arrow keys on a focused fader or pad: 5 % steps (Espace and Entrée keep their roles).
function deskKeyStep(e) {
  const el = e.target.closest?.('#deskGrid .desk-item');
  if (!el || (DESK.edit && !showMode)) return;
  const it = deskTab()?.items.find(i => i.id === +el.dataset.item);
  const d = { ArrowUp: [0, 1], ArrowDown: [0, -1], ArrowRight: [1, 0], ArrowLeft: [-1, 0] }[e.key];
  if (!it || !d || el.classList.contains('missing')) return;
  const step = (id, dir) => { const n = deskNorm(id); if (n != null && dir) post('/api/control', { id, norm: Math.min(1, Math.max(0, n + dir * 0.05)) }); };
  if (it.kind === 'fader' && it.binding?.type === 'control') step(it.binding.id, it.vertical ? d[1] : d[0]);
  else if (it.kind === 'xy_pad') {
    if (it.binding?.type === 'control') step(it.binding.id, d[0]);
    if (it.binding_y?.type === 'control') step(it.binding_y.id, d[1]);
  } else return;
  e.preventDefault();
}

// ----- edit -----
const deskFits = (tab, r, skip) => r.x >= 0 && r.y >= 0 && r.w >= 1 && r.h >= 1 && r.x + r.w <= tab.cols && r.y + r.h <= tab.rows
  && !tab.items.some(o => o.id !== skip && r.x < o.x + o.w && o.x < r.x + r.w && r.y < o.y + o.h && o.y < r.y + r.h);
function deskFreeSpot(tab, w, h) {
  for (const [ww, hh] of [[w, h], [1, 1]]) {
    for (let y = 0; y + hh <= tab.rows; y++) for (let x = 0; x + ww <= tab.cols; x++) {
      if (deskFits(tab, { x, y, w: ww, h: hh })) return { x, y, w: ww, h: hh };
    }
  }
  return null;
}
function deskCell(e) {
  const tab = deskTab(), r = $('deskGrid').getBoundingClientRect();
  if (!tab || e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) return null;
  return { x: Math.min(tab.cols - 1, Math.floor((e.clientX - r.left) / (r.width / tab.cols))),
    y: Math.min(tab.rows - 1, Math.floor((e.clientY - r.top) / (r.height / tab.rows))) };
}
async function deskAdd(kind, cell) {
  if (showMode) return flash(LOCKED_FR);
  const tabs = deskCopy();
  if (!tabs.length) tabs.push({ name: 'Pupitre 1', cols: 12, rows: 8, items: [] });
  const tab = tabs[Math.min(DESK.tab, tabs.length - 1)];
  if (tab.items.length >= DESK_MAX_ITEMS) return flash(`${DESK_MAX_ITEMS} composants au plus par onglet`);
  const [w, h] = DESK_SIZE[kind];
  let spot = cell && deskFits(tab, { x: cell.x, y: cell.y, w, h }) ? { x: cell.x, y: cell.y, w, h } : null;
  if (!spot && cell) {
    const ww = Math.min(w, tab.cols - cell.x), hh = Math.min(h, tab.rows - cell.y);
    if (deskFits(tab, { x: cell.x, y: cell.y, w: ww, h: hh })) spot = { x: cell.x, y: cell.y, w: ww, h: hh };
  }
  spot = spot || deskFreeSpot(tab, w, h);
  if (!spot) return flash('Plus de place sur cet onglet');
  const id = 1 + Math.max(0, ...tabs.flatMap(t => t.items.map(i => i.id)));
  tab.items.push({ id, kind, ...spot, label: '', color: null, binding: { type: 'none' }, binding_y: { type: 'none' }, spring: false, vertical: true });
  if (await saveDesks(tabs)) { DESK.sel = id; DESK.key = ''; buildDesk(); }
}
async function deskChange(fn) {
  const tabs = deskCopy(), tab = tabs[DESK.tab], it = tab?.items.find(i => i.id === DESK.sel);
  if (!it) return false;
  fn(it, tab);
  return saveDesks(tabs, true);
}
async function deskDelete() {
  if (DESK.sel == null || showMode) return;
  const tabs = deskCopy(), tab = tabs[DESK.tab];
  if (!tab) return;
  tab.items = tab.items.filter(i => i.id !== DESK.sel);
  DESK.sel = null;
  await saveDesks(tabs);
}
// Move or resize with the pointer; saved on release, if it fits.
function deskEditDown(e, el, it) {
  DESK.sel = it.id;
  const resize = !!e.target.closest('.desk-resize');
  try { el.setPointerCapture(e.pointerId); } catch { /* synthetic pointer */ }
  const c = deskCell(e) || { x: it.x, y: it.y };
  DESK.drag = { pointer: e.pointerId, it, el, resize, from: c, rect: { x: it.x, y: it.y, w: it.w, h: it.h } };
  document.querySelectorAll('#deskGrid .desk-item.sel').forEach(x => { x.classList.remove('sel'); x.setAttribute('aria-pressed', 'false'); });
  el.classList.add('sel'); el.setAttribute('aria-pressed', 'true');
  showDeskInspector();
  e.preventDefault();
}
function deskEditMove(e) {
  const d = DESK.drag;
  if (!d || d.pointer !== e.pointerId) return;
  const c = deskCell(e), tab = deskTab();
  if (!c || !tab) return;
  const dx = c.x - d.from.x, dy = c.y - d.from.y, it = d.it;
  d.rect = d.resize ? { x: it.x, y: it.y, w: Math.max(1, it.w + dx), h: Math.max(1, it.h + dy) } : { x: it.x + dx, y: it.y + dy, w: it.w, h: it.h };
  d.el.style.gridColumn = `${Math.max(0, Math.min(tab.cols - 1, d.rect.x)) + 1} / span ${Math.max(1, Math.min(d.rect.w, tab.cols))}`;
  d.el.style.gridRow = `${Math.max(0, Math.min(tab.rows - 1, d.rect.y)) + 1} / span ${Math.max(1, Math.min(d.rect.h, tab.rows))}`;
  d.el.classList.toggle('bad', !deskFits(tab, d.rect, it.id));
}
async function deskEditUp(e) {
  const d = DESK.drag;
  if (!d || d.pointer !== e.pointerId) return;
  DESK.drag = null;
  const tab = deskTab(), r = d.rect, it = d.it;
  if (r.x === it.x && r.y === it.y && r.w === it.w && r.h === it.h) return;
  if (!tab || !deskFits(tab, r, it.id)) { flash('Pas de place ici'); DESK.key = ''; buildDesk(); return; }
  await deskChange(x => Object.assign(x, r));
}
function deskBindOptions(list, kind) {
  const regs = DESK.regs || new Map();
  const fits = c => c.external && !deskForbidden(c.id) && (kind === 'button'
    ? ['trigger', 'toggle', 'momentary'].includes(c.kind?.type) : c.kind?.type === 'continuous');
  const opts = [...regs.values()].filter(fits).map(c => `<option value="${esc(c.id)}">${esc(c.label_fr)}</option>`);
  if (kind === 'button') for (const p of presets) opts.push(`<option value="cue:${esc(p.id)}">Cue « ${esc(p.name)} »</option>`);
  if (kind === 'button') for (const m of MAC.list) opts.push(`<option value="macro:${esc(m.id)}">Macro « ${esc(m.name)} »</option>`);
  list.innerHTML = opts.join('');
}
const deskBindText = b => !b || b.type === 'none' ? '' : b.type === 'control' ? b.id : `${b.type}:${b.id}`;
function showDeskInspector() {
  const tab = deskTab(), it = tab?.items.find(i => i.id === DESK.sel), edit = DESK.edit && !showMode;
  // Always there in « Modifier », so the grid keeps its size under the pointer.
  $('deskInsp').hidden = !edit;
  $('deskInspFields').hidden = !it;
  $('deskInspNone').hidden = !!it;
  $('deskInspTitle').textContent = it ? DESK_KIND_FR[it.kind] : 'Composant';
  if (!it || !edit) return;
  if (document.activeElement !== $('deskLabel')) $('deskLabel').value = it.label || '';
  $('deskColorTheme').checked = !it.color;
  $('deskColor').disabled = !it.color;
  if (it.color) $('deskColor').value = hex(it.color);
  const bindable = it.kind === 'button' || it.kind === 'fader' || it.kind === 'xy_pad';
  $('deskBind').hidden = $('deskBindLab').hidden = !bindable;
  $('deskBindLab').textContent = it.kind === 'xy_pad' ? 'Liaison X' : 'Liaison';
  deskBindOptions($('deskBindList'), it.kind === 'button' ? 'button' : 'continuous');
  deskBindOptions($('deskBindListY'), 'continuous');
  if (document.activeElement !== $('deskBind')) $('deskBind').value = deskBindText(it.binding);
  if (document.activeElement !== $('deskBindY')) $('deskBindY').value = deskBindText(it.binding_y);
  document.querySelectorAll('#deskInsp .desk-xy').forEach(el => { el.hidden = it.kind !== 'xy_pad'; });
  document.querySelectorAll('#deskInsp .desk-fader').forEach(el => { el.hidden = it.kind !== 'fader'; });
  $('deskSpring').checked = !!it.spring;
  $('deskOrient').value = it.vertical ? 'v' : 'h';
}
// A typed or picked binding: a control id, `cue:<id>`, or empty. Arming
// and safety are refused here and again by the server.
function deskParseBind(text, kind) {
  const t = text.trim();
  if (!t) return { type: 'none' };
  if (t.startsWith('cue:')) {
    if (kind !== 'button') throw new Error('Seul un bouton se lie à un cue');
    return { type: 'cue', id: t.slice(4) };
  }
  if (t.startsWith('macro:')) {
    if (kind !== 'button') throw new Error('Seul un bouton se lie à une macro');
    return { type: 'macro', id: t.slice(6) };
  }
  if (deskForbidden(t)) throw new Error('Ce contrôle ne peut pas être placé sur un pupitre (armement et sécurité restent hors pupitre)');
  const c = DESK.regs?.get(t.startsWith('live.') ? 'master.' + t.slice(5) : t);
  if (!c) throw new Error('Contrôle inconnu : ' + t);
  if (!c.external) throw new Error('Ce contrôle ne peut pas être placé sur un pupitre');
  const ok = kind === 'button' ? ['trigger', 'toggle', 'momentary'].includes(c.kind?.type) : c.kind?.type === 'continuous';
  if (!ok) throw new Error(kind === 'button' ? 'Un bouton se lie à un déclencheur, une bascule ou un maintien' : 'Un curseur ou un pavé XY se lie à un contrôle continu');
  return { type: 'control', id: t };
}
async function deskSetBind(field, input) {
  const it = deskTab()?.items.find(i => i.id === DESK.sel);
  if (!it) return;
  await loadDeskControls();
  let b;
  try { b = deskParseBind(input.value, field === 'binding' && it.kind === 'button' ? 'button' : 'continuous'); } catch (err) {
    $('deskMsg').textContent = err.message;
    input.value = deskBindText(it[field]);
    return;
  }
  $('deskMsg').textContent = '';
  await deskChange(x => { x[field] = b; });
}

// ----- full screen -----
// The whole page goes full screen, so the top bar (tempo, LASER, ARRÊT)
// stays. Échap is the emergency stop: with the keyboard lock the page
// gets it; without, the browser uses it to leave full screen, so any exit
// we did not ask for stops too.
async function deskEnterFull() {
  const root = document.documentElement;
  try { await root.requestFullscreen(); } catch { flash('Plein écran refusé par le navigateur'); return; }
  try { await navigator.keyboard?.lock?.(['Escape']); } catch { /* not available */ }
  DESK.full = true; DESK.ownExit = false;
  document.body.classList.add('desk-full');
  $('deskFull').setAttribute('aria-pressed', 'true');
  $('deskFull').textContent = 'Quitter le plein écran';
}
function deskExitFull() {
  DESK.ownExit = true;
  try { navigator.keyboard?.unlock?.(); } catch { /* not available */ }
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  deskFullOff();
}
function deskFullOff() {
  DESK.full = false;
  document.body.classList.remove('desk-full');
  $('deskFull').setAttribute('aria-pressed', 'false');
  $('deskFull').textContent = 'Plein écran';
}
document.addEventListener('fullscreenchange', () => {
  if (document.fullscreenElement || !DESK.full) return;
  const ours = DESK.ownExit;
  deskFullOff();
  if (!ours) { emergencyStop('keyboard'); afterEscape(); }
});

// ----- wiring -----
$('deskTabs').addEventListener('click', e => {
  const b = e.target.closest('button[data-desk-tab]');
  if (!b) return;
  deskReleaseAll();
  DESK.tab = +b.dataset.deskTab; DESK.sel = null;
  remember(DESK_TAB_KEY, String(DESK.tab));
  DESK.key = ''; buildDesk();
});
$('deskMode').addEventListener('click', () => {
  if (showMode) return flash(LOCKED_FR);
  deskReleaseAll();
  DESK.edit = !DESK.edit; DESK.sel = null; DESK.key = '';
  if (DESK.edit) loadDeskControls().then(() => { DESK.key = ''; buildDesk(); });
  buildDesk();
});
$('deskFull').addEventListener('click', () => { if (DESK.full) deskExitFull(); else deskEnterFull(); });
$('deskTabAdd').addEventListener('click', async () => {
  const tabs = deskCopy();
  if (tabs.length >= DESK_MAX_TABS) return;
  let n = tabs.length + 1;
  while (tabs.some(t => t.name === `Pupitre ${n}`)) n++;
  tabs.push({ name: `Pupitre ${n}`, cols: 12, rows: 8, items: [] });
  if (await saveDesks(tabs)) { DESK.tab = tabs.length - 1; remember(DESK_TAB_KEY, String(DESK.tab)); DESK.key = ''; buildDesk(); }
});
async function deskRename() {
  const tabs = deskCopy(), name = $('deskTabName').value.trim();
  if (!tabs[DESK.tab] || !name || name === tabs[DESK.tab].name) return;
  tabs[DESK.tab].name = name.slice(0, 32);
  await saveDesks(tabs);
}
$('deskTabName').addEventListener('change', deskRename);
$('deskTabRename').addEventListener('click', () => {
  if ($('deskTabName').value.trim() && $('deskTabName').value.trim() !== deskTab()?.name) deskRename();
  else { $('deskTabName').focus(); $('deskTabName').select(); }
});
$('deskTabDel').addEventListener('click', async () => {
  const tabs = deskCopy();
  if (!tabs[DESK.tab]) return;
  tabs.splice(DESK.tab, 1);
  DESK.sel = null;
  if (await saveDesks(tabs)) { DESK.tab = Math.max(0, DESK.tab - 1); DESK.key = ''; buildDesk(); flash('Onglet supprimé (Annuler le rétablit)'); }
});
// The palette: drag onto a cell, or click for the first free place.
$('deskPalette').addEventListener('pointerdown', e => {
  const b = e.target.closest('button[data-add]');
  if (!b || e.button !== 0) return;
  e.preventDefault();
  try { b.setPointerCapture(e.pointerId); } catch { /* synthetic pointer */ }
  DESK.palette = { kind: b.dataset.add, x: e.clientX, y: e.clientY, pointer: e.pointerId, ghost: null };
});
$('deskPalette').addEventListener('pointermove', e => {
  const p = DESK.palette;
  if (!p || p.pointer !== e.pointerId) return;
  if (!p.ghost && Math.hypot(e.clientX - p.x, e.clientY - p.y) > 6) {
    p.ghost = document.createElement('div');
    p.ghost.className = 'desk-ghost';
    p.ghost.textContent = DESK_KIND_FR[p.kind];
    document.body.appendChild(p.ghost);
  }
  if (p.ghost) { p.ghost.style.left = (e.clientX + 8) + 'px'; p.ghost.style.top = (e.clientY + 8) + 'px'; }
});
['pointerup', 'pointercancel'].forEach(t => $('deskPalette').addEventListener(t, e => {
  const p = DESK.palette;
  if (!p || p.pointer !== e.pointerId) return;
  DESK.palette = null;
  p.ghost?.remove();
  if (t === 'pointercancel') return;
  if (!p.ghost) return deskAdd(p.kind, null);
  const cell = deskCell(e);
  if (cell) deskAdd(p.kind, cell);
}));
$('deskPalette').addEventListener('keydown', e => {
  const b = e.target.closest('button[data-add]');
  if (b && e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); deskAdd(b.dataset.add, null); }
});
$('deskGrid').addEventListener('pointerdown', e => {
  const el = e.target.closest('.desk-item');
  if (!el || e.button !== 0 || learnMode) return;
  const it = deskTab()?.items.find(i => i.id === +el.dataset.item);
  if (!it) return;
  if (DESK.edit && !showMode) deskEditDown(e, el, it); else deskPlayDown(e, el, it);
});
$('deskGrid').addEventListener('pointermove', e => { if (DESK.drag) deskEditMove(e); else deskPlayMove(e); });
['pointerup', 'pointercancel', 'lostpointercapture'].forEach(t => $('deskGrid').addEventListener(t, e => {
  if (DESK.drag) { if (t !== 'lostpointercapture') deskEditUp(e); return; }
  deskPlayUp(e);
}));
$('deskGrid').addEventListener('pointerdown', e => {
  // A click on an empty spot of the grid in « Modifier » clears the selection.
  if (DESK.edit && !e.target.closest('.desk-item') && DESK.sel != null) { DESK.sel = null; DESK.key = ''; buildDesk(); }
});
$('deskGrid').addEventListener('keydown', deskKeyStep);
// Edit mode: a click selects (keyboard: Entrée on a focused component).
$('deskGrid').addEventListener('keydown', e => {
  const el = e.target.closest('.desk-item');
  if (!el || !DESK.edit || e.key !== 'Enter') return;
  e.preventDefault(); e.stopPropagation();
  DESK.sel = +el.dataset.item; DESK.key = ''; buildDesk();
});
$('deskLabel').addEventListener('change', e => deskChange(x => { x.label = e.target.value.trim().slice(0, 32); }));
$('deskColorTheme').addEventListener('change', e => deskChange(x => { x.color = e.target.checked ? null : rgb($('deskColor').value); }));
$('deskColor').addEventListener('change', e => deskChange(x => { x.color = rgb(e.target.value); }));
$('deskBind').addEventListener('change', () => deskSetBind('binding', $('deskBind')));
$('deskBindY').addEventListener('change', () => deskSetBind('binding_y', $('deskBindY')));
$('deskSpring').addEventListener('change', e => deskChange(x => { x.spring = e.target.checked; }));
$('deskOrient').addEventListener('change', e => deskChange((x, tab) => {
  x.vertical = e.target.value === 'v';
  // A fader turned on its side keeps its area, rotated, when that fits.
  const turned = { x: x.x, y: x.y, w: x.h, h: x.w };
  if (((x.vertical && x.w > x.h) || (!x.vertical && x.h > x.w)) && deskFits(tab, turned, x.id)) [x.w, x.h] = [x.h, x.w];
}));
$('deskDel').addEventListener('click', deskDelete);

export { DESK, buildDesk, deskDelete, deskEnter, deskLeave, deskReleaseAll, showDesk };
