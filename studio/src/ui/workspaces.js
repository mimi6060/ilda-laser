// Laser Studio — workspaces.js (T-435).
// Sections (as in the former index.html script):
//   show mode (T-283)
//   workspaces (T-295) and LIVE panel tabs (T-270)
//   Mode nuit et mode tactile (T-282)
//   Vues enregistrées et préférences d'opérateur (T-426)
//   Fenêtres supplémentaires (T-281)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { R3_KEY, STAGE_KEY, beam3d, loadRender3d, saveRender3d, setView, showRender3d, vidStop,
  view3d } from './view3d.js';
import { $, VUE, VUE_LAYOUT_KEYS, VUE_RENDU, api, apiJson, armed, assignLinkedCue, linkedCue,
  winChan } from './core.js';
import { afterEscape, emergencyStop } from './laser.js';
import { loadCueLists, renderCueList } from './cuelist.js';
import { FILTER_KEY, THUMB_MODES, THUMB_MODE_KEY, applyCueFilter, assignCueFilter, assignGridPage,
  cueFilter, grid, gridPage, paintCues, refreshLook, renderCueFilter, renderCues, selectPage,
  setStoreMode, setThumbMode, thumbMode } from './cues.js';
import { loadPools, pollRunning } from './presets.js';
import { loadMacros } from './macros.js';
import { TL, tlSetView } from './timeline-editor.js';
import { DESK, buildDesk, deskEnter, deskLeave } from './desk.js';
import { stopLearning } from './midi.js';
import { CTAB_KEY, setCreationTab, syncFigView } from './figures.js';
import { loadSdBank } from './media.js';

// ---------- show mode (T-283) ----------
// The server owns the lock (GET/POST /api/lock, `lock` in every frame) and
// answers 423 to edits while it is on; the page hides the editing
// controls and keeps the playing ones (cues, pages, masters, tempo, noir,
// laser). Locking is one click; unlocking needs a 1 s hold on the
// padlock. Échap, Espace and ARRÊT never go through here.
let showMode = false;
const UNLOCK_HOLD_MS = 1000, CONFIRM_HOLD_MS = 800, LOCKED_FR = 'Verrouillé en mode spectacle';
let toastTimer = null;
function flash(text) {
  $('lockToast').textContent = text;
  $('lockToast').hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { $('lockToast').hidden = true; }, 2500);
}
function showShowMode(lock) {
  showSafetyLock(lock && lock.safety);
  const on = !!(lock && lock.show_mode);
  if (on === showMode) return;
  showMode = on;
  document.body.classList.toggle('show-mode', on);
  $('lockBtn').classList.toggle('on', on);
  $('lockBtn').setAttribute('aria-pressed', String(on));
  $('lockLabel').textContent = libelle(on ? 'spectacle.verrouille' : 'spectacle.libelle');
  $('lockBtn').title = libelle(on ? 'spectacle.verrouille.info' : 'spectacle.info');
  // The cue list's steps: read-only in show mode (GO stays).
  if (!on && $('cqPanel') && !$('cqPanel').hidden) renderCueList();
  if (!on) return;
  // Only what plays stays on screen (« En cours » is playback too).
  if (activeWs === 'creation' || activeWs === 'settings') setWorkspace('live');
  if ([$('runPanel'), $('cqPanel'), $('fgPanel')].every(p => !p || p.hidden)) setLivePanel('direct');
  else if ($('cqPanel') && !$('cqPanel').hidden) renderCueList();
  ['cueMenu', 'pageMenu', 'midiMenu', 'projMenu', 'projDialog', 'newProjDialog', 'tplDialog'].forEach(id => { $(id).hidden = true; });
  stopLearning();
  setStoreMode(false);
  if (typeof DESK !== 'undefined' && DESK.edit) { DESK.edit = false; DESK.sel = null; DESK.key = ''; buildDesk(); }
}
// Safety lock (T-261): the safety settings locked with a code. The
// server refuses what loosens (423); the page greys the guarded panels
// and offers lock / unlock / change / remove. Échap stays the e-stop.
const SLK_LOCKED_FR = 'Verrouillé par le responsable sécurité';
let slkState = null, slkSet = false, slkMode = 'set';
function showSafetyLock(sl) {
  if (!sl) return;
  const key = JSON.stringify([sl.set, sl.locked, sl.damaged, sl.wait_s, sl.relock_in_s == null ? null : Math.ceil(sl.relock_in_s / 60)]);
  if (key === slkState) return;
  slkState = key;
  slkSet = !!sl.set;
  const locked = !!sl.locked;
  document.body.classList.toggle('safety-locked', locked);
  document.querySelectorAll('.slk-guard').forEach(d => { if (locked) d.title = SLK_LOCKED_FR; else d.removeAttribute('title'); });
  $('slkLed').hidden = !locked;
  $('slkLockBtn').hidden = locked;
  $('slkLockBtn').textContent = sl.set ? 'Verrouiller maintenant' : 'Verrouiller…';
  $('slkUnlockBtn').hidden = !(locked && sl.set && !sl.damaged);
  $('slkChangeBtn').hidden = $('slkRemoveBtn').hidden = !(sl.set && !locked);
  let text;
  if (sl.damaged) text = 'Fichier du code illisible : réglages verrouillés. Supprimez safety-lock.json (studio arrêté) pour déverrouiller.';
  else if (!sl.set) text = 'Aucun code : les réglages de sécurité sont modifiables.';
  else if (locked) text = SLK_LOCKED_FR + (sl.wait_s ? ` · trop d'essais : attendez ${sl.wait_s} s` : '');
  else text = `Déverrouillé : reverrouillage dans ${Math.max(1, Math.ceil((sl.relock_in_s || 0) / 60))} min sans réglage de sécurité.`;
  $('slkStatus').textContent = text;
}
function openSafetyLockDialog(mode) {
  slkMode = mode;
  const asking = mode === 'unlock';
  $('slkTitle').textContent = asking ? 'Déverrouiller la sécurité' : mode === 'change' ? 'Changer le code de sécurité' : 'Verrouiller la sécurité';
  $('slkHint').textContent = asking ? 'Le code du responsable sécurité.' : 'Choisissez un code de 4 à 8 chiffres. Il n\'est jamais enregistré en clair : notez-le en lieu sûr.';
  $('slkOk').textContent = asking ? 'Déverrouiller' : 'Verrouiller';
  $('slkCode2Box').hidden = asking;
  $('slkError').textContent = '';
  $('slkDialog').hidden = false;
  $('slkCode').focus();
}
function closeSafetyLockDialog() {
  $('slkCode').value = $('slkCode2').value = '';
  $('slkDialog').hidden = true;
}
// The lock's state from an answer, if it carries one; the answer's JSON.
async function slkAnswer(r) {
  let j = null;
  try { j = await r.json(); } catch { /* plain text */ }
  if (j && j.set !== undefined) { slkState = null; showSafetyLock(j); }
  return j;
}
$('slkForm').addEventListener('submit', async e => {
  e.preventDefault();
  const code = $('slkCode').value.trim();
  if (!/^[0-9]{4,8}$/.test(code)) { $('slkError').textContent = 'Le code doit compter 4 à 8 chiffres.'; return; }
  if (slkMode !== 'unlock' && code !== $('slkCode2').value.trim()) { $('slkError').textContent = 'Les deux codes diffèrent.'; return; }
  const unlock = slkMode === 'unlock';
  try {
    const r = unlock
      ? await api('POST', '/api/safety/unlock', { code, source: 'ui' }, { quiet: true, ignore: [403, 409, 429] })
      : await api('POST', '/api/safety/lock', { action: 'set', code, source: 'ui' }, { quiet: true, ignore: [400, 409] });
    const text = await r.clone().text();
    const j = await slkAnswer(r);
    if (r.ok) { closeSafetyLockDialog(); flash(unlock ? 'Sécurité déverrouillée' : 'Sécurité verrouillée'); return; }
    $('slkError').textContent = (j && j.error) || text || `erreur ${r.status}`;
    $('slkCode').value = $('slkCode2').value = '';
    $('slkCode').focus();
  } catch { $('slkError').textContent = 'Studio injoignable'; }
});
$('slkCancel').addEventListener('click', closeSafetyLockDialog);
$('slkLockBtn').addEventListener('click', async () => {
  if (!slkSet) { openSafetyLockDialog('set'); return; }
  const r = await api('POST', '/api/safety/lock', { action: 'lock', source: 'ui' }, { quiet: true });
  if (r.ok) { await slkAnswer(r); flash('Sécurité verrouillée'); }
});
$('slkUnlockBtn').addEventListener('click', () => openSafetyLockDialog('unlock'));
$('slkChangeBtn').addEventListener('click', () => openSafetyLockDialog('change'));
$('slkRemoveBtn').addEventListener('click', async () => {
  const r = await api('POST', '/api/safety/lock', { action: 'remove', source: 'ui' }, { quiet: true });
  if (r.ok) { await slkAnswer(r); flash('Code de sécurité retiré'); }
});
async function postLock(on) {
  try {
    const r = await api('POST', '/api/lock', { show_mode: on, source: 'ui' }, { quiet: true });
    if (r.ok) showShowMode(await r.json()); else flash(await r.text());
  } catch { flash('Studio injoignable'); }
}
// Press and hold `btn` for `ms` to run `done` (the button fills
// meanwhile); let go early, or use the keyboard: only `early` (a hint).
// `active()` false: a plain button.
function holdToRun(btn, ms, done, early, active = () => true) {
  let timer = null, raf = 0;
  const end = () => {
    clearTimeout(timer); timer = null; cancelAnimationFrame(raf);
    btn.classList.remove('holding'); btn.style.setProperty('--hold', 0);
  };
  btn.addEventListener('pointerdown', e => {
    if (e.button !== 0 || !active()) return;
    e.preventDefault();
    try { btn.setPointerCapture(e.pointerId); } catch { /* synthetic pointer */ }
    const start = performance.now();
    btn.classList.add('holding');
    const tick = () => { btn.style.setProperty('--hold', Math.min(1, (performance.now() - start) / ms)); raf = requestAnimationFrame(tick); };
    tick();
    timer = setTimeout(() => { end(); btn.dataset.held = '1'; done(); }, ms);
  });
  for (const ev of ['pointerup', 'pointercancel']) btn.addEventListener(ev, () => { if (timer) { end(); early?.(); } });
  btn.addEventListener('click', e => {
    if (btn.dataset.held) { delete btn.dataset.held; return; }
    if (e.detail === 0 && active()) early?.();
  });
}
// Registered before holdToRun's own click handler, which clears `held`.
$('lockBtn').addEventListener('click', () => { if (!showMode && !$('lockBtn').dataset.held) postLock(true); });
holdToRun($('lockBtn'), UNLOCK_HOLD_MS, () => postLock(false), () => flash('Maintenir pour déverrouiller'), () => showMode);
apiJson('GET', '/api/lock').then(showShowMode).catch(() => {});

// ---------- workspaces (T-295) and LIVE panel tabs (T-270) ----------
// Pure display: switching never posts anything, so the laser, tempo and
// running cues are untouched. The top bar (laser, ARRÊT, tempo, banners)
// is outside the workspaces and always on screen. Remembered in this
// browser; a page without localStorage just starts on LIVE / Direct.
const WS_KEY = 'laserStudio.workspace', PANEL_KEY = 'laserStudio.livePanel';
const WORKSPACES = ['live', 'timeline', 'creation', 'settings', 'desk'];
let activeWs = 'live';
function remembered(key) { try { return localStorage.getItem(key); } catch { return null; } }
function remember(key, v) {
  // A window (T-281) shows a fixed layout: it never becomes the main window's.
  if (VUE && VUE_LAYOUT_KEYS.includes(key)) return;
  try { localStorage.setItem(key, v); } catch { /* private window: not remembered */ }
  opTouched();
}
// Opérateur (T-426): this browser's localStorage is a cache of the
// operator's preferences, kept on the studio (<data-dir>/operators). Set
// up here because remember() runs from the first lines below; the rest is
// further down (« Vues enregistrées »).
const OP_MARK_KEY = 'laserStudio.operator', OP_DEFAULT = 'Défaut';
const OP = { name: OP_DEFAULT, names: [OP_DEFAULT], views: [], prefs: {}, ready: false, timer: 0, chain: Promise.resolve(), last: null };
/** A preference changed: sent to the studio 500 ms after the last change. */
function opTouched() {
  if (!OP.ready) return;
  clearTimeout(OP.timer);
  OP.timer = setTimeout(opPushPrefs, 500);
}

// ---------- Mode nuit et mode tactile (T-282) ----------
// This browser's display only: nothing reaches the studio or the laser.
// prefs = { night, nightLevel (0.4–1), redTint, touch: 'auto'|'on'|'off' }.
const DISPLAY_KEY = 'laserStudio.display', TOUCH_MODES = ['auto', 'on', 'off'];
const coarsePointer = matchMedia('(pointer: coarse)');
function readDisplayPrefs() {
  let p = {};
  try { p = JSON.parse(remembered(DISPLAY_KEY) || '{}') || {}; } catch { p = {}; }
  const lvl = Number(p.nightLevel);
  return {
    night: p.night === true,
    nightLevel: Number.isFinite(lvl) ? Math.min(1, Math.max(0.4, lvl)) : 0.6,
    redTint: p.redTint === true,
    touch: TOUCH_MODES.includes(p.touch) ? p.touch : 'auto',
  };
}
let displayPrefs = readDisplayPrefs();
const touchMode = () => displayPrefs.touch === 'on' || (displayPrefs.touch === 'auto' && coarsePointer.matches);
function applyDisplay() {
  const p = displayPrefs, root = document.documentElement;
  root.classList.toggle('night', p.night);
  root.classList.toggle('red-tint', p.night && p.redTint);
  root.style.setProperty('--nl', p.night ? String(p.nightLevel) : '1');
  root.classList.toggle('touch', touchMode());
  $('dispNight').checked = p.night;
  $('dispRed').checked = p.redTint;
  $('dispRed').disabled = $('dispLevel').disabled = !p.night;
  if (document.activeElement !== $('dispLevel')) $('dispLevel').value = String(Math.round(p.nightLevel * 100));
  $('dispLevelV').textContent = `${Math.round(p.nightLevel * 100)} %`;
  document.querySelectorAll('#dispTouch button').forEach(b => {
    const on = b.dataset.touch === p.touch;
    b.classList.toggle('active', on);
    b.setAttribute('aria-checked', String(on));
  });
  $('dispTouchInfo').textContent = p.touch === 'auto'
    ? (coarsePointer.matches ? 'Écran tactile détecté : mode tactile activé.' : 'Pas d\'écran tactile détecté : mode tactile désactivé.')
    : '';
}
function setDisplay(patch) {
  displayPrefs = { ...displayPrefs, ...patch };
  remember(DISPLAY_KEY, JSON.stringify(displayPrefs));
  applyDisplay();
}
$('dispNight').addEventListener('change', e => setDisplay({ night: e.target.checked }));
$('dispRed').addEventListener('change', e => setDisplay({ redTint: e.target.checked }));
$('dispLevel').addEventListener('input', e => setDisplay({ nightLevel: +e.target.value / 100 }));
$('dispLevel').addEventListener('change', e => setDisplay({ nightLevel: +e.target.value / 100 }));
document.querySelectorAll('#dispTouch button').forEach(b => b.addEventListener('click', () => setDisplay({ touch: b.dataset.touch })));
coarsePointer.addEventListener?.('change', applyDisplay);
applyDisplay();

// Long press (T-282): a finger or a pen held still for 500 ms is a right
// click. Sliders handle it themselves (back to neutral, enhanceSlider);
// elsewhere a `contextmenu` goes to what was pressed (cue properties, page
// menu, « Apprendre MIDI »…); if nothing takes it, the control's help
// (its title) is shown, so no information stays hover-only. Never on the
// laser button or ARRÊT: a long press never arms nor resets anything there.
const LONG_PRESS_MS = 500, LONG_PRESS_SLOP = 10;
let longPress = null, longPressAt = -Infinity;
// Hold buttons (flash effects, reverse, padlock) and played cues keep the
// press for themselves: a flash cue is held for as long as the finger is
// down. In grid editing mode a long press on a cue opens its properties.
const LONG_PRESS_SKIP = '#armBtn, #estopBtn, #blackoutBtn, #estopReset, #lockBtn, .fx-hold, #rotReverse, input[type=range], input[type=text], input[type=number], textarea, select';
// The desk's components while playing too (T-427): a held flash or a fader under a finger.
const longPressSkipped = el => !!el.closest(LONG_PRESS_SKIP) || (!!el.closest('#cues .cue') && !el.closest('#cues.editing'))
  || (!!el.closest('#deskGrid .desk-item') && !el.closest('#deskGrid.editing'));
function endLongPress() { if (longPress) { clearTimeout(longPress.timer); longPress = null; } }
document.addEventListener('pointerdown', e => {
  endLongPress();
  if (e.pointerType === 'mouse' || !e.isPrimary || !(e.target instanceof Element) || longPressSkipped(e.target)) return;
  const target = e.target, x = e.clientX, y = e.clientY;
  longPress = { x, y, timer: setTimeout(() => {
    longPress = null;
    if (!target.isConnected) return;
    longPressAt = performance.now();
    const ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, button: 2, buttons: 0 });
    if (target.dispatchEvent(ev)) {
      const help = target.closest('[title]')?.title;
      if (help) flash(help);
    }
  }, LONG_PRESS_MS) };
}, true);
document.addEventListener('pointermove', e => {
  if (longPress && Math.hypot(e.clientX - longPress.x, e.clientY - longPress.y) > LONG_PRESS_SLOP) endLongPress();
}, true);
['pointerup', 'pointercancel'].forEach(t => document.addEventListener(t, endLongPress, true));
// The browser's own long-press menu, and the click that ends a long
// press, are not passed on: the press already did its job.
document.addEventListener('contextmenu', e => {
  if (e.isTrusted && (longPress || performance.now() - longPressAt < 800)) { e.preventDefault(); e.stopImmediatePropagation(); }
}, true);
document.addEventListener('click', e => {
  if (performance.now() - longPressAt < 800) { longPressAt = -Infinity; e.preventDefault(); e.stopImmediatePropagation(); }
}, true);
function setWorkspace(ws) {
  if (!WORKSPACES.includes(ws) || VUE) ws = 'live'; // every window (T-281) is cut from LIVE
  if (showMode && (ws === 'creation' || ws === 'settings')) ws = 'live'; // T-283
  // PUPITRE (T-427): leaving lets go of what it held and of full screen.
  if (activeWs === 'desk' && ws !== 'desk' && typeof deskLeave === 'function') deskLeave();
  // Vidéo de la vue 3D (T-459): another workspace ends the recording.
  if (ws !== activeWs) vidStop('espace');
  activeWs = ws;
  $('workspace').dataset.wsActive = ws;
  document.querySelectorAll('[data-ws]').forEach(el => { el.hidden = el.dataset.ws !== ws; });
  document.querySelectorAll('#wsTabs [data-ws-tab]').forEach(b => {
    b.classList.toggle('active', b.dataset.wsTab === ws);
    b.setAttribute('aria-selected', String(b.dataset.wsTab === ws));
  });
  remember(WS_KEY, ws);
  if (ws === 'live') setTimeout(pollRunning, 0); // « En cours » polls only while on screen
  // RÉGLAGES › Carte SD ShowNET (T-302): cue names may have changed.
  if (ws === 'settings') setTimeout(loadSdBank, 0);
  // CRÉATION › Figures shows the editor (T-296); set up at the end of the script.
  if ($('workspace').dataset.ctab) syncFigView();
  if (ws === 'desk') setTimeout(deskEnter, 0); // set up further down the script
}
function setLivePanel(name) {
  const tabs = [...document.querySelectorAll('#liveTabs [data-panel-tab]')];
  if (!tabs.some(b => b.dataset.panelTab === name) || (showMode && name !== 'running' && name !== 'cuelist' && name !== 'fx' && name !== 'macros')) name = 'direct';
  if (VUE === 'direct') name = 'direct';
  tabs.forEach(b => {
    b.classList.toggle('active', b.dataset.panelTab === name);
    b.setAttribute('aria-selected', String(b.dataset.panelTab === name));
  });
  document.querySelectorAll('#livePanel [data-panel]').forEach(el => { el.hidden = el.dataset.panel !== name; });
  remember(PANEL_KEY, name);
  if (name === 'running') setTimeout(pollRunning, 0); // after the script has run
  if (name === 'cuelist') setTimeout(loadCueLists, 0);
  if (name === 'macros') setTimeout(loadMacros, 0);
  if (name === 'pools') setTimeout(loadPools, 0);
}
document.querySelectorAll('#wsTabs [data-ws-tab]').forEach(b => b.onclick = () => setWorkspace(b.dataset.wsTab));
document.querySelectorAll('#liveTabs [data-panel-tab]').forEach(b => b.onclick = () => setLivePanel(b.dataset.panelTab));
// The MIDI status in the top bar leads to the controller settings.
$('midiStatus').style.cursor = 'pointer';
$('midiStatus').onclick = () => { if (showMode) return flash(LOCKED_FR); setWorkspace('settings'); $('midiPanel').open = true; };
// T-519: the shortcut hint after the tabs is whole, or « Espace laser ·
// Échap arrêt », or not shown: never a cut fragment (« [ », « Espace l… »).
// Its box takes the room left in the row, whatever it shows, so measuring
// it never moves anything; measured again when that room or the text changes.
function fitHint() {
  const h = $('wsHint');
  h.classList.remove('short', 'none');
  if (h.scrollWidth <= h.clientWidth) return;
  h.classList.add('short');
  if (h.scrollWidth > h.clientWidth) h.classList.add('none');
}
const hintFit = new ResizeObserver(fitHint);
hintFit.observe($('wsHint'));
hintFit.observe($('wsHint').firstElementChild);
setWorkspace(remembered(WS_KEY));
setLivePanel(remembered(PANEL_KEY));
// The remembered stage (2D/3D) is restored by app.js once every module has
// run (T-508): setView draws the 2D preview, whose overlays read modules
// evaluated after this one (safety.js, targets.js).
function restoreStage() { const v = remembered(STAGE_KEY); if (VUE || v === '3d' || v === 'both') setView(v); }

// ---------- Vues enregistrées et préférences d'opérateur (T-426) ----------
// A view is the screen's layout (workspace, LIVE panel tab, CRÉATION tab,
// 2D/3D and render, cue page, timeline zoom). Recalling one never posts
// anything but the cue page (control page.<n>: display and MIDI grid
// only): no cue, look, master, tempo or arming changes. The top bar
// (laser, Noir, ARRÊT, Ciel coupé, banners) is outside the workspaces and
// stays on screen whatever the view. Views and the localStorage keys
// (`laserStudio.*`) belong to the operator, on the studio, outside the
// project. This browser's copy wins when it was last synced with the
// same operator; a new browser (or another operator) takes the studio's.
const VIEW_FKEYS = ['F1', 'F2', 'F3', 'F4', 'F6', 'F7', 'F8', 'F9']; // Alt+F5 = PUPITRE
const VIEW_MAX = 32, VIEW_SLOTS = 8;
const STAGE_MODES = ['2d', '3d', 'both'];
// As operators.rs `valid_name`: never a path, never an API word.
const opValidName = n => /^[\p{L}\p{N} _-]{1,32}$/u.test(n) && n.trim() === n && !n.startsWith('_') && !['current', 'delete'].includes(n.toLowerCase());
const viewKey = slot => slot ? 'Alt+' + VIEW_FKEYS[slot - 1] : '';
/** This browser's `laserStudio.*` keys (the operator mark left out), sorted. */
function opLocalPrefs() {
  const out = {};
  try {
    const keys = [];
    for (let i = 0; i < localStorage.length; i++) keys.push(localStorage.key(i));
    for (const k of keys.sort()) {
      if (!k || !k.startsWith('laserStudio.') || k === OP_MARK_KEY) continue;
      const v = localStorage.getItem(k);
      if (v !== null && v.length <= 16384) out[k] = v; // 64 Ko for them all on the studio
    }
  } catch { /* no storage: nothing to send */ }
  return out;
}
const sameJson = (a, b) => JSON.stringify(a) === JSON.stringify(b);
function opSave() {
  const body = { name: OP.name, views: OP.views, prefs: OP.prefs };
  const path = '/api/operators/' + encodeURIComponent(OP.name);
  OP.chain = OP.chain.then(() => api('POST', path, body, { quiet: true })).then(r => r.ok, () => false);
  return OP.chain;
}
function opPushPrefs() {
  clearTimeout(OP.timer);
  const p = opLocalPrefs();
  if (sameJson(p, OP.prefs)) return OP.chain;
  OP.prefs = p;
  return opSave();
}
/** The studio's preferences replace this browser's (another operator, a new browser). */
function opReplaceLocal(prefs) {
  try {
    const keys = [];
    for (let i = 0; i < localStorage.length; i++) keys.push(localStorage.key(i));
    for (const k of keys) if (k && k.startsWith('laserStudio.') && k !== OP_MARK_KEY && !(k in prefs)) localStorage.removeItem(k);
    for (const [k, v] of Object.entries(prefs)) if (typeof v === 'string') localStorage.setItem(k, v);
  } catch { /* no storage: the page keeps what it shows */ }
}
/** Shows what the preferences say, as a reload would. */
function opApplyPrefs() {
  try {
    displayPrefs = readDisplayPrefs(); applyDisplay();
    // T-519: the shortcut hint after the tabs is whole, or « Espace laser ·
// Échap arrêt », or not shown: never a cut fragment (« [ », « Espace l… »).
// Its box takes the room left in the row, whatever it shows, so measuring
// it never moves anything; measured again when that room or the text changes.
function fitHint() {
  const h = $('wsHint');
  h.classList.remove('short', 'none');
  if (h.scrollWidth <= h.clientWidth) return;
  h.classList.add('short');
  if (h.scrollWidth > h.clientWidth) h.classList.add('none');
}
const hintFit = new ResizeObserver(fitHint);
hintFit.observe($('wsHint'));
hintFit.observe($('wsHint').firstElementChild);
setWorkspace(remembered(WS_KEY));
    setLivePanel(remembered(PANEL_KEY));
    setCreationTab(remembered(CTAB_KEY));
    if (view3d) { view3d.setRender({ ...beam3d.DEFAULT_RENDER, ...loadRender3d() }); showRender3d(); }
    const st = remembered(STAGE_KEY);
    setView(STAGE_MODES.includes(st) ? st : '2d');
    const tm = remembered(THUMB_MODE_KEY);
    if (THUMB_MODES.includes(tm) && tm !== thumbMode) setThumbMode(tm);
    try {
      const f = JSON.parse(remembered(FILTER_KEY) || '{}') || {};
      assignCueFilter({ tags: Array.isArray(f.tags) ? f.tags.filter(t => typeof t === 'string') : [], text: typeof f.text === 'string' ? f.text : '', any: !!f.any });
      $('cueFilterText').value = cueFilter.text; renderCueFilter(); applyCueFilter();
    } catch { /* filter kept */ }
  } catch (e) { console.warn('préférences d\'opérateur', e); }
}
/** Loads `name`'s profile; `switching`: chosen here, its preferences replace this browser's. */
async function opLoad(name, switching = false) {
  const prof = await apiJson('GET', '/api/operators/' + encodeURIComponent(name), undefined, { quiet: true, cache: 'no-store' });
  OP.name = prof.name || OP_DEFAULT;
  OP.views = Array.isArray(prof.views) ? prof.views : [];
  const server = prof.prefs && typeof prof.prefs === 'object' ? prof.prefs : {};
  const mark = remembered(OP_MARK_KEY);
  OP.last = null;
  OP.ready = false;
  if (!switching && (mark === OP.name || (mark === null && Object.keys(server).length === 0))) {
    // Same browser, same operator (or the first time: migrate what this browser has).
    OP.prefs = server;
    OP.ready = true;
    opPushPrefs();
  } else {
    opReplaceLocal(server);
    OP.prefs = opLocalPrefs();
    opApplyPrefs();
    OP.ready = true;
  }
  remember(OP_MARK_KEY, OP.name);
  renderViewsMenu(); renderOpPanel();
}
async function opInit() {
  try {
    const cur = await apiJson('GET', '/api/operators', undefined, { quiet: true, cache: 'no-store' });
    OP.names = cur.names || [OP_DEFAULT];
    await opLoad(cur.current || OP_DEFAULT);
  } catch (e) {
    console.warn('opérateur', e); // the page works from this browser's copy
    OP.ready = true;
  }
  renderOpPanel();
}
async function opRefreshNames() {
  try { OP.names = (await apiJson('GET', '/api/operators', undefined, { quiet: true, cache: 'no-store' })).names || OP.names; } catch { /* keep the list */ }
  renderOpPanel();
}
async function opSwitch(name) {
  if (name === OP.name) return;
  await opPushPrefs(); // the leaving operator's last changes
  if (!(await api('POST', '/api/operators/current', { name })).ok) { renderOpPanel(); return; }
  await opLoad(name, true);
  await opRefreshNames();
  flash(`Opérateur « ${OP.name} »`);
}
function renderOpPanel() {
  const sel = $('opSelect');
  const names = OP.names.includes(OP.name) ? OP.names : [...OP.names, OP.name];
  sel.innerHTML = '';
  for (const n of names) { const o = document.createElement('option'); o.value = o.textContent = n; sel.appendChild(o); }
  sel.value = OP.name;
  $('opDelete').disabled = OP.name === OP_DEFAULT;
  const n = OP.views.length;
  $('opInfo').textContent = `${n} vue${n > 1 ? 's' : ''} enregistrée${n > 1 ? 's' : ''} (${VIEW_SLOTS} à touche, ${VIEW_MAX} au plus).`;
}
$('opSelect').addEventListener('change', e => opSwitch(e.target.value));
$('opNew').addEventListener('click', async () => {
  const name = (prompt('Nom du nouvel opérateur (lettres, chiffres, espaces, - et _) :') || '').trim();
  if (!name) return;
  if (!opValidName(name)) return flash('Nom d\'opérateur invalide');
  if (!OP.names.includes(name)) {
    await opPushPrefs();
    // A new operator starts from this screen's preferences, without views.
    if (!(await api('POST', '/api/operators/' + encodeURIComponent(name), { name, views: [], prefs: OP.prefs })).ok) return;
  }
  await opSwitch(name);
});
$('opDelete').addEventListener('click', async () => {
  const name = OP.name;
  if (name === OP_DEFAULT || !confirm(`Supprimer l'opérateur « ${name} », ses vues et ses préférences ?`)) return;
  clearTimeout(OP.timer);
  await OP.chain;
  if (!(await api('POST', '/api/operators/delete', { name })).ok) return;
  await opLoad(OP_DEFAULT, true);
  await opRefreshNames();
  flash(`Opérateur « ${name} » supprimé`);
});

// The view on screen now.
function captureView(name, slot = null) {
  const panel = document.querySelector('#liveTabs [data-panel-tab].active')?.dataset.panelTab || 'direct';
  const stage = document.querySelector('#viewSeg button.active')?.dataset.view || '2d';
  let render3d = {};
  try { render3d = view3d ? { ...view3d.render } : loadRender3d(); } catch { render3d = {}; }
  return {
    name, slot, workspace: activeWs, live_panel: panel, creation_tab: $('workspace').dataset.ctab || 'look',
    stage_mode: stage, render3d, cue_page: gridPage,
    timeline: { zoom: TL.pps / 40, scroll: TL.t0, follow: !!TL.follow }, sizes: {},
  };
}
/** Puts a view on screen. Unknown values (a tab that no longer exists) are skipped. */
function applyView(v) {
  if (WORKSPACES.includes(v.workspace)) setWorkspace(v.workspace);
  if (document.querySelector(`#liveTabs [data-panel-tab="${CSS.escape(v.live_panel || '')}"]`)) setLivePanel(v.live_panel);
  if (document.querySelector(`#creationTabs [data-ctab="${CSS.escape(v.creation_tab || '')}"]`)) setCreationTab(v.creation_tab);
  if (v.render3d && typeof v.render3d === 'object' && Object.keys(v.render3d).length) {
    if (view3d) { view3d.setRender(v.render3d); saveRender3d(); showRender3d(); }
    else remember(R3_KEY, JSON.stringify(v.render3d)); // read when the 3D view starts
  }
  if (STAGE_MODES.includes(v.stage_mode)) setView(v.stage_mode);
  const page = Number(v.cue_page);
  if (Number.isInteger(page) && grid.pages[page] && page !== gridPage) selectPage(page);
  const t = v.timeline || {};
  if (Number.isFinite(t.zoom) && t.zoom > 0 && Number.isFinite(t.scroll)) tlSetView(t.scroll, t.zoom * 40);
  if (typeof t.follow === 'boolean') { TL.follow = t.follow; $('tlFollow').checked = t.follow; }
  OP.last = v.name;
  renderViewsMenu();
}
function recallSlot(slot) {
  const v = OP.views.find(x => x.slot === slot);
  if (!v) return flash(`Aucune vue sur ${viewKey(slot)}`);
  applyView(v);
  flash('Vue rappelée');
}
async function storeView(v) {
  const i = OP.views.findIndex(x => x.name === v.name);
  if (i >= 0) OP.views[i] = v; else OP.views.push(v);
  OP.last = v.name;
  renderViewsMenu(); renderOpPanel();
  if (await opSave()) flash(`Vue « ${v.name} » enregistrée`);
  else flash('Vue non enregistrée : le studio a refusé');
}
function storeSlot(slot) {
  const old = OP.views.find(x => x.slot === slot);
  if (old && !confirm(`Remplacer la vue « ${old.name} » (${viewKey(slot)}) par l'écran actuel ?`)) return;
  if (!old && OP.views.length >= VIEW_MAX) return flash(`${VIEW_MAX} vues au plus`);
  let name = old ? old.name : `Vue ${slot}`;
  for (let n = 2; !old && OP.views.some(x => x.name === name); n++) name = `Vue ${slot} (${n})`;
  storeView(captureView(name, slot));
}
function storeNamed() {
  if (OP.views.length >= VIEW_MAX) return flash(`${VIEW_MAX} vues au plus`);
  const name = (prompt('Nom de la vue :', `Vue ${OP.views.length + 1}`) || '').trim().slice(0, 32);
  if (!name) return;
  const old = OP.views.find(x => x.name === name);
  if (old && !confirm(`Remplacer la vue « ${name} » ?`)) return;
  const used = new Set(OP.views.filter(x => x !== old).map(x => x.slot).filter(Boolean));
  let slot = old ? old.slot : null;
  for (let s = 1; !slot && s <= VIEW_SLOTS; s++) if (!used.has(s)) slot = s;
  storeView(captureView(name, slot || null));
}
function renderViewsMenu() {
  const m = $('viewsMenu');
  m.innerHTML = '';
  const item = (text, key, onClick, extra = {}) => {
    const b = document.createElement('button');
    b.setAttribute('role', 'menuitem');
    const t = document.createElement('span'); t.textContent = text; b.appendChild(t);
    if (key) { const k = document.createElement('kbd'); k.textContent = key; b.appendChild(k); }
    if (extra.current) b.setAttribute('aria-current', 'true');
    if (extra.disabled) b.disabled = true;
    b.onclick = () => { closeViewsMenu(); onClick(); };
    m.appendChild(b);
    return b;
  };
  const sorted = [...OP.views].sort((a, b) => (a.slot || 99) - (b.slot || 99));
  if (!sorted.length) item('Aucune vue enregistrée', '', () => {}, { disabled: true });
  for (const v of sorted) item(v.name, viewKey(v.slot), () => { applyView(v); flash('Vue rappelée'); }, { current: v.name === OP.last });
  m.appendChild(document.createElement('hr'));
  item('Enregistrer la vue…', '', storeNamed);
  const cur = OP.views.find(x => x.name === OP.last);
  if (cur) {
    item(`Mettre à jour « ${cur.name} »`, cur.slot ? 'Alt+⇧+' + VIEW_FKEYS[cur.slot - 1] : '', () => storeView(captureView(cur.name, cur.slot ?? null)));
    item(`Renommer « ${cur.name} »…`, '', () => {
      const name = (prompt('Nouveau nom de la vue :', cur.name) || '').trim().slice(0, 32);
      if (!name || name === cur.name) return;
      if (OP.views.some(x => x.name === name)) return flash(`Une vue s'appelle déjà « ${name} »`);
      cur.name = name; OP.last = name;
      renderViewsMenu(); opSave();
    });
    item(`Supprimer « ${cur.name} »`, '', () => {
      if (!confirm(`Supprimer la vue « ${cur.name} » ?`)) return;
      OP.views = OP.views.filter(x => x !== cur); OP.last = null;
      renderViewsMenu(); renderOpPanel(); opSave();
    });
  }
}
function closeViewsMenu() { $('viewsMenu').hidden = true; $('viewsBtn').setAttribute('aria-expanded', 'false'); }
$('viewsBtn').onclick = e => {
  e.stopPropagation();
  if (!$('viewsMenu').hidden) { closeViewsMenu(); return; }
  renderViewsMenu();
  const r = $('viewsBtn').getBoundingClientRect();
  const menu = $('viewsMenu');
  menu.hidden = false;
  menu.style.top = (r.bottom + 4) + 'px';
  menu.style.left = Math.max(8, Math.min(r.left, innerWidth - menu.offsetWidth - 8)) + 'px';
  $('viewsBtn').setAttribute('aria-expanded', 'true');
  menu.querySelector('button:not(:disabled)')?.focus();
};
document.addEventListener('click', e => { if (!$('viewsMenu').hidden && !e.target.closest('#viewsMenu, #viewsBtn')) closeViewsMenu(); });
renderViewsMenu();
setTimeout(opInit, 0); // once the whole script has run (grid, timeline, filters)
// The e2e tests read the operator here: read only.
window.__operator = () => ({ name: OP.name, ready: OP.ready, views: OP.views.map(v => ({ ...v })), last: OP.last });

// ---------- Fenêtres supplémentaires (T-281) ----------
// « Ouvrir une fenêtre ▾ » opens this page again with ?vue=…, one window
// per kind (choosing it again brings it forward), for a second monitor, a
// touch screen or a client. Every window is a full page of its own: it
// beats (presence, T-252), reads the engine state from the studio (the
// pushed state, T-411), shows the laser state, and Échap stops the laser
// from any of them. It lets go of what it holds (flashes, held buttons,
// executors) when it loses focus or closes; closing it never arms or
// disarms by itself. The grid page and the cue last pressed reach the
// other windows at once through `winChan`, without waiting for the studio.
const VUE_NAMES = { sortie: 'Sortie', grille: 'Grille', direct: 'Direct', '3d': '3D' };
const VUE_SIZES = { sortie: [900, 900], grille: [1280, 820], direct: [560, 900], '3d': [1200, 820] };
function winSend(msg) {
  try { winChan?.postMessage(msg); } catch (err) { console.warn('fenêtres', err); }
}
if (winChan) winChan.onmessage = ({ data: d }) => {
  if (!d || typeof d !== 'object') return;
  if (d.type === 'page' && Number.isInteger(d.page) && grid.pages[d.page] && d.page !== gridPage) { assignGridPage(d.page); renderCues(); }
  if (d.type === 'select' && typeof d.id === 'string') { assignLinkedCue(d.id); paintCues(); refreshLook(); }
};
function openVueWindow(v) {
  if (!VUE_NAMES[v]) return;
  const [w, h] = VUE_SIZES[v];
  const win = window.open('/?vue=' + v, 'laser-studio-' + v, `popup,width=${w},height=${h}`);
  if (!win) return flash('Fenêtre refusée par le navigateur (fenêtres surgissantes bloquées ?)');
  try { win.focus(); } catch { /* another origin cannot happen; kept quiet */ }
}
function closeWinMenu() { $('winMenu').hidden = true; $('winBtn').setAttribute('aria-expanded', 'false'); }
$('winBtn').onclick = e => {
  e.stopPropagation();
  if (!$('winMenu').hidden) { closeWinMenu(); return; }
  const r = $('winBtn').getBoundingClientRect(), menu = $('winMenu');
  menu.hidden = false;
  menu.style.top = (r.bottom + 4) + 'px';
  menu.style.left = Math.max(8, Math.min(r.left, innerWidth - menu.offsetWidth - 8)) + 'px';
  $('winBtn').setAttribute('aria-expanded', 'true');
  menu.querySelector('button')?.focus();
};
$('winMenu').querySelectorAll('[data-vue]').forEach(b => b.onclick = () => { closeWinMenu(); openVueWindow(b.dataset.vue); });
$('winMenu').addEventListener('keydown', e => {
  const items = [...$('winMenu').querySelectorAll('button')], i = items.indexOf(document.activeElement);
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); items[(i + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length].focus(); }
});
document.addEventListener('click', e => { if (!$('winMenu').hidden && !e.target.closest('#winMenu, #winBtn')) closeWinMenu(); });
// The laser state in the window's own bar: the top bar is not shown in
// Sortie and 3D. Armed always shows, even with the engine link lost.
function showVueArm() {
  if (!VUE) return;
  const estop = !$('estopBanner').hidden, lost = !$('linkLost').hidden;
  const unknown = libelle('securite.laser.moteur_inconnu');
  const [state, text] = estop ? ['estop', libelle('securite.estop')] : armed ? ['armed', libelle('securite.laser.arme')]
    : lost ? ['lost', libelle('securite.laser.off') + ' · ' + unknown] : ['off', libelle('securite.laser.off')];
  const el = $('vueArm');
  const full = armed && lost ? text + ' · ' + unknown : text;
  if (el.dataset.state === state && el.textContent === full) return;
  el.dataset.state = state;
  el.textContent = full;
}
// « Plein écran »: the whole page, so the laser state stays on screen. As
// in PUPITRE, Échap is the emergency stop: with the keyboard lock the page
// gets it; without, the browser uses it to leave full screen, so any exit
// not asked for with this button stops the laser too.
const VUEFS = { full: false, ownExit: false };
async function vueEnterFull() {
  try { await document.documentElement.requestFullscreen(); } catch { flash('Plein écran refusé par le navigateur'); return; }
  try { await navigator.keyboard?.lock?.(['Escape']); } catch { /* not available */ }
  VUEFS.full = true; VUEFS.ownExit = false;
  $('vueFull').setAttribute('aria-pressed', 'true');
  $('vueFull').textContent = libelle('fenetres.plein_ecran.quitter');
}
function vueExitFull() {
  VUEFS.ownExit = true;
  try { navigator.keyboard?.unlock?.(); } catch { /* not available */ }
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  vueFullOff();
}
function vueFullOff() {
  VUEFS.full = false;
  $('vueFull').setAttribute('aria-pressed', 'false');
  $('vueFull').textContent = libelle('fenetres.plein_ecran');
}
document.addEventListener('fullscreenchange', () => {
  if (document.fullscreenElement || !VUEFS.full) return;
  const ours = VUEFS.ownExit;
  vueFullOff();
  if (!ours) { emergencyStop('keyboard'); afterEscape(); }
});
$('vueFull').onclick = () => { if (VUEFS.full) vueExitFull(); else vueEnterFull(); };
if (VUE) {
  $('vueBar').hidden = false;
  $('vueName').textContent = 'Fenêtre ' + VUE_NAMES[VUE];
  document.title = VUE_NAMES[VUE] + ' — Laser Studio';
  showVueArm();
  setInterval(showVueArm, 250);
  // Sortie: the pointer hides after 2 s without moving.
  if (VUE === 'sortie') {
    let idle = 0;
    const wake = () => { document.body.classList.remove('vue-idle'); clearTimeout(idle); idle = setTimeout(() => document.body.classList.add('vue-idle'), 2000); };
    ['pointermove', 'pointerdown', 'keydown'].forEach(ev => document.addEventListener(ev, wake, { passive: true }));
    wake();
  }
}
// Which shortcuts a window takes: Sortie and 3D only the safety ones
// (not arming: their bar has no laser button), and 3D its viewpoints;
// Grille and Direct all but those that change the workspace or the view.
function vueAllows(s) {
  if (!VUE) return true;
  if (VUE === 'sortie' || VUE === '3d') return (s.group === 'Sécurité' && s.id !== 'arm') || (VUE === '3d' && s.group === 'Vue 3D');
  return s.group !== 'Espaces de travail' && s.group !== 'Vues';
}
// For the e2e tests: read only.
window.__vue = () => ({ vue: VUE, rendu: VUE === 'sortie' ? VUE_RENDU : null, page: gridPage, linked: linkedCue, channel: !!winChan, full: VUEFS.full });
// The workspace grid fills the window below the top bar, whose height
// changes with the banners.
if (globalThis.ResizeObserver) new ResizeObserver(() => {
  document.documentElement.style.setProperty('--top-h', $('top').offsetHeight + 'px');
}).observe($('top'));

export { CONFIRM_HOLD_MS, LOCKED_FR, LONG_PRESS_MS, VIEW_FKEYS, WORKSPACES, activeWs, closeViewsMenu,
  flash, holdToRun, opTouched, recallSlot, remember, remembered, restoreStage, setLivePanel, setWorkspace,
  showMode, showShowMode, showVueArm, storeSlot, vueAllows, winSend };
