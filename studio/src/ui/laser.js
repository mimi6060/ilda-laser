// Laser Studio — laser.js (T-435).
// Sections (as in the former index.html script):
//   laser on/off, emergency stop
//   keyboard shortcuts (T-284)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { $, HIST, NC, api, armed, assignArmed, assignPageLeaving, editLook, historyStep, post,
  setHistoryPanel, setNoticePanel, settings, showSettings } from './core.js';
import { assignPickingHoldKey, clientId, downCodes, keyName, pickHoldKey, pickingHoldKey, presence,
  setHold, shiftMod } from './presence.js';
import { LINK } from './state.js';
import { REC, beam3d, prepMode, prepareCue, prepared, sendPrepared, setCam3d, show3d, vidReady,
  vidHidden, vidToggle, view3d } from './view3d.js';
import { VIEW_FKEYS, WORKSPACES, activeWs, closeViewsMenu, recallSlot, setWorkspace, showVueArm,
  storeSlot, vueAllows } from './workspaces.js';
import { cqAct, cqPlayer } from './cuelist.js';
import { toggleSky } from './safety.js';
import { FX_HOLDS, assignReverseKey, control, fxHold, reverse, reverseKey } from './direct.js';
import { CUE_CODES, CUE_KEYS, PAGE_FKEYS, assignPointerCue, cueDown, cueUp, focusedCell, grid,
  gridEditing, gridMove, gridPage, heldKeys, pointerCue, presetById, selectPage, setStoreMode,
  storeInto, storeMode, updateCue } from './cues.js';
import { macRun, macroForKey, scrRun } from './macros.js';
import { SL, slAct, toggleTcBig } from './shows.js';
import { TL, closeTlExamples, tlEvents, tlLockAll, tlLockKey, tlPlayhead, tlRedo, tlUndo } from './timeline-editor.js';
import { tlAddMarker, tlCopy, tlDelete, tlDuplicate, tlPaste } from './timeline-markers.js';
import { tlCv } from './timeline-canvas.js';
import { execKeyDown, execKeyUp, execReleaseAll } from './layers.js';
import { DESK, deskDelete, deskReleaseAll } from './desk.js';
import { setHazeHeld } from './outputs.js';
import { openChecklist } from './venue.js';
import { stopLearning } from './midi.js';
import { closeVapcMenu, ext, setGate } from './apc40.js';
import { projAction } from './project.js';
import { deleteSelected, figRedo, figUndo } from './figures.js';
import { closeHelpMenu, closeSearch, toggleSearch } from './help.js';
import { toggleCommand } from './command.js';

// ---------- laser on/off, emergency stop ----------
// Frames fetched before the last arm request answered may carry an old
// "armed" value; they must not overwrite what the server just told us.
let arm = null, refusedTimer = null, armAnsweredAt = 0;
function assignArm(v) { return (arm = v); }
// {enforced, complete} of the pre-show checklist (T-258), from each frame.
let checklistState = null;
function assignChecklistState(v) { return (checklistState = v); }
// {active, hw_stop_ok, wifi} of a ShowNET output (T-472), from each frame.
let shownetState = null;
function assignShownetState(v) { return (shownetState = v); }
const hwStopPending = () => !!(shownetState && shownetState.active && !shownetState.hw_stop_ok);
const checklistPending = () => !!(checklistState && checklistState.enforced && !checklistState.complete) || hwStopPending();
async function sendArm(body) {
  // client_id: this page is alive, even if its first heartbeat is late.
  const r = await api('POST', '/api/arm', { ...body, client_id: clientId }, { quiet: true });
  armAnsweredAt = performance.now();
  // Binary frames up to this one were published before the change (T-411).
  LINK.armSeqMin = +(r.headers.get('X-Frame-Seq') || 0);
  if (r.ok) { assignArmed((await r.json()).armed); $('armRefused').hidden = true; showArmed(); return; }
  if (r.status === 409) {
    const { blocking, confirm_backup_lost } = await r.json();
    // Main machine whose backup is unreachable (T-456): the operator
    // confirms the risk, then the request goes again, explicitly on.
    if (confirm_backup_lost && !body.confirm_backup_lost) {
      assignArmed(false); showArmed();
      if (confirm(confirm_backup_lost)) return sendArm({ on: true, source: body.source, confirm_backup_lost: true });
      return;
    }
    assignArmed(false); showArmed();
    const box = $('armRefused');
    box.innerHTML = '<b>Armement impossible :</b><ul></ul>';
    for (const why of blocking) { const li = document.createElement('li'); li.textContent = why; box.querySelector('ul').appendChild(li); }
    box.hidden = false;
    clearTimeout(refusedTimer);
    refusedTimer = setTimeout(() => { box.hidden = true; }, 5000);
  } else console.warn('/api/arm', await r.text());
}
// Plain "off" (Shift+Escape, blackouts) is always explicit, never a toggle.
const setArmed = (on, source) => sendArm({ on, source });
const toggleArmed = source => sendArm({ toggle: true, source });
// Latched stop: no JSON, no await before the request leaves.
function emergencyStop(source) {
  assignArmed(false); showArmed();
  // Frames already on their way still say "armed": not shown (T-411).
  armAnsweredAt = performance.now();
  LINK.armSeqMin = (LINK.seq ?? 0) + 3;
  api('POST', '/api/estop?source=' + source, undefined, { keepalive: true })
    .then(r => { const seq = +(r.headers.get('X-Frame-Seq') || 0); if (seq) LINK.armSeqMin = seq; }).catch(() => {});
  $('estopBanner').hidden = false;
}
// Only text entry counts as typing: after a slider, checkbox or button,
// Space, Enter, Backspace and the cue keys must keep working.
function isTyping() {
  const el = document.activeElement;
  if (!el) return false;
  if (el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable) return true;
  return el.tagName === 'INPUT' && ['text', 'number', 'search', 'password', 'email', 'url'].includes(el.type);
}
const clock = ms => new Date(ms).toLocaleTimeString('fr-FR');
function since(ms) {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000)), m = Math.floor(s / 60);
  return m ? `${m} min ${s % 60} s` : `${s} s`;
}
function showArmed() {
  $('armBtn').classList.toggle('on', armed);
  $('armBtn').setAttribute('aria-pressed', String(armed));
  document.body.classList.toggle('emitting', armed);
  const todo = !armed && checklistPending();
  $('armBtn').classList.toggle('todo', todo);
  $('armBtn').textContent = libelle(armed ? 'securite.laser.on' : !todo ? 'securite.laser.off' : hwStopPending() ? 'securite.laser.arret_materiel' : 'securite.laser.liste');
  $('offBadge').classList.toggle('hidden', armed);
  if (!arm) return showVueArm();
  $('estopBanner').hidden = !arm.estop;
  const d = arm.last_disarm;
  $('armInfo').textContent = arm.armed ? libelle('securite.arme.depuis', { duree: since(arm.since), source: arm.source_fr })
    : d ? libelle('securite.desarme.raison', { raison: d.reason_fr, source: d.source_fr, heure: clock(d.at) }) : libelle('securite.desarme');
  showVueArm();
}
// ShowNET output (T-472): the hardware-stop reminder and the Wi-Fi pill,
// a permanent warning while the link to the box is the Wi-Fi. A LaserCube
// on the network (T-496) gets the same pill.
function showShownet(sn) {
  shownetState = sn || null;
  const wifi = sn && sn.wifi;
  $('wifiPill').hidden = !wifi;
  if (wifi) {
    const via = wifi.interface ? ' (' + wifi.interface + ')' : '';
    $('wifiPill').title = sn.wifi_device === 'lasercube'
      ? `La LaserCube est jointe par le Wi-Fi${via} : liaison non fiable pour un show, préférez l'Ethernet.`
      : `La sortie ShowNET passe par le Wi-Fi${via} : `
        + 'Laserworld le déconseille (performances réduites, fonctionnement non sûr). Branchez un câble réseau.';
  }
}
// Battery and heat of this Mac (T-495): a pill each, only when not normal.
// Information for the operator: nothing is armed, disarmed or dimmed.
function showPowerWatch(p) {
  const pct = p && p.battery_pct != null ? ` (${p.battery_pct} %)` : '';
  const battery = !!(p && p.source === 'battery');
  const low = battery && p.battery_pct != null && p.battery_pct <= 20, crit = battery && p.battery_pct != null && p.battery_pct <= 10;
  const pp = $('powerPill');
  pp.hidden = !battery;
  if (battery) {
    pp.textContent = libelle(low ? 'barre.batterie.faible' : 'barre.batterie', { pct });
    pp.title = libelle('barre.batterie.info');
    pp.classList.toggle('crit', crit);
  }
  const hot = !!(p && (p.thermal === 'serious' || p.thermal === 'critical'));
  const tp = $('thermalPill');
  tp.hidden = !hot;
  if (hot) {
    tp.textContent = libelle(p.thermal === 'critical' ? 'barre.surchauffe' : 'barre.chauffe');
    tp.title = libelle('barre.chauffe.info');
    tp.classList.toggle('crit', p.thermal === 'critical');
  }
}
// While the checklist is to do, the button opens it instead (Space still
// asks, and shows why arming is refused).
$('armBtn').addEventListener('click', () => { if (!armed && checklistPending()) openChecklist(); else toggleArmed('ui'); });
// Lit slider tracks (T-299): the filled part follows the value, also when
// the server or MIDI moves it.
function syncFill(r) { const lo = +r.min || 0, hi = r.max === '' ? 100 : +r.max; r.style.setProperty('--fill', `${hi > lo ? (r.value - lo) / (hi - lo) * 100 : 0}%`); syncValueText(r); }
// T-436: a slider says its value with its unit (« 80 % »), as shown next
// to it (its `<id>V` or the value right after it).
function syncValueText(r) {
  const next = r.nextElementSibling;
  const shown = (r.id && document.getElementById(r.id + 'V')) || (next && next.matches('.v, .dv') ? next : null);
  const t = shown ? shown.textContent.trim() : '';
  if (t && r.getAttribute('aria-valuetext') !== t) r.setAttribute('aria-valuetext', t);
  // Its name: the <label> right before it, also in built lists (modulators, effects).
  const l = r.previousElementSibling;
  if (l && l.tagName === 'LABEL' && !l.htmlFor && !r.hasAttribute('aria-label') && !r.hasAttribute('aria-labelledby')) {
    if (!l.id) l.id = 'lbl-' + (r.id || 'r' + (syncValueText.n = (syncValueText.n || 0) + 1));
    r.setAttribute('aria-labelledby', l.id);
  }
}
document.addEventListener('input', e => { if (e.target.type === 'range') syncFill(e.target); });
setInterval(() => document.querySelectorAll('input[type=range]').forEach(syncFill), 200);
$('estopBtn').addEventListener('click', () => emergencyStop('ui'));
// Noir (T-271): a plain disarm, like Shift+Escape - never anything else,
// never an arm; works in show mode too (the top bar is never locked).
$('blackoutBtn').addEventListener('click', () => setArmed(false, 'ui'));
$('estopReset').addEventListener('click', async () => { if (await post('/api/estop/reset')) { $('estopBanner').hidden = true; } });
// ---------- keyboard shortcuts (T-284) ----------
// ONE table drives the keydown/keyup handler and the « ? » help. Read in
// order, the first entry that matches wins. Fields:
//   codes: KeyboardEvent.code (by position: AZERTY « a » = QWERTY « q »),
//   or keys: KeyboardEvent.key (a character: Cmd+Z stays on the Z of the
//   layout, like the system's own shortcuts; Échap by key, so a remapped
//   Caps Lock still stops).
//   meta (Cmd or Ctrl), alt, shift: true / false (omitted) / '*' (either).
//   Shift is shiftMod(e): a Shift that is the hold key does not count.
//   zones: where it applies ('*' = everywhere), see shortcutZone().
//   typing: also while a text field has the focus. show: allowed in show
//   mode (T-283; the action itself, or the server, enforces it).
//   down(e, i) (i = index in codes/keys): false = not for us (no
//   preventDefault). up(e): on release, whatever the zone or modifiers.
// The safety entries come first and only use what is defined above, so
// Échap works even if a later part of the page failed to load.
// 'ask': the « Modifications non enregistrées » question is open (T-433).
// 'grid': LIVE with the focus in the cue grid (T-436): the arrows and
// Début / Fin move between cells, `0` plays the focused one.
// 'script': CRÉATION with the focus in the script editor (T-458): always
// typing, so nothing but Cmd+Entrée (and the « typing » entries) fires.
const SHORTCUT_ZONES = ['live', 'grid', 'timeline', 'tl-canvas', 'creation', 'figures', 'settings', 'desk', 'ask', 'script'];
const NOT_TL_CANVAS = SHORTCUT_ZONES.filter(z => z !== 'tl-canvas' && z !== 'ask' && z !== 'script');
const NOT_LIVE = SHORTCUT_ZONES.filter(z => z !== 'live' && z !== 'grid');
const LIVE_ZONES = ['live', 'grid'];
// Macros (T-425): Alt + letter, read by position (e.code). Alt+B (the
// haze, T-453), Alt+C (« Ciel coupé », T-262), Alt+R (the 3D video,
// T-459), P, S, T and X already have a role.
const MACRO_TAKEN = ['B', 'C', 'P', 'R', 'S', 'T', 'X'];
const MACRO_LETTERS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'.split('').filter(l => !MACRO_TAKEN.includes(l));
const SAFETY_SHORTCUTS = [
  { id: 'estop', group: 'Sécurité', keys: ['Escape'], meta: '*', alt: '*', zones: '*', typing: true, show: true,
    label: "Arrêt d'urgence (verrouillé : réinitialiser pour réarmer) ; ferme aussi cette aide",
    down: () => { emergencyStop('keyboard'); afterEscape(); } },
  { id: 'disarm', group: 'Sécurité', keys: ['Escape'], shift: true, meta: '*', alt: '*', zones: '*', typing: true, show: true,
    label: 'Désarmer (noir), sans verrouiller',
    down: () => { setArmed(false, 'keyboard'); afterEscape(); } },
];
// After the stop, never instead of it: end a hold-key pick, a MIDI learn, the help.
// The bell and history panels and the top menus close too (T-482): they
// sit under the top bar, where the stop's banner and its reset appear.
function afterEscape() {
  assignPickingHoldKey(false);
  try { stopLearning(); closeKeysHelp(); closeTlExamples(); closeVapcMenu(); closeSearch(); closeViewsMenu(); } catch (err) { console.warn('Échap', err); }
  try { if (NC.open) setNoticePanel(false); if (HIST.open) setHistoryPanel(false); closeHelpMenu(); $('projMenu').hidden = true; } catch (err) { console.warn('Échap', err); }
}
let restShortcuts = null;
function buildShortcuts() {
  return [
    { id: 'arm', group: 'Sécurité', codes: ['Space'], meta: '*', alt: '*', shift: '*', zones: '*', show: true,
      label: 'Armer / désarmer le laser',
      down: e => { if (!e.repeat) toggleArmed('keyboard'); } },
    // Hold-to-run (T-252): the key is chosen in RÉGLAGES, it works whatever
    // has focus. Left out of the conflict test (it changes at run time; the
    // server refuses Espace and Échap).
    { id: 'hold', group: 'Sécurité', dynamic: true, codes: [presence.hold_key], meta: '*', alt: '*', shift: '*', zones: '*', typing: true, show: true,
      cap: () => keyName(presence.hold_key), label: 'Homme mort : tenir pour émettre (si activé dans RÉGLAGES)',
      when: () => presence.hold_to_run,
      down: e => { setHold(true); if (isTyping()) return false; },
      up: () => setHold(false) },
    // « Ciel coupé » (T-262): Alt+C everywhere (C alone is a cue key in LIVE).
    { id: 'sky', group: 'Sécurité', codes: ['KeyC'], alt: true, zones: '*', show: true,
      label: 'Ciel coupé : éteindre / rétablir tout au-dessus de l\'horizon (sans désarmer)',
      down: e => { if (!e.repeat) toggleSky('keyboard'); } },
    // Points de vue de la vue 3D (T-278): Alt+Maj+chiffre, while it is shown
    // (Alt+chiffre is the cue page, the digit alone an executor).
    // Vidéo de la vue 3D (T-459): Alt+R in LIVE (R alone is a cue key),
    // read by position like the other Alt letters.
    { id: 'view3d.record', group: 'Vue 3D', codes: ['KeyR'], alt: true, zones: LIVE_ZONES, show: true,
      label: 'Enregistrer la vue 3D en vidéo (démarrer / arrêter ; vue 3D affichée)',
      when: () => !!REC.rec || vidReady() || vidHidden(), // hidden behind the prepared pane: refused with a message
      down: e => { if (!e.repeat) vidToggle(); } },
    { id: 'view3d.camera', group: 'Vue 3D', codes: ['Digit1', 'Digit2', 'Digit3', 'Digit4', 'Digit5', 'Digit6'], alt: true, shift: true, zones: '*', show: true,
      cap: () => '1 … 6', label: 'Point de vue : Public (régie), Premier rang, Scène, Dessus, Côté, Libre (vue 3D affichée)',
      when: () => show3d && !!view3d,
      down: (e, i) => { if (!e.repeat) setCam3d(beam3d.CAMERA_PRESETS[i]); } },
    { id: 'workspace', group: 'Espaces de travail', codes: ['F1', 'F2', 'F3', 'F4'], zones: '*', typing: true, show: false,
      cap: () => 'F1 … F4', label: 'LIVE, TIMELINE, CRÉATION, RÉGLAGES (en mode spectacle : LIVE et TIMELINE)',
      down: (e, i) => setWorkspace(WORKSPACES[i]) },
    // PUPITRE (T-427): Alt+F5, F5 alone being cue page 1; allowed in show mode (playing).
    { id: 'workspace.desk', group: 'Espaces de travail', codes: ['F5'], alt: true, zones: '*', typing: true, show: true,
      label: 'PUPITRE : votre page de boutons, curseurs et pavés XY',
      down: () => setWorkspace('desk') },
    // Vues enregistrées (T-426): Alt + F1…F4, F6…F9 (Alt+F5 is PUPITRE,
    // F alone the workspaces and cue pages). Display only, never the output.
    { id: 'view.recall', group: 'Vues', codes: VIEW_FKEYS, alt: true, zones: '*', typing: true, show: true,
      cap: () => 'F1 … F4, F6 … F9', label: 'Rappeler la vue 1 à 8 (l\'écran seulement : la sortie ne change pas)',
      down: (e, i) => { if (!e.repeat) recallSlot(i + 1); } },
    { id: 'view.store', group: 'Vues', codes: VIEW_FKEYS, alt: true, shift: true, zones: '*', typing: true, show: true,
      cap: () => 'F1 … F4, F6 … F9', label: 'Enregistrer l\'écran actuel dans la vue 1 à 8 (confirmation si elle existe)',
      down: (e, i) => { if (!e.repeat) storeSlot(i + 1); } },
    { id: 'desk.delete', group: 'Pupitre', keys: ['Delete'], alt: '*', shift: '*', zones: ['desk'], show: false,
      where: 'PUPITRE (mode Modifier)', label: 'Supprimer le composant sélectionné',
      when: () => DESK.edit && DESK.sel != null, down: () => deskDelete() },
    { id: 'page.f', group: 'Cues et pages', codes: PAGE_FKEYS, zones: '*', typing: true, show: true,
      cap: () => 'F5 … F12', label: 'Pages de cues 1 à 8',
      down: (e, i) => { if (!e.repeat) selectPage(i); } },
    { id: 'page.alt', group: 'Cues et pages', codes: ['Digit1', 'Digit2', 'Digit3', 'Digit4', 'Digit5', 'Digit6', 'Digit7', 'Digit8', 'Digit9'],
      alt: true, zones: '*', show: true, cap: () => '1 … 9', label: 'Pages de cues 1 à 9',
      down: (e, i) => { if (!e.repeat) selectPage(i); } },
    // Entrée and ⌫ belong to the timeline while it has the focus (marker, delete).
    { id: 'tempo.tap', group: 'Tempo', codes: ['Enter'], meta: '*', shift: '*', zones: NOT_TL_CANVAS, show: true,
      label: 'Tap tempo',
      down: () => { post('/api/control', { id: 'tempo.tap' }); } },
    { id: 'tempo.resync', group: 'Tempo', codes: ['Backspace'], meta: '*', alt: '*', shift: '*', zones: NOT_TL_CANVAS, show: true,
      label: 'Recaler sur le 1',
      down: () => { post('/api/control', { id: 'tempo.resync' }); } },
    // Liste de cues (T-410), LIVE, once a list is loaded. Presentation
    // remotes send Page suivante / Page précédente. Espace stays the arming.
    { id: 'list.go', group: 'Liste de cues', codes: ['PageDown'], zones: LIVE_ZONES, show: true,
      label: 'GO : pas suivant (télécommande de présentation)', when: () => !!cqPlayer,
      down: e => { if (!e.repeat) cqAct('go', 'keyboard'); } },
    // « Préparer » (T-274): Entrée du pavé numérique (Entrée est le tap).
    { id: 'prepared.send', group: 'Cues et pages', codes: ['NumpadEnter'], zones: LIVE_ZONES, show: true,
      label: 'Envoyer le cue préparé à la sortie', when: () => !!(prepared && prepared.cue),
      down: e => { if (!e.repeat) sendPrepared(); } },
    { id: 'list.go.alt', group: 'Liste de cues', codes: ['Enter', 'NumpadEnter'], alt: true, zones: LIVE_ZONES, show: true,
      label: 'GO (claviers sans touche Page suivante)', when: () => !!cqPlayer,
      down: e => { if (!e.repeat) cqAct('go', 'keyboard'); } },
    { id: 'list.back', group: 'Liste de cues', codes: ['PageUp'], zones: LIVE_ZONES, show: true,
      label: 'Retour : pas précédent', when: () => !!cqPlayer,
      down: e => { if (!e.repeat) cqAct('back', 'keyboard'); } },
    // Lecteur de shows (T-444), TIMELINE: the same remotes' keys.
    { id: 'showlist.next', group: 'Lecteur de shows', codes: ['PageDown'], zones: ['timeline', 'tl-canvas'], show: true,
      where: 'TIMELINE', label: 'Show suivant de la liste', when: () => SL.items.length > 0,
      down: e => { if (!e.repeat) slAct('next'); } },
    { id: 'showlist.prev', group: 'Lecteur de shows', codes: ['PageUp'], zones: ['timeline', 'tl-canvas'], show: true,
      where: 'TIMELINE', label: 'Show précédent de la liste', when: () => SL.items.length > 0,
      down: e => { if (!e.repeat) slAct('prev'); } },
    { id: 'list.top', group: 'Liste de cues', codes: ['Home'], zones: ['live'], where: 'LIVE (hors grille active)', show: true,
      label: 'Début : avant le premier pas, sortie au noir', when: () => !!cqPlayer,
      down: e => { if (!e.repeat) cqAct('top', 'keyboard'); } },
    { id: 'list.pause', group: 'Liste de cues', codes: ['KeyP'], alt: true, zones: LIVE_ZONES, show: true,
      label: 'Pause de la liste (GO reprend)', when: () => !!cqPlayer,
      down: e => { if (!e.repeat) cqAct('pause', 'keyboard'); } },
    { id: 'reverse', group: 'Direct', keys: ['<'], meta: '*', alt: '*', shift: '*', zones: LIVE_ZONES, show: true,
      label: 'Tenir : inverser la rotation',
      down: () => { if (!reverseKey) { assignReverseKey(true); reverse(true); } },
      up: () => { if (reverseKey) { assignReverseKey(false); reverse(false); } } },
    // Hold S (T-103): strobe 1/4 while held. In LIVE, S is a cue letter, so Alt+S.
    { id: 'strobe.live', group: 'Direct', codes: ['KeyS'], alt: true, zones: LIVE_ZONES, show: true,
      label: 'Tenir : stroboscope 1/4 (LIVE)',
      down: () => holdStrobe(true), up: () => holdStrobe(false) },
    { id: 'strobe', group: 'Direct', codes: ['KeyS'], alt: '*', zones: NOT_LIVE, show: true,
      label: 'Tenir : stroboscope 1/4 (hors LIVE)',
      down: () => holdStrobe(true), up: () => holdStrobe(false) },
    // Brouillard (T-453): Alt+B held in LIVE (B alone is a cue key, Maj+B
    // its flash), once a pump channel is chosen; the max burst still applies.
    { id: 'haze', group: 'Direct', codes: ['KeyB'], alt: true, zones: LIVE_ZONES, show: true,
      label: "Tenir : brouillard (sortie DMX, au plus la durée max d'un jet)",
      when: () => !$('hazeBox').hidden,
      down: e => { if (!e.repeat) setHazeHeld(true); }, up: () => setHazeHeld(false) },
    // Grand afficheur du timecode (T-409), in every workspace.
    { id: 'tc.big', group: 'Timeline', codes: ['KeyT'], alt: true, zones: '*', show: true,
      label: 'Grand afficheur du timecode (ouvrir / fermer)',
      down: e => { if (!e.repeat) toggleTcBig(); } },
    // Entrées externes (T-408).
    { id: 'gate', group: 'Direct', codes: ['KeyX'], alt: true, shift: '*', zones: '*', show: true,
      label: 'Ouvrir / fermer les entrées externes (DMX, OSC, timecode)',
      down: e => { if (!e.repeat) setGate(!(ext && ext.open), 'keyboard'); } },
    // Cues utilisateur (T-405), in LIVE. Never a lone letter (cues).
    { id: 'cue.store', group: 'Cues et pages', codes: ['KeyE'], meta: true, zones: '*', where: 'LIVE', show: false,
      label: 'Enregistrer le look dans une case (LIVE)',
      down: e => { if (!e.repeat && activeWs === 'live') setStoreMode(!storeMode); } },
    { id: 'cue.update', group: 'Cues et pages', codes: ['KeyE'], meta: true, shift: true, zones: '*', where: 'LIVE', show: false,
      label: 'Mettre à jour la cue jouée avec le look (LIVE)',
      down: e => { if (!e.repeat && activeWs === 'live') updateCue(); } },
    // Projet (T-286): also from a text field, like a document's Cmd+S.
    { id: 'project.save', group: 'Projet', keys: ['s'], meta: true, shift: '*', zones: '*', typing: true, show: true,
      label: 'Enregistrer le projet',
      down: e => { if (!e.repeat) projAction('save'); } },
    { id: 'project.ask.save', group: 'Projet', codes: ['Enter', 'NumpadEnter'], meta: '*', shift: '*', zones: ['ask'], typing: true, show: false,
      label: 'Enregistrer (question « Modifications non enregistrées »)',
      down: e => { if (!e.repeat) $('askSave').click(); } },
    { id: 'project.open', group: 'Projet', keys: ['o'], meta: true, shift: '*', zones: '*', typing: true, show: false,
      label: 'Ouvrir un projet',
      down: e => { if (!e.repeat) projAction('open'); } },
    // Annuler / Rétablir (T-285): the studio's history of edits, in LIVE,
    // CRÉATION and RÉGLAGES. TIMELINE and CRÉATION › Figures keep their own
    // undo (below); a text field keeps the browser's.
    { id: 'edit.undo', group: 'Édition', keys: ['z'], meta: true, zones: ['live', 'grid', 'creation', 'settings', 'desk'], show: true,
      label: 'Annuler la dernière modification (hors TIMELINE et éditeur de figures)',
      down: e => { if (!e.repeat) historyStep('undo'); } },
    { id: 'edit.redo', group: 'Édition', keys: ['z'], meta: true, shift: true, zones: ['live', 'grid', 'creation', 'settings', 'desk'], show: true,
      label: 'Rétablir la modification annulée',
      down: e => { if (!e.repeat) historyStep('redo'); } },
    { id: 'edit.redo.y', group: 'Édition', keys: ['y'], meta: true, shift: '*', zones: ['live', 'grid', 'creation', 'settings', 'desk'], show: false,
      label: 'Rétablir', down: e => { if (!e.repeat) historyStep('redo'); } },
    // TIMELINE (T-285): undo anywhere in the workspace; the rest while the
    // timeline itself has the focus.
    { id: 'tl.undo', group: 'Timeline', keys: ['z'], meta: true, zones: ['timeline', 'tl-canvas'], show: false,
      label: 'Annuler', when: () => !!TL.show, down: () => tlUndo() },
    { id: 'tl.redo', group: 'Timeline', keys: ['z'], meta: true, shift: true, zones: ['timeline', 'tl-canvas'], show: false,
      label: 'Rétablir', when: () => !!TL.show, down: () => tlRedo() },
    { id: 'tl.redo.y', group: 'Timeline', keys: ['y'], meta: true, shift: '*', zones: ['timeline', 'tl-canvas'], show: false,
      label: 'Rétablir', when: () => !!TL.show, down: () => tlRedo() },
    { id: 'tl.marker', group: 'Timeline', codes: ['Enter', 'NumpadEnter'], alt: '*', shift: '*', zones: ['tl-canvas'], show: false,
      label: 'Marqueur à la tête de lecture (timeline active)', down: () => tlAddMarker(tlPlayhead()) },
    { id: 'tl.delete', group: 'Timeline', keys: ['Delete', 'Backspace'], alt: '*', shift: '*', zones: ['tl-canvas'], show: false,
      cap: () => 'Suppr / ⌫', label: 'Supprimer la sélection (timeline active)', down: () => tlDelete() },
    { id: 'tl.copy', group: 'Timeline', keys: ['c'], meta: true, shift: '*', zones: ['tl-canvas'], show: false,
      label: 'Copier (timeline active)', down: () => tlCopy() },
    { id: 'tl.paste', group: 'Timeline', keys: ['v'], meta: true, shift: '*', zones: ['tl-canvas'], show: false,
      label: 'Coller à la tête de lecture (timeline active)', down: () => tlPaste() },
    { id: 'tl.duplicate', group: 'Timeline', keys: ['d'], meta: true, shift: '*', zones: ['tl-canvas'], show: false,
      label: 'Dupliquer (timeline active)', down: () => tlDuplicate() },
    // Verrou de piste (T-445): by code, Alt changes the character on macOS.
    { id: 'tl.lock', group: 'Timeline', codes: ['KeyL'], alt: true, zones: ['tl-canvas'], show: false,
      label: 'Verrouiller / déverrouiller la piste (timeline active)', down: e => { if (!e.repeat) tlLockKey(); } },
    { id: 'tl.lock.all', group: 'Timeline', codes: ['KeyL'], alt: true, shift: true, zones: ['tl-canvas'], show: false,
      label: 'Tout verrouiller / déverrouiller (timeline active)', down: e => { if (!e.repeat) tlLockAll(); } },
    { id: 'tl.all', group: 'Timeline', keys: ['a'], meta: true, shift: '*', zones: ['tl-canvas'], show: false,
      label: 'Tout sélectionner (timeline active)',
      down: () => { TL.sel = new Set(tlEvents().map(v => v.e.id)); TL.need = true; } },
    // CRÉATION › Figures (T-296).
    { id: 'fig.undo', group: 'Figures', keys: ['z'], meta: true, zones: ['figures'], show: false,
      label: 'Annuler', down: () => figUndo() },
    { id: 'fig.redo', group: 'Figures', keys: ['z'], meta: true, shift: true, zones: ['figures'], show: false,
      label: 'Rétablir', down: () => figRedo() },
    { id: 'fig.redo.y', group: 'Figures', keys: ['y'], meta: true, shift: '*', zones: ['figures'], show: false,
      label: 'Rétablir', down: () => figRedo() },
    { id: 'fig.delete', group: 'Figures', keys: ['Delete'], alt: '*', shift: '*', zones: ['figures'], show: false,
      label: 'Supprimer les points sélectionnés', down: () => deleteSelected() },
    // Recherche globale (T-429): also from a text field (its own field
    // included, to close it); it replaces the browser's find on this page.
    { id: 'search', group: 'Recherche', keys: ['k', 'f'], meta: true, zones: '*', typing: true, show: true,
      cap: () => 'K ou F', label: 'Rechercher : aller à un cue, une page, une scène, un show, un contrôle, un réglage (ouvrir / fermer)',
      down: e => { if (!e.repeat) toggleSearch(); } },
    // Ligne de commande (T-424): the same box; Cmd+K stays the search.
    { id: 'command', group: 'Recherche', keys: ['k'], meta: true, shift: true, zones: '*', typing: true, show: true,
      label: 'Ligne de commande : régler un contrôle, lancer un cue, page, tempo, noir (ouvrir / fermer ; n\'arme jamais)',
      down: e => { if (!e.repeat) toggleCommand(); } },
    // Scripts (T-458): Cmd+Entrée in the editor = « Lancer » (only there).
    { id: 'script.run', group: 'Édition', codes: ['Enter', 'NumpadEnter'], meta: true, zones: ['script'], typing: true, show: true,
      label: 'Lancer le script (dans l\'éditeur de CRÉATION › Scripts ; n\'arme jamais)', down: e => { if (!e.repeat) scrRun(); } },
    { id: 'help', group: 'Aide', keys: ['?'], shift: '*', zones: '*', show: true,
      label: 'Afficher / masquer cette aide',
      down: e => { if (!e.repeat) toggleKeysHelp(); } },
    // Exécuteurs (T-418), LIVE: the top-row digits (read by position, so
    // AZERTY needs no Maj) hold button 1 of the page shown; Alt + Page
    // changes the executor page (Page alone is the cue list's Go / Retour).
    { id: 'exec.button', group: 'Exécuteurs', codes: ['Digit1', 'Digit2', 'Digit3', 'Digit4', 'Digit5', 'Digit6', 'Digit7', 'Digit8'], zones: LIVE_ZONES, show: true,
      cap: () => '1 … 8', label: "Bouton 1 de l'exécuteur 1 à 8 de la page affichée (tenu tant que la touche est enfoncée)",
      down: (e, i) => execKeyDown(e, i), up: execKeyUp },
    { id: 'exec.page.next', group: 'Exécuteurs', codes: ['PageDown'], alt: true, zones: LIVE_ZONES, show: true,
      label: "Page d'exécuteurs suivante", down: e => { if (!e.repeat) control('exec.page.next', 1); } },
    { id: 'exec.page.prev', group: 'Exécuteurs', codes: ['PageUp'], alt: true, zones: LIVE_ZONES, show: true,
      label: "Page d'exécuteurs précédente", down: e => { if (!e.repeat) control('exec.page.prev', 1); } },
    // Macros (T-425): Alt + the macro's letter, LIVE only (letters alone
    // are the cues, Alt + digit the pages).
    { id: 'macro', group: 'Macros', codes: MACRO_LETTERS.map(l => 'Key' + l), alt: true, zones: LIVE_ZONES, show: true,
      cap: () => 'A … Z', label: 'Lancer la macro liée à cette lettre (CRÉATION › Macros ; sauf B, C, P, R, S, T, X)',
      down: (e, i) => { const m = macroForKey(MACRO_LETTERS[i]); if (!m) return false; if (!e.repeat) macRun(m.id, 'keyboard'); } },
    // Cue letters, LIVE only (T-272); Maj = flash.
    { id: 'cue', group: 'Cues et pages', codes: CUE_CODES, shift: '*', zones: LIVE_ZONES, show: true,
      cap: cueCaps, label: 'Jouer la cue de la case (Maj : flash, tant que la touche est tenue)',
      down: (e, i) => cellKeyDown(e, i), up: cellKeyUp },
    // Grille au clavier (T-436), while the focus is in the grid: never
    // Espace (arming) nor Entrée (tap), which keep their role there.
    { id: 'grid.move', group: 'Cues et pages', codes: ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'], zones: ['grid'], show: true,
      where: 'LIVE (grille active)', cap: () => '← → ↑ ↓ Début Fin', label: 'Aller de case en case dans la grille de cues (Tab y entre)',
      down: (e, i) => gridMove(['left', 'right', 'up', 'down', 'home', 'end'][i]) },
    { id: 'grid.activate', group: 'Cues et pages', codes: ['Digit0', 'Numpad0'], shift: '*', zones: ['grid'], show: true,
      where: 'LIVE (grille active)', label: 'Déclencher la case sélectionnée (tenir : joue jusqu\'au relâchement en Flash ; Maj : flash)',
      down: e => { const i = focusedCell(); return i < 0 ? false : cellKeyDown(e, i); }, up: cellKeyUp },
  ];
}
// A key on a cell (its letter, or `0` on the focused one): the same
// press as a click, released with the key.
function cellKeyDown(e, i) {
  if (gridEditing) return;
  // « Enregistrer dans une case »: the key picks the cell.
  if (storeMode) { if (!e.repeat) storeInto(i); return; }
  const id = grid.pages[gridPage]?.cells[i] ?? null;
  if (!id || !presetById.has(id)) return false;
  // « Préparer » on (T-274): the key prepares, nothing goes out.
  if (prepMode) { if (!e.repeat) prepareCue(id); return; }
  // Auto-repeat would toggle the cue on and off while the key is held.
  if (e.repeat || heldKeys.has(e.code)) return;
  heldKeys.set(e.code, id);
  cueDown(id, shiftMod(e) ? 'flash' : null);
}
function cellKeyUp(e) {
  const id = heldKeys.get(e.code);
  if (id === undefined) return;
  heldKeys.delete(e.code);
  cueUp(id);
}
function allShortcuts() {
  if (!restShortcuts) {
    try { restShortcuts = buildShortcuts(); } catch (err) { console.error('raccourcis', err); return SAFETY_SHORTCUTS; }
  }
  // The hold key follows the settings.
  const hold = restShortcuts.find(s => s.id === 'hold');
  if (hold) hold.codes = [presence.hold_key];
  return SAFETY_SHORTCUTS.concat(restShortcuts);
}
// Where the keyboard is: the workspace, or a part of it with its own keys.
function shortcutZone() {
  if (!$('projAsk').hidden) return 'ask';
  if (activeWs === 'live') return document.activeElement?.closest?.('#cues') ? 'grid' : 'live';
  if (activeWs === 'timeline') return TL.show && document.activeElement === tlCv ? 'tl-canvas' : 'timeline';
  if (activeWs === 'creation') {
    if (document.activeElement === $('scrSource')) return 'script';
    return !$('figEditor').hidden && !$('figImportDlg').open ? 'figures' : 'creation';
  }
  if (activeWs === 'desk') return 'desk';
  return 'settings';
}
const modOk = (want, has) => want === '*' || !!want === has;
// Index of the key in s.codes / s.keys, or -1.
function shortcutKeyIndex(s, e) {
  if (s.codes) return s.codes.indexOf(e.code);
  return s.keys.findIndex(k => k.length === 1 ? (e.key || '').toLowerCase() === k : e.key === k);
}
function shortcutFor(e, list, zone, typing) {
  for (const s of list) {
    const i = shortcutKeyIndex(s, e);
    if (i < 0) continue;
    if (!modOk(s.meta, e.metaKey || e.ctrlKey) || !modOk(s.alt, e.altKey) || !modOk(s.shift, shiftMod(e))) continue;
    if (s.zones !== '*' && (zone === null || !s.zones.includes(zone))) continue;
    if (typing && !s.typing) continue;
    if (s.when && !s.when()) continue;
    return { s, i };
  }
  return null;
}
document.addEventListener('keydown', e => {
  // Échap before anything else, whatever has focus.
  const stop = shortcutFor(e, SAFETY_SHORTCUTS, null, false);
  if (stop) { e.preventDefault(); stop.s.down(e, stop.i); return; }
  downCodes.add(e.code);
  if (pickingHoldKey) { e.preventDefault(); pickHoldKey(e.code); return; }
  const hit = shortcutFor(e, allShortcuts(), shortcutZone(), isTyping());
  if (!hit || !vueAllows(hit.s)) return;
  if (hit.s.down(e, hit.i) !== false) e.preventDefault();
});
document.addEventListener('keyup', e => {
  downCodes.delete(e.code);
  for (const s of allShortcuts()) if (s.up && shortcutKeyIndex(s, e) >= 0) s.up(e);
});
// Two entries clash when one key press could match both: same key (a
// character counts as its QWERTY and AZERTY positions), modifiers that can
// both hold, and a zone in common. `when` conditions are not counted.
const AZERTY_SWAP = { a: 'q', q: 'a', z: 'w', w: 'z' };
function shortcutTokens(s) {
  const t = new Set();
  for (const c of s.codes || []) {
    t.add('code:' + c);
    const m = /^Key([A-Z])$/.exec(c);
    if (m) { const l = m[1].toLowerCase(); t.add('key:' + l); t.add('key:' + (AZERTY_SWAP[l] || l)); }
    if (c === 'Semicolon') t.add('key:m');
  }
  for (const k of s.keys || []) {
    const l = k.toLowerCase();
    t.add('key:' + l);
    if (/^[a-z]$/.test(l)) {
      t.add('code:Key' + l.toUpperCase());
      t.add(l === 'm' ? 'code:Semicolon' : 'code:Key' + (AZERTY_SWAP[l] || l).toUpperCase());
    } else t.add('code:' + k);
  }
  return t;
}
const modsMeet = (a, b) => a === '*' || b === '*' || !!a === !!b;
function shortcutConflicts(list = allShortcuts()) {
  const out = [], fixed = list.filter(s => !s.dynamic);
  const zonesOf = s => s.zones === '*' ? SHORTCUT_ZONES : s.zones;
  for (let a = 0; a < fixed.length; a++) {
    for (let b = a + 1; b < fixed.length; b++) {
      const x = fixed[a], y = fixed[b];
      if (!['meta', 'alt', 'shift'].every(m => modsMeet(x[m], y[m]))) continue;
      if (!zonesOf(x).some(z => zonesOf(y).includes(z))) continue;
      const ty = shortcutTokens(y);
      const same = [...shortcutTokens(x)].filter(k => ty.has(k));
      if (same.length) out.push({ a: x.id, b: y.id, keys: same });
    }
  }
  return out;
}

// « ? » : the table, grouped. Keys shown as on this keyboard when the
// browser can tell (navigator.keyboard.getLayoutMap), else as on an AZERTY Mac.
const SHORTCUT_GROUPS = ['Sécurité', 'Cues et pages', 'Liste de cues', 'Exécuteurs', 'Pupitre', 'Tempo', 'Direct', 'Vue 3D', 'Projet', 'Édition', 'Espaces de travail', 'Vues', 'Recherche', 'Timeline', 'Figures', 'Aide'];
const ZONE_FR = { script: 'CRÉATION › Scripts (éditeur)', live: 'LIVE', grid: 'LIVE (grille active)', timeline: 'TIMELINE', 'tl-canvas': 'TIMELINE active', creation: 'CRÉATION', figures: 'CRÉATION › Figures', settings: 'RÉGLAGES', desk: 'PUPITRE' };
const KEY_CAPS = { Digit0: '0', Numpad0: '0 (pavé)', Escape: 'Échap', Space: 'Espace', Enter: 'Entrée', NumpadEnter: 'Entrée', PageDown: 'Page suivante', PageUp: 'Page précédente', Home: 'Début', Backspace: '⌫', Delete: 'Suppr', '<': '<', '?': '?' };
const IS_MAC = /Mac|iP(hone|ad)/.test(navigator.platform || navigator.userAgent);
let keyLayout = null;
function keyCap(code) {
  const k = keyLayout && keyLayout.get(code);
  if (k) return k.length === 1 ? k.toUpperCase() : k;
  if (KEY_CAPS[code]) return KEY_CAPS[code];
  const i = CUE_CODES.indexOf(code);
  if (i >= 0) return CUE_KEYS[i].toUpperCase();
  return code.replace(/^(Key|Digit)/, '');
}
function cueCaps() { return CUE_CODES.slice(0, 10).map(keyCap).join(' ') + ' …'; }
function shortcutCaps(s) {
  const parts = [];
  if (s.meta === true) parts.push(IS_MAC ? 'Cmd' : 'Ctrl');
  if (s.alt === true) parts.push('Alt');
  if (s.shift === true) parts.push('Maj');
  const main = s.cap ? s.cap() : s.codes ? keyCap(s.codes[0]) : (KEY_CAPS[s.keys[0]] || s.keys[0].toUpperCase());
  parts.push(main);
  return parts;
}
function shortcutWhere(s) {
  if (s.where) return s.where;
  if (s.zones === '*') return 'partout';
  if (s.zones === NOT_TL_CANVAS) return 'partout (sauf timeline active)';
  if (s.zones === NOT_LIVE) return 'hors LIVE';
  if (s.zones === LIVE_ZONES) return 'LIVE';
  if (s.zones.length === 2 && s.zones.includes('timeline') && s.zones.includes('tl-canvas')) return 'TIMELINE';
  return s.zones.filter(z => z !== 'grid' || !s.zones.includes('live')).map(z => ZONE_FR[z]).join(', ');
}
function renderKeysHelp() {
  const box = $('keysList');
  box.textContent = '';
  const list = allShortcuts();
  for (const g of SHORTCUT_GROUPS) {
    const rows = list.filter(s => s.group === g);
    if (!rows.length) continue;
    const sec = document.createElement('div');
    sec.className = 'keys-group';
    sec.dataset.group = g;
    const h = document.createElement('h4');
    h.textContent = g;
    sec.appendChild(h);
    for (const s of rows) {
      const row = document.createElement('div');
      row.className = 'keys-row';
      row.dataset.shortcut = s.id;
      const caps = document.createElement('span');
      caps.className = 'caps';
      shortcutCaps(s).forEach((c, n) => {
        if (n) caps.append('+');
        const k = document.createElement('kbd'); k.textContent = c; caps.appendChild(k);
      });
      const label = document.createElement('span');
      label.className = 'what';
      label.textContent = s.label;
      const tags = document.createElement('span');
      tags.className = 'tags';
      const tag = (text, cls) => { const t = document.createElement('span'); t.className = 'tag ' + cls; t.textContent = text; tags.appendChild(t); };
      tag(shortcutWhere(s), 'where');
      if (s.typing) tag('aussi en saisie', 'typing');
      if (!s.show) tag('verrouillé en spectacle', 'locked');
      row.append(caps, label, tags);
      sec.appendChild(row);
    }
    box.appendChild(sec);
  }
}
async function openKeysHelp() {
  renderKeysHelp();
  $('keysHelp').hidden = false;
  try {
    if (!keyLayout && navigator.keyboard?.getLayoutMap) { keyLayout = await navigator.keyboard.getLayoutMap(); if (!$('keysHelp').hidden) renderKeysHelp(); }
  } catch { /* not allowed here: AZERTY labels */ }
}
function closeKeysHelp() { $('keysHelp').hidden = true; }
function toggleKeysHelp() { if ($('keysHelp').hidden) openKeysHelp(); else closeKeysHelp(); }
$('keysBtn').addEventListener('click', toggleKeysHelp);
$('keysClose').addEventListener('click', closeKeysHelp);
$('keysHelp').addEventListener('click', e => { if (e.target === $('keysHelp')) closeKeysHelp(); });
// For the e2e tests: the table and its conflicts.
window.shortcutsDebug = {
  list: () => allShortcuts().map(s => ({ id: s.id, group: s.group, codes: s.codes, keys: s.keys, meta: s.meta, alt: s.alt, shift: s.shift,
    zones: s.zones, typing: !!s.typing, show: !!s.show, dynamic: !!s.dynamic })),
  conflicts: (extra = []) => shortcutConflicts(allShortcuts().concat(extra)),
  zone: () => shortcutZone(),
};
// A key or button released while the window had no focus never sends its
// keyup: end every flash still held. Closing the window (or leaving the
// page) lets go the same way: another window may keep the studio alive,
// so the presence timeout would not (T-281).
window.addEventListener('blur', releaseHeld);
window.addEventListener('pagehide', () => { assignPageLeaving(true); releaseHeld(); });
function releaseHeld() {
  for (const id of heldKeys.values()) cueUp(id);
  heldKeys.clear();
  if (pointerCue) { cueUp(pointerCue); assignPointerCue(null); }
  downCodes.clear();
  setHold(false);
  holdStrobe(false);
  // Live effect hold buttons (T-142), defined further down.
  try { FX_HOLDS.forEach(id => fxHold(id, false)); } catch (err) { console.warn('effets', err); }
  // Executor buttons held by a key or the pointer (T-418).
  try { execReleaseAll(); } catch (err) { console.warn('exécuteurs', err); }
  // PUPITRE buttons and pads held by the pointer (T-427).
  try { deskReleaseAll(); } catch (err) { console.warn('pupitre', err); }
}
// The look's strobe before S was held (null = not held). Released, the
// look gets its own strobe back.
let strobeHeldFrom = null;
function holdStrobe(on) {
  if (!settings || on === (strobeHeldFrom !== null)) return;
  if (on) {
    strobeHeldFrom = settings.strobe_div ?? 0;
    editLook(s => { s.strobe_div = 4; });
  } else {
    const was = strobeHeldFrom;
    strobeHeldFrom = null;
    editLook(s => { s.strobe_div = was; });
  }
  showSettings();
}

export { IS_MAC, MACRO_LETTERS, afterEscape, arm, armAnsweredAt, assignArm, assignChecklistState,
  assignShownetState, checklistState, clock, closeKeysHelp, emergencyStop, openKeysHelp, showArmed,
  showPowerWatch, showShownet, shownetState, syncFill };
