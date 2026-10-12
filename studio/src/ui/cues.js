// Laser Studio — cues.js (T-435).
// Sections (as in the former index.html script):
//   cues
//   Filtre de grille (T-430)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './tempo.js';
import { $, api, apiJson, assignLinkedCue, hex, linkedCue, loadLook, lookTask, post } from './core.js';
import { shiftMod } from './presence.js';
import { tempoNow } from './state.js';
import { prepMode, prepareCue, prepared } from './view3d.js';
import { CONFIRM_HOLD_MS, LOCKED_FR, flash, holdToRun, remember, remembered, showMode, winSend } from './workspaces.js';
import { LOCK_SVG, loadPools, loadScanPresets, openCueMenu, poolName } from './presets.js';
import { pgSync } from './program.js';

// ---------- cues ----------
// Cue keys by physical position (e.code), labelled as on an AZERTY Mac:
// the key under the same finger plays the same cell on AZERTY and on
// QWERTY (T-272). The first 26 cells of a page have a key.
const CUE_KEYS = 'azertyuiopqsdfghjklmwxcvbn'.split('');
const CUE_CODES = ['KeyQ', 'KeyW', 'KeyE', 'KeyR', 'KeyT', 'KeyY', 'KeyU', 'KeyI', 'KeyO', 'KeyP',
  'KeyA', 'KeyS', 'KeyD', 'KeyF', 'KeyG', 'KeyH', 'KeyJ', 'KeyK', 'KeyL', 'Semicolon',
  'KeyZ', 'KeyX', 'KeyC', 'KeyV', 'KeyB', 'KeyN'];
// Page keys: F5–F12 = pages 1–8 (F1–F4 are the workspaces), Alt+1…9 = pages 1–9.
const PAGE_FKEYS = ['F5', 'F6', 'F7', 'F8', 'F9', 'F10', 'F11', 'F12'];
let presets = [], presetById = new Map(), cueCategories = [], serverPage = null;
function assignServerPage(v) { return (serverPage = v); }
// The fixed grid (GET /api/grid): size and pages as shown; `gridPage` is
// the page on screen. `gridEditing`: « Réorganiser », cells are dragged
// instead of played.
let grid = { cols: 8, rows: 5, pages: [], rev: -1 }, gridPage = 0, gridEditing = false, dragFrom = null;
function assignGridPage(v) { return (gridPage = v); }
let gridFocus = 0; // the grid's keyboard cell (T-436)
// What the server says plays (from /api/frame), and the grid's cue properties.
let deck = { active: [], shown: [], ending: [], pending: [], click_mode: 'toggle', multi: false, max_active: 4 }, deckKey = '', slots = {};
// Referenced presets (T-422): `GET /api/pools`, the one chosen in the panel.
let pools = { presets: [], uses: {} }, poolSel = null, poolRev = 0;
function assignPoolRev(v) { return (poolRev = v); }
function assignPoolSel(v) { return (poolSel = v); }
function assignPools(v) { return (pools = v); }
// T-158: the grid bar's transition lengths, in the server's order (cue.transition_length).
const TRANS_LENGTHS = [{ beats: 0.25 }, { beats: 0.5 }, { beats: 1 }, { beats: 2 }, { beats: 4 }, { seconds: 0.5 }, { seconds: 1 }, { seconds: 2 }, { seconds: 4 }];
const TRANS_KINDS = ['cut', 'fade', 'fade_through_black', 'morph'];
const lengthIndex = l => TRANS_LENGTHS.findIndex(x => l && (x.beats === l.beats && x.beats !== undefined || x.seconds === l.seconds && x.seconds !== undefined));
const heldKeys = new Map(); // key code -> cue id, for keyup
let pointerCue = null;      // cue id under a held mouse button / finger
function assignPointerCue(v) { return (pointerCue = v); }
const MODE_TAGS = { toggle: 'B', flash: 'F', solo: 'S', restart: 'R' };
// The « Festival » page's moments of a track (T-110), as the dot's tooltip.
const TRACK_TAGS = { intro: 'Intro', groove: 'Groove', break: 'Break', build: 'Montée', drop: 'Drop' };

// Theme (T-299): a colour per cue category and a small picture per cue,
// like a desk's coloured pools. The pictures come once per catalogue load.
const PAGE_COLORS = ['#22e3ff', '#8b5cf6', '#ff3dc8', '#3d9bff', '#2ee6a6', '#ffb020', '#ff5d6e', '#c4f03d', '#ff8a3d'];
const catColor = c => PAGE_COLORS[Math.max(0, cueCategories.indexOf(c)) % PAGE_COLORS.length];
// A page takes the colour of its first cue's category.
function pageColorOf(pg, i) {
  const p = presetById.get((pg && pg.cells.find(Boolean)) || '');
  return p ? catColor(p.category) : PAGE_COLORS[i % PAGE_COLORS.length];
}
let thumbs = {};
async function loadPresets() {
  const r = await apiJson('GET', '/api/presets');
  presets = r.presets; cueCategories = r.categories;
  presetById = new Map(presets.map(p => [p.id, p]));
  await loadGrid(false);
  await loadPools(false);
  await loadSlots();
  loadScanPresets();
  loadThumbs(); // not awaited: the grid works before its pictures arrive
  if (!$('pgEditor').hidden) pgSync(); // the cue editor's lists (T-164)
}
async function loadThumbs() {
  try { thumbs = await apiJson('GET', '/api/presets/thumbs', undefined, { cache: 'no-store' }); } catch { return; }
  cueLoops.clear(); // a cue may have changed: its loop is asked for again
  document.querySelectorAll('#cues .cue').forEach(b => drawThumb(b.querySelector('canvas.th'), thumbs[b.dataset.id]));
  loopsWanted();
}
// A thumbnail is one hex string, 5 bytes per point: x, y (0..255 = -1..1), r, g, b.
function thumbPoints(hex) {
  const out = [];
  for (let i = 0; i + 10 <= hex.length; i += 10) {
    const v = [0, 2, 4, 6, 8].map(k => parseInt(hex.substr(i + k, 2), 16));
    out.push([v[0] / 127.5 - 1, v[1] / 127.5 - 1, v[2] / 255, v[3] / 255, v[4] / 255]);
  }
  return out;
}
function drawThumb(c, hex) {
  if (!c) return;
  c._loopFrame = -1; // a loop (T-273) draws over it again
  const pts = hex ? thumbPoints(hex) : null;
  const g = c.getContext('2d'), W = c.width, H = c.height, S = H * 0.92;
  g.fillStyle = '#000'; g.fillRect(0, 0, W, H);
  if (!pts || !pts.length) return;
  const px = p => [W / 2 + p[0] * S / 2, H / 2 - p[1] * S / 2];
  const col = (p, a) => `rgba(${[2, 3, 4].map(i => Math.round(255 * Math.sqrt(p[i]))).join(',')},${a})`;
  g.lineCap = 'round'; g.lineJoin = 'round';
  for (const [w, a] of [[5, 0.22], [1.6, 1]]) {
    g.lineWidth = w;
    for (let i = 1; i < pts.length; i++) {
      const a0 = pts[i - 1], b0 = pts[i];
      if (a0[2] + a0[3] + a0[4] === 0 || b0[2] + b0[3] + b0[4] === 0) continue;
      const [x1, y1] = px(a0), [x2, y2] = px(b0);
      g.strokeStyle = col(b0, a); g.beginPath(); g.moveTo(x1, y1); g.lineTo(x2 + 0.01, y2); g.stroke();
    }
  }
}
// Hover preview of a cue program (T-157): the pictures of its length
// (GET /api/presets/preview), played in the cue's picture at the tempo.
// Display only, nothing goes to the laser.
let cuePreview = null; // { b, id, timer }
async function playCuePreview(b, id) {
  stopCuePreview();
  const mine = cuePreview = { b, id, timer: 0 };
  let pv;
  try { pv = await apiJson('GET', '/api/presets/preview?id=' + encodeURIComponent(id), undefined, { cache: 'no-store' }); } catch { return; }
  if (cuePreview !== mine || !pv.frames || !pv.frames.length) return;
  const canvas = b.querySelector('canvas.th');
  let i = 0;
  const step = () => {
    if (cuePreview !== mine) return;
    drawThumb(canvas, pv.frames[i]);
    i = (i + 1) % pv.frames.length;
    const bpm = +$('bpm').value || 120;
    mine.timer = setTimeout(step, 1000 * pv.step_beats * 60 / Math.max(20, bpm));
  };
  b.classList.add('previewing');
  step();
}
function stopCuePreview() {
  const p = cuePreview;
  cuePreview = null;
  if (!p) return;
  clearTimeout(p.timer);
  p.b.classList.remove('previewing');
  drawThumb(p.b.querySelector('canvas.th'), thumbs[p.id]);
  loopTick(); // its loop takes over again
}

// Animated thumbnails (T-273): one bar of each cue, 16 small pictures drawn
// once by the studio aside (GET /api/presets/loops, only for the cells on
// screen), looped here at the current tempo, on the beat. One timer for
// the whole grid, which only wakes when a picture changes; nothing runs
// while the tab is hidden, the grid is off screen or the setting says
// « Fixes » / « Aucune ». Display only, nothing goes to the laser.
const THUMB_MODE_KEY = 'laserStudio.thumbs', THUMB_MODES = ['animated', 'static', 'none'];
let thumbMode = THUMB_MODES.includes(remembered(THUMB_MODE_KEY)) ? remembered(THUMB_MODE_KEY) : 'animated';
const cueLoops = new Map();   // cue id -> { step, frames: [[x, y, colour or null]…] }
const loopCells = new Set();  // the grid's canvases on screen
let loopTimer = 0, loopAsking = false;
const loopSeen = typeof IntersectionObserver === 'function' ? new IntersectionObserver(entries => {
  for (const e of entries) {
    if (e.isIntersecting) loopCells.add(e.target); else loopCells.delete(e.target);
  }
  loopsWanted();
}) : null;
// A frame parsed once: pixel positions in a 92 px picture and the colour
// string of each lit point (null = blanked), so a redraw builds nothing.
function loopFrame(hex) {
  const S = 92 * 0.92;
  return thumbPoints(hex).map(p => [46 + p[0] * S / 2, 46 - p[1] * S / 2,
    p[2] + p[3] + p[4] === 0 ? null : `rgb(${[2, 3, 4].map(i => Math.round(255 * Math.sqrt(p[i]))).join(',')})`]);
}
// Without the halo, and one stroke per run of one colour: cheap enough
// for a full page at the tempo.
function drawLoopFrame(c, pts) {
  const g = c.getContext('2d');
  g.fillStyle = '#000'; g.fillRect(0, 0, c.width, c.height);
  g.lineWidth = 1.8; g.lineCap = 'round'; g.lineJoin = 'round';
  let style = null, at = -1;
  for (let i = 1; i < pts.length; i++) {
    const a = pts[i - 1], b = pts[i];
    if (!a[2] || !b[2]) continue;
    if (b[2] !== style) {
      if (style) g.stroke();
      g.strokeStyle = style = b[2]; g.beginPath(); g.moveTo(a[0], a[1]);
    } else if (at !== i - 1) g.moveTo(a[0], a[1]);
    g.lineTo(b[0] + 0.01, b[1]); at = i;
  }
  if (style) g.stroke();
}
function loopsRunning() { return thumbMode === 'animated' && !document.hidden && loopCells.size > 0; }
// Ask for the loops of the cells on screen not drawn yet; the studio
// answers with those it has and draws the others aside (« pending »).
async function loopsWanted() {
  if (!loopsRunning() || loopAsking) { loopTick(); return; }
  const ids = [...new Set([...loopCells].map(c => c.closest('.cue')?.dataset.id).filter(id => id && !cueLoops.has(id)))];
  if (!ids.length) { loopTick(); return; }
  loopAsking = true;
  let r = null;
  try { r = await apiJson('GET', '/api/presets/loops?ids=' + ids.map(encodeURIComponent).join(','), undefined, { cache: 'no-store' }); } catch { /* the still pictures stay */ }
  loopAsking = false;
  if (!r) return;
  for (const [id, l] of Object.entries(r.loops || {})) {
    cueLoops.set(id, l.frames.length ? { step: l.step_beats, frames: l.frames.map(loopFrame) } : null);
  }
  loopTick();
  if (r.pending && r.pending.length) setTimeout(loopsWanted, 300);
}
function loopTick() {
  clearTimeout(loopTimer); loopTimer = 0;
  if (!loopsRunning()) return;
  const t = tempoNow(), bpm = Math.max(20, (t && t.bpm) || +$('bpm').value || 120);
  const beat = t ? t.beat : performance.now() / 1000 * bpm / 60;
  let step = Infinity;
  for (const c of loopCells) {
    const b = c.closest('.cue'), l = b && cueLoops.get(b.dataset.id);
    if (!l || (cuePreview && cuePreview.b === b)) continue;
    const n = l.frames.length, i = ((Math.floor(beat / l.step) % n) + n) % n;
    step = Math.min(step, l.step);
    if (c._loopFrame !== i) { c._loopFrame = i; drawLoopFrame(c, l.frames[i]); }
  }
  if (step === Infinity) return;
  // Wake at the next picture, on the beat grid.
  const next = (Math.floor(beat / step) + 1) * step;
  loopTimer = setTimeout(loopTick, Math.max(15, (next - beat) * 60000 / bpm + 2));
}
// After each grid render: watch the new canvases.
function loopsObserve() {
  loopCells.clear();
  if (!loopSeen) return;
  loopSeen.disconnect();
  if (thumbMode !== 'animated') return;
  document.querySelectorAll('#cues .cue canvas.th').forEach(c => loopSeen.observe(c));
}
document.addEventListener('visibilitychange', () => loopsWanted());
function setThumbMode(mode) {
  thumbMode = THUMB_MODES.includes(mode) ? mode : 'animated';
  remember(THUMB_MODE_KEY, thumbMode);
  $('thumbMode').value = thumbMode;
  renderCues();
}
$('thumbMode').value = thumbMode;
$('thumbMode').addEventListener('change', e => setThumbMode(e.target.value));
async function loadSlots() {
  slots = (await apiJson('GET', '/api/cues')).slots || {};
  try { cueTags = await apiJson('GET', '/api/cues/tags', undefined, { quiet: true }); } catch { /* the bar keeps the last list */ }
  renderCueFilter();
  renderCues();
}

// ---------- Filtre de grille (T-430) ----------
// A display state only: cells that don't match are dimmed, never moved
// nor hidden, and still play from the mouse, the keys and MIDI. Kept in
// this browser (localStorage), never in the project; output and
// controller LEDs don't see it.
const FILTER_KEY = 'laserStudio.cueFilter';
let cueTags = [];
let cueFilter = (() => {
  try {
    const f = JSON.parse(remembered(FILTER_KEY) || '{}') || {};
    return { tags: Array.isArray(f.tags) ? f.tags.filter(t => typeof t === 'string') : [], text: typeof f.text === 'string' ? f.text : '', any: !!f.any };
  } catch { return { tags: [], text: '', any: false }; }
})();
function assignCueFilter(v) { return (cueFilter = v); }
// Tags compare without case or accents (cues.rs `tag_key`).
const tagKey = t => String(t).trim().toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g, '');
const filterOn = () => cueFilter.tags.length > 0 || cueFilter.text.trim() !== '';
function cueMatches(p) {
  const slot = slots[p.id] || {};
  const tags = (slot.tags || []).map(tagKey);
  const want = cueFilter.tags.map(tagKey);
  if (want.length && !(cueFilter.any ? want.some(t => tags.includes(t)) : want.every(t => tags.includes(t)))) return false;
  const q = tagKey(cueFilter.text);
  return !q || [p.name, slot.notes || '', ...(slot.tags || [])].some(x => tagKey(x).includes(q));
}
function setCueFilter(f) {
  cueFilter = { ...cueFilter, ...f };
  remember(FILTER_KEY, JSON.stringify(cueFilter));
  renderCueFilter();
  applyCueFilter();
}
function renderCueFilter() {
  const keys = new Set(cueTags.map(t => tagKey(t.tag)));
  // A ticked tag no cue carries any more still shows, to untick it.
  const list = [...cueTags, ...cueFilter.tags.filter(t => !keys.has(tagKey(t))).map(tag => ({ tag, count: 0 }))];
  const on = new Set(cueFilter.tags.map(tagKey));
  $('cueFilterTags').replaceChildren(...list.map(({ tag, count }) => {
    const b = document.createElement('button');
    b.textContent = tag;
    b.insertAdjacentHTML('beforeend', `<small>${count}</small>`);
    b.dataset.tag = tag;
    b.setAttribute('aria-pressed', String(on.has(tagKey(tag))));
    b.title = `${count} cue${count > 1 ? 's' : ''}`;
    b.onclick = () => {
      const k = tagKey(tag);
      setCueFilter({ tags: on.has(k) ? cueFilter.tags.filter(t => tagKey(t) !== k) : [...cueFilter.tags, tag] });
    };
    return b;
  }));
  if (document.activeElement !== $('cueFilterText')) $('cueFilterText').value = cueFilter.text;
  document.querySelectorAll('#cueFilterMode button').forEach(b => b.classList.toggle('active', (b.dataset.any === '1') === cueFilter.any));
  $('cueTagList').replaceChildren(...cueTags.map(t => { const o = document.createElement('option'); o.value = t.tag; return o; }));
}
function applyCueFilter() {
  const on = filterOn();
  let shown = 0, total = 0;
  document.querySelectorAll('#cues .cue').forEach(el => {
    const p = presetById.get(el.dataset.id);
    if (!p) return;
    total++;
    const match = !on || cueMatches(p);
    if (match) shown++;
    el.classList.toggle('dimmed', !match);
  });
  $('cueFilter').classList.toggle('on', on);
  $('cueFilterCount').textContent = on ? `${shown} / ${total}` : '';
  // The bar opens from « Filtrer » (no row taken from the grid when unused)
  // and stays open while a filter is on.
  if (on) cueFilterOpen = true;
  $('cueFilter').hidden = !cueFilterOpen;
  $('cueFilterBtn').setAttribute('aria-expanded', String(cueFilterOpen));
  $('cueFilterBtn').classList.toggle('on', on);
  $('cueFilterBadge').hidden = !on;
  $('cueFilterBadge').textContent = on ? `${shown} / ${total}` : '';
}
let cueFilterOpen = false;
$('cueFilterBtn').addEventListener('click', () => {
  cueFilterOpen = !cueFilterOpen;
  if (!cueFilterOpen) { $('cueFilterText').value = ''; setCueFilter({ tags: [], text: '' }); }
  applyCueFilter();
  if (cueFilterOpen) $('cueFilterText').focus();
});
$('cueFilterText').addEventListener('input', () => setCueFilter({ text: $('cueFilterText').value }));
$('cueFilterText').addEventListener('keydown', e => { if (e.key === 'Enter') e.target.blur(); });
document.querySelectorAll('#cueFilterMode button').forEach(b => b.addEventListener('click', () => setCueFilter({ any: b.dataset.any === '1' })));
$('cueFilterClear').addEventListener('click', () => { $('cueFilterText').value = ''; setCueFilter({ tags: [], text: '' }); });
async function loadGrid(render = true) {
  try { applyGrid(await apiJson('GET', '/api/grid', undefined, { cache: 'no-store' }), render); } catch { /* next frame retries */ }
}
function applyGrid(g, render = true) {
  grid = g;
  gridPage = Math.min(gridPage, Math.max(0, g.pages.length - 1));
  if (document.activeElement !== $('gridCols')) $('gridCols').value = g.cols;
  if (document.activeElement !== $('gridRows')) $('gridRows').value = g.rows;
  if (render) renderCues();
}
const pageKey = i => i < PAGE_FKEYS.length ? PAGE_FKEYS[i] : i < 9 ? `Alt+${i + 1}` : '';
function selectPage(i) {
  if (!grid.pages[i]) return;
  gridPage = i; renderCues();
  winSend({ type: 'page', page: i });
  post('/api/control', { id: 'page.' + (i + 1) });
}
function renderCues() {
  stopCuePreview(); // its button is about to go
  $('cueTabs').innerHTML = '';
  grid.pages.forEach((pg, i) => {
    const b = document.createElement('button');
    b.setAttribute('role', 'tab');
    b.setAttribute('aria-selected', String(i === gridPage));
    const n = pg.cells.filter(id => id && presetById.has(id)).length + pg.hidden;
    b.innerHTML = '<span class="pn"></span>' + (pageKey(i) ? `<kbd>${pageKey(i)}</kbd>` : '');
    b.querySelector('.pn').textContent = `${pg.name} (${n})`;
    b.title = `page.${i + 1}` + (i < 9 ? ` · ${i < 8 ? PAGE_FKEYS[i] + ' ou ' : ''}Alt+${i + 1}` : '');
    b.classList.toggle('active', i === gridPage);
    b.style.setProperty('--c', pageColorOf(pg, i));
    b.onclick = () => selectPage(i);
    // « Réorganiser »: a cell dropped on a tab goes to that page's first empty cell.
    b.addEventListener('dragover', e => { if (canDropOn(i)) { e.preventDefault(); b.classList.add('drop'); } });
    b.addEventListener('dragleave', () => b.classList.remove('drop'));
    b.addEventListener('drop', e => { e.preventDefault(); b.classList.remove('drop'); dropOn(i, null, e.altKey); });
    // T-405: right click = rename or delete the page.
    b.addEventListener('contextmenu', e => { e.preventDefault(); openPageMenu(i, e.clientX, e.clientY); });
    $('cueTabs').appendChild(b);
  });
  // The tabs scroll sideways: keep the shown page's tab in view (never the window).
  const tabs = $('cueTabs'), act = tabs.querySelector('.active');
  if (act) {
    const t = tabs.getBoundingClientRect(), a = act.getBoundingClientRect();
    if (a.left < t.left) tabs.scrollLeft -= t.left - a.left + 8;
    else if (a.right > t.right) tabs.scrollLeft += a.right - t.right + 8;
  }
  const pg = grid.pages[gridPage] || { name: '', cells: [], hidden: 0, auto: false, editable: false };
  const cues = $('cues');
  // Keyboard (T-436): a re-render keeps the focus on the same cell.
  const hadFocus = cues.contains(document.activeElement);
  cues.innerHTML = '';
  cues.style.setProperty('--cols', grid.cols);
  let row = null;
  cues.classList.toggle('editing', gridEditing);
  cues.classList.toggle('storing', storeMode);
  $('cueRegion').style.setProperty('--cat', pageColorOf(pg, gridPage));
  let any = false;
  pg.cells.forEach((id, i) => {
    const p = id ? presetById.get(id) : null;
    const key = CUE_KEYS[i] ? CUE_KEYS[i].toUpperCase() : '';
    let el;
    if (!p) {
      // An empty cell keeps its place, greyed, and plays nothing.
      el = document.createElement('div');
      el.className = 'cue-empty';
      el.innerHTML = `<span class="key">${key}</span>`;
      el.setAttribute('aria-label', 'Case vide' + (key ? ' ' + key : ''));
      el.addEventListener('pointerdown', e => { if (storeMode && e.button === 0) { e.preventDefault(); storeInto(i); } });
    } else {
      any = true;
      el = document.createElement('button');
      el.className = 'cue';
      el.dataset.id = p.id;
      el.setAttribute('aria-label', p.name);
      el.title = el.dataset.control = `grid.${gridPage + 1}.${Math.floor(i / grid.cols) + 1}.${i % grid.cols + 1}`;
      const slot = slots[p.id] || {};
      // A user cue (T-405) has its own colour band; a colour set on the
      // cell (T-430) wins over both.
      el.style.setProperty('--cat', slot.color ? hex(slot.color) : p.user && p.color ? hex(p.color) : catColor(p.category));
      el.classList.toggle('user', !!p.user);
      // T-422: « ↗ » when the cue refers to presets (their names on hover).
      const refNames = Object.values(slot.refs || {}).filter(Boolean).map(id => poolName(id) || id + ' (inconnu)');
      const tag = [slot.mode ? MODE_TAGS[slot.mode] : '', slot.group ? 'G' + slot.group : '', slot.layer > 1 ? 'C' + slot.layer : '', slot.zones?.length ? 'Z' + slot.zones.join('') : '', slot.transition ? 'T' : '', refNames.length ? '↗' : '']
        .filter(Boolean).join(' ');
      el.innerHTML = `<span class="key">${key}</span>${thumbMode === 'none' ? '' : '<canvas class="th" width="92" height="92"></canvas>'}<span class="n"></span><span class="tag">${tag}</span>`
        + (slot.protect_stop || slot.protect_solo ? LOCK_SVG.replace('<svg', `<svg class="lock"><title>${slot.protect_stop ? 'Protégé de Tout arrêter' : ''}${slot.protect_stop && slot.protect_solo ? ' · ' : ''}${slot.protect_solo ? 'Protégé du solo' : ''}</title`) : '');
      el.querySelector('.n').textContent = p.name;
      if (TRACK_TAGS[p.tag]) {
        const dot = document.createElement('span');
        dot.className = 'ftag'; dot.dataset.tag = p.tag; dot.title = TRACK_TAGS[p.tag];
        el.appendChild(dot);
      }
      if (refNames.length) el.title += '\nPréréglages : ' + refNames.join(', ');
      if (slot.notes || slot.tags?.length) {
        // Annotated (T-430): a dot, and the note and tags on hover.
        const dot = document.createElement('span');
        dot.className = 'note';
        el.appendChild(dot);
        el.title += '\n' + [slot.tags?.length ? '# ' + slot.tags.join(', ') : '', slot.notes || ''].filter(Boolean).join('\n');
      }
      drawThumb(el.querySelector('canvas.th'), thumbs[p.id]);
      if (p.program || slot.program) {
        // Cue program (T-157): a badge, and the picture plays it on hover.
        const badge = document.createElement('span');
        badge.className = 'prog'; badge.textContent = '∿';
        badge.title = 'Cue évolutif : programme (couches, courbes, LFO). Survol : aperçu au tempo.';
        el.appendChild(badge);
        el.classList.add('has-program');
        el.addEventListener('pointerenter', () => playCuePreview(el, p.id));
        el.addEventListener('pointerleave', () => stopCuePreview());
      }
      el.addEventListener('pointerdown', e => {
        if (e.button !== 0) return;
        if (storeMode) { e.preventDefault(); storeInto(i); return; }
        if (gridEditing) return;
        e.preventDefault();
        // « Préparer » (T-274): into the prepared view, never the output.
        if (prepMode || e.altKey) { prepareCue(p.id); return; }
        if (pointerCue) cueUp(pointerCue);
        pointerCue = p.id;
        cueDown(p.id, shiftMod(e) ? 'flash' : null);
      });
      el.addEventListener('contextmenu', e => { e.preventDefault(); openCueMenu(p, e.clientX, e.clientY, i); });
      if (gridEditing && pg.editable) {
        el.draggable = true;
        el.addEventListener('dragstart', e => { dragFrom = { page: gridPage, slot: i }; e.dataTransfer.effectAllowed = 'copyMove'; e.dataTransfer.setData('text/plain', p.id); });
        el.addEventListener('dragend', () => { dragFrom = null; });
      }
    }
    el.dataset.slot = i;
    // role=grid (T-436): rows of `cols` cells (display: contents), one
    // tab stop for the whole grid; the arrows move it (gridMove).
    el.setAttribute('role', 'gridcell');
    el.tabIndex = -1;
    if (i % grid.cols === 0) {
      row = document.createElement('div');
      row.className = 'cue-row';
      row.setAttribute('role', 'row');
      cues.appendChild(row);
    }
    if (gridEditing) {
      el.addEventListener('dragover', e => { if (canDropOn(gridPage)) { e.preventDefault(); el.classList.add('drop'); } });
      el.addEventListener('dragleave', () => el.classList.remove('drop'));
      el.addEventListener('drop', e => { e.preventDefault(); el.classList.remove('drop'); dropOn(gridPage, i, e.altKey); });
    }
    row.appendChild(el);
  });
  gridFocusTo(gridFocus, hadFocus);
  if (any) delete cues.dataset.empty; else cues.dataset.empty = 'Page vide';
  $('gridHidden').textContent = pg.hidden
    ? ` · ${pg.hidden} cue${pg.hidden > 1 ? 's' : ''} de cette page hors grille : agrandir la grille pour ${pg.hidden > 1 ? 'les' : 'le'} voir` : '';
  $('gridPageRename').disabled = $('gridPageDelete').disabled = !pg.editable;
  loopsObserve();
  paintCues();
  applyCueFilter();
}
// Keyboard in the grid (T-436): one tab stop (the last focused cell),
// arrows and Début / Fin move it; `0` plays it (grid.activate).
const gridCells = () => [...$('cues').querySelectorAll('[data-slot]')];
function focusedCell() {
  const el = document.activeElement?.closest?.('#cues [data-slot]');
  return el ? +el.dataset.slot : -1;
}
function gridFocusTo(i, focus = true) {
  const cells = gridCells();
  if (!cells.length) return;
  gridFocus = Math.max(0, Math.min(cells.length - 1, i));
  cells.forEach((c, n) => { c.tabIndex = n === gridFocus ? 0 : -1; });
  if (focus) cells[gridFocus].focus();
}
function gridMove(dir) {
  const n = gridCells().length, cols = grid.cols || 1;
  let i = Math.max(0, focusedCell());
  const start = i - i % cols;
  if (dir === 'left') i -= 1;
  else if (dir === 'right') i += 1;
  else if (dir === 'up') i = i - cols >= 0 ? i - cols : i;
  else if (dir === 'down') i = i + cols < n ? i + cols : i;
  else if (dir === 'home') i = start;
  else if (dir === 'end') i = Math.min(n - 1, start + cols - 1);
  gridFocusTo(i);
}
$('cues').addEventListener('focusin', e => {
  const el = e.target.closest('[data-slot]');
  if (el && +el.dataset.slot !== gridFocus) gridFocusTo(+el.dataset.slot, false);
});
function canDropOn(page) {
  const pg = grid.pages[page];
  return !!(dragFrom && pg && pg.editable && !pg.auto);
}
// Dropped: moved; with Alt, copied (a user cue gets a copy of its own);
// onto a cue, the two swap (T-405).
async function dropOn(page, slot, copy = false) {
  if (!dragFrom) return;
  const from = [dragFrom.page, dragFrom.slot];
  dragFrom = null;
  const onto = slot !== null && grid.pages[page]?.cells[slot];
  const op = copy ? { op: 'copy', from, to: [page, slot] } : onto ? { op: 'swap', from, to: [page, slot] } : { op: 'move', from, to: [page, slot] };
  if (await gridPost('/api/grid/edit', op) && copy) await loadPresets();
}
// Grid edits answer with the new grid; a refusal (full page, show mode…) is shown.
async function gridPost(path, body) {
  const r = await api('POST', path, body, { quiet: true });
  if (!r.ok) {
    const t = await r.text();
    $('gridMsg').textContent = t;
    if (r.status === 423) flash(t);
    else if ($('gridMenu').hidden) storeMsg(t);
    return false;
  }
  $('gridMsg').textContent = '';
  const g = await r.json();
  if (g.page !== undefined) { gridPage = g.page; serverPage = g.page; }
  applyGrid(g);
  return true;
}
$('gridMenuBtn').addEventListener('click', () => {
  const open = $('gridMenu').hidden;
  $('gridMenu').hidden = !open;
  $('gridMenuBtn').setAttribute('aria-expanded', String(open));
  if (!open && gridEditing) setGridEditing(false);
});
function setGridEditing(on) {
  gridEditing = on;
  $('gridEdit').setAttribute('aria-pressed', String(on));
  renderCues();
}
$('gridEdit').addEventListener('click', () => setGridEditing(!gridEditing));
for (const id of ['gridCols', 'gridRows']) {
  $(id).addEventListener('change', () => gridPost('/api/grid', { cols: +$('gridCols').value, rows: +$('gridRows').value }));
}
$('cuePageAdd').addEventListener('click', () => gridPost('/api/grid/pages', { op: 'add' }));

// « Enregistrer dans une case » / « Mettre à jour » (T-405): the current
// look into a user cue. Échap stays the emergency stop: the mode ends by
// clicking its button again, or once a cell is picked.
let storeMode = false, storeCopyFrom = null;
function setStoreMode(on) {
  if (on && showMode) { flash(LOCKED_FR); on = false; }
  storeMode = on;
  $('cueStore').setAttribute('aria-pressed', String(on));
  $('cues').classList.toggle('storing', on);
  if (on) storeMsg('Cliquer la case où enregistrer le look courant (ou recliquer « Enregistrer dans une case » pour annuler).');
  else if (!storeCopyFrom) $('cueStoreMsg').hidden = true;
}
function storeMsg(text, copyFrom = null) {
  storeCopyFrom = copyFrom;
  $('cueStoreText').textContent = text;
  $('cueStoreCopy').hidden = !copyFrom;
  $('cueStoreMsg').hidden = !text;
}
const storeBody = () => ({ filter: $('cueStoreFilter').value, with_master: $('cueStoreMaster').checked });
async function storeInto(i) {
  const pg = grid.pages[gridPage];
  if (!pg || pg.auto) { storeMsg('Cette page suit sa bibliothèque : choisir une autre page.'); return; }
  const p = pg.cells[i] ? presetById.get(pg.cells[i]) : null;
  if (p && p.user && !confirm(`Remplacer « ${p.name} » par le look courant ?`)) return;
  const r = await api('POST', '/api/cues/store', { page: gridPage, slot: i, ...storeBody() }, { quiet: true });
  if (!r.ok) {
    const t = await r.text();
    if (r.status === 423) { flash(t); setStoreMode(false); return; }
    // A built-in cue: refused, with an editable copy offered.
    storeMsg(t, r.status === 409 ? { page: gridPage, slot: i } : null);
    return;
  }
  const g = await r.json();
  storeCopyFrom = null;
  setStoreMode(false);
  await loadPresets();
  storeMsg(`Look enregistré dans « ${presetById.get(g.cue)?.name || g.cue} ».`);
}
async function updateCue() {
  if (showMode) return flash(LOCKED_FR);
  const r = await api('POST', '/api/cues/update', storeBody(), { quiet: true });
  const t = await r.text();
  if (!r.ok) { if (r.status === 423) flash(t); else storeMsg(t); return; }
  await loadPresets();
  storeMsg(`« ${JSON.parse(t).name} » mis à jour avec le look courant.`);
}
$('cueStore').addEventListener('click', () => setStoreMode(!storeMode));
$('cueUpdate').addEventListener('click', updateCue);
$('cueStoreCopy').addEventListener('click', async () => {
  const at = storeCopyFrom;
  if (!at) return;
  storeCopyFrom = null;
  setStoreMode(false);
  if (await gridPost('/api/grid/edit', { op: 'copy_to_user', from: [at.page, at.slot] })) {
    await loadPresets();
    storeMsg(`Copie modifiable créée sur la page « ${grid.pages[gridPage]?.name || ''} ».`);
  }
});

// Right click on a page tab (T-405): rename or delete it.
let pageMenuAt = null;
function openPageMenu(i, x, y) {
  const pg = grid.pages[i];
  if (!pg || !pg.editable) return;
  if (showMode) return flash(LOCKED_FR);
  pageMenuAt = i;
  $('pageMenuName').textContent = pg.name;
  const m = $('pageMenu');
  m.hidden = false;
  m.style.left = Math.max(8, Math.min(x, innerWidth - m.offsetWidth - 8)) + 'px';
  m.style.top = Math.max(8, Math.min(y, innerHeight - m.offsetHeight - 8)) + 'px';
}
$('pageMenuRename').addEventListener('click', async () => {
  $('pageMenu').hidden = true;
  const pg = grid.pages[pageMenuAt];
  if (!pg) return;
  const name = prompt('Nom de la page', pg.name);
  if (name && name.trim()) await gridPost('/api/grid/pages', { op: 'rename', page: pageMenuAt, name });
});
$('pageMenuDelete').addEventListener('click', async () => {
  $('pageMenu').hidden = true;
  const pg = grid.pages[pageMenuAt];
  if (!pg || !confirm(`Supprimer la page « ${pg.name} » ?`)) return;
  if (await gridPost('/api/grid/pages', { op: 'delete', page: pageMenuAt })) await loadPresets();
});
document.addEventListener('pointerdown', e => { if (!$('pageMenu').hidden && !$('pageMenu').contains(e.target)) $('pageMenu').hidden = true; });

$('gridPageNew').addEventListener('click', async () => {
  if (await gridPost('/api/grid/page', { action: 'new', name: $('gridPageName').value })) $('gridPageName').value = '';
});
$('gridPageRename').addEventListener('click', async () => {
  const name = $('gridPageName').value.trim();
  if (!name) { $('gridPageName').focus(); return; }
  if (await gridPost('/api/grid/page', { action: 'rename', page: gridPage, name })) $('gridPageName').value = '';
});
holdToRun($('gridPageDelete'), CONFIRM_HOLD_MS, () => gridPost('/api/grid/page', { action: 'delete', page: gridPage }),
  () => flash('Maintenir pour confirmer'));
// Released anywhere: the button may have been re-rendered (page change)
// while it was held.
['pointerup', 'pointercancel'].forEach(ev => window.addEventListener(ev, () => {
  if (pointerCue) { cueUp(pointerCue); pointerCue = null; }
}));
// Highlight in place: re-rendering would lose the button under a held pointer.
function paintCues() {
  const latched = new Set(deck.active.filter(a => !a.held).map(a => a.cue));
  const held = new Set(deck.active.filter(a => a.held).map(a => a.cue));
  const shown = new Set(deck.shown);
  // T-159: « en attente » until its quantised start; T-158: still fading out.
  const waiting = new Set((deck.pending || []).map(p => p.cue));
  const ending = new Set((deck.ending || []).map(e => e.cue));
  document.querySelectorAll('#cues .cue').forEach(b => {
    const id = b.dataset.id;
    b.classList.toggle('flash', held.has(id));
    b.classList.toggle('active', latched.has(id) && !held.has(id));
    b.classList.toggle('hidden-out', (latched.has(id) || held.has(id)) && !shown.has(id));
    b.classList.toggle('pending', waiting.has(id));
    b.classList.toggle('ending', ending.has(id) && !latched.has(id) && !held.has(id));
    b.classList.toggle('prepared', !!(prepared && prepared.cue && prepared.cue.id === id));
    b.classList.toggle('linked', id === linkedCue); // pressed in another window (T-281)
    // Playing, for assistive tech (T-436; a gridcell takes aria-selected, not aria-pressed).
    const on = String(latched.has(id) || held.has(id));
    if (b.getAttribute('aria-selected') !== on) b.setAttribute('aria-selected', on);
  });
}
function showCueDeck(c) {
  if (!c) return;
  const key = JSON.stringify(c);
  if (key === deckKey) return;
  deckKey = key; deck = c;
  document.querySelectorAll('#cueModes button').forEach(b => b.classList.toggle('active', b.dataset.mode === c.click_mode));
  document.querySelectorAll('#cueMulti button').forEach(b => b.classList.toggle('active', (b.dataset.multi === '1') === c.multi));
  if (document.activeElement !== $('cueMax')) $('cueMax').value = c.max_active;
  if (c.transition) {
    $('cueTransition').value = String(Math.max(0, TRANS_KINDS.indexOf(c.transition.kind)));
    const li = lengthIndex(c.transition.length);
    if (li >= 0) $('cueTransLen').value = String(li);
  }
  document.querySelectorAll('#cueQuant button').forEach(b => b.classList.toggle('active', ['off', 'beat', 'bar'][+b.dataset.q] === c.quantize));
  const be = [0, 1, 2, 4, 8, 16].indexOf(c.beat_every);
  if (be >= 0) $('cueBeatMode').value = String(be);
  paintCues();
}
async function cueDown(id, mode) {
  if (!(await post('/api/cue', { id, down: true, mode }))) return;
  refreshLook();
  if (linkedCue) { assignLinkedCue(null); paintCues(); }
  winSend({ type: 'select', id });
}
async function cueUp(id) {
  if (await post('/api/cue', { id, down: false })) refreshLook();
}
// The look panel shows the newest cue's look.
function refreshLook() {
  return lookTask(loadLook);
}
document.querySelectorAll('#cueModes button').forEach((b, i) => b.addEventListener('click', () => post('/api/control', { id: 'cue.mode', value: i })));
document.querySelectorAll('#cueMulti button').forEach(b => b.addEventListener('click', () => post('/api/control', { id: 'cue.multi', value: b.dataset.multi === '1' })));
$('cueMax').addEventListener('change', () => post('/api/control', { id: 'cue.max_active', value: +$('cueMax').value }));
$('cueTransition').addEventListener('change', () => post('/api/control', { id: 'cue.transition', value: +$('cueTransition').value }));
$('cueTransLen').addEventListener('change', () => post('/api/control', { id: 'cue.transition_length', value: +$('cueTransLen').value }));
document.querySelectorAll('#cueQuant button').forEach(b => b.addEventListener('click', () => post('/api/control', { id: 'cue.quantize', value: +b.dataset.q })));
$('cueBeatMode').addEventListener('change', () => post('/api/control', { id: 'cue.beat_mode', value: +$('cueBeatMode').value }));

export { CUE_CODES, CUE_KEYS, FILTER_KEY, PAGE_FKEYS, THUMB_MODES, THUMB_MODE_KEY, TRANS_LENGTHS,
  applyCueFilter, assignCueFilter, assignGridPage, assignPointerCue, assignPoolRev, assignPoolSel,
  assignPools, assignServerPage, cueCategories, cueDown, cueFilter, cueUp, deck, drawThumb,
  focusedCell, grid, gridEditing, gridMove, gridPage, gridPost, heldKeys, lengthIndex, loadGrid,
  loadPresets, loadSlots, pageKey, paintCues, pointerCue, poolRev, poolSel, pools, presetById,
  presets, refreshLook, renderCueFilter, renderCues, selectPage, serverPage, setStoreMode,
  setThumbMode, showCueDeck, slots, storeInto, storeMode, tagKey, thumbMode, updateCue };
