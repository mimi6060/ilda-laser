// Laser Studio — core.js (T-435).
// Sections (as in the former index.html script):
//   notification centre (T-432)
//   Annuler / Rétablir / Historique (T-285)
//   unsaved work (T-433)
//   settings <-> controls
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { LOCKED_FR, flash, showMode } from './workspaces.js';
import { loadCueLists } from './cuelist.js';
import { loadProjection } from './zones.js';
import { showTargetPicker } from './targets.js';
import { lastTimeline } from './shows.js';
import { GEN_DEFAULTS, showGenHelp } from './layers.js';
import { reloadVenue } from './venue.js';
import { afterOpen, project } from './project.js';
import { figureDirty } from './figures.js';
import { pg, pgDirty } from './program.js';

const $ = id => document.getElementById(id);
let settings = null, calibration = null, armed = false, pps = 30000;
function assignArmed(v) { return (armed = v); }
function assignCalibration(v) { return (calibration = v); }
function assignPps(v) { return (pps = v); }
// Generators whose Forme A is a rotation speed in turns per beat, and the
// « Vitesse » presets (research §4.1: glacial … very fast).
const ROTATING = new Set(['finger_tunnel', 'twin_tunnel', 'sunburst']);
const TURNS = [0.015625, 0.03125, 0.0625, 0.25, 0.5];
// Fenêtre supplémentaire (T-281): ?vue=sortie|grille|direct|3d shows one
// region of this interface (set up further down, « Fenêtres »); null = the
// whole interface. Read first: the layout code below obeys it.
const VUES = ['sortie', 'grille', 'direct', '3d'];
const VUE = (v => VUES.includes(v) ? v : null)(new URLSearchParams(location.search).get('vue'));
// The layout keys a window never writes (WS_KEY, PANEL_KEY, STAGE_KEY).
const VUE_LAYOUT_KEYS = ['laserStudio.workspace', 'laserStudio.livePanel', 'laserStudio.stageTab'];
const VUE_RENDU = (v => ['3d', 'both'].includes(v) ? v : '2d')(new URLSearchParams(location.search).get('rendu'));
if (VUE) document.body.dataset.vue = VUE;
// Set when the page goes away: the last requests (releasing what this
// window held) must outlive it.
let pageLeaving = false;
function assignPageLeaving(v) { return (pageLeaving = v); }
// Windows talk to each other here: { type: 'page', page } and
// { type: 'select', id } (the cue last pressed, marked in the others).
const winChan = 'BroadcastChannel' in window ? new BroadcastChannel('laser-studio') : null;
let linkedCue = null;
function assignLinkedCue(v) { return (linkedCue = v); }

// ---------- notification centre (T-432) ----------
// One place where what went wrong is said, and stays readable: every
// request goes through api(), whose refusals and failures become
// notifications (notify): a toast at the bottom right (never over the top
// bar or the safety banners) and a line in the list behind the bell (the
// last 200). A condition (`key` + `sticky`: output error, studio
// unreachable, MIDI controller lost…) stays pinned at the top of the list
// while it lasts and resolves by itself. The safety banners keep their
// place and priority; they are only recorded here. The studio's own events
// (a file it could not write, autosave, audio input) come from
// GET /api/notices when `notices_last` moves in the state. In memory only.
// No keyboard shortcut: Échap stays the emergency stop.
const NOTICE_MAX = 200, TOAST_MAX = 3, NOTICE_DEDUP_MS = 2000;
const NOTICE_LEVELS = { error: 'Erreur', warning: 'Avertissement', info: 'Info' };
const NOTICE_TOAST_MS = { error: 8000, warning: 5000, info: 4000 };
// list: newest first; active: pinned conditions by key; toasts shown, then waiting.
const NC = { list: [], active: new Map(), seq: 0, open: false, toasts: [], waiting: [], drawPending: false, serverLast: 0, serverBusy: false, loadedAt: Date.now() };

function notify({ level = 'error', source = 'Studio', text = '', key = null, sticky = false, toast = true, at = null }) {
  if (!NOTICE_LEVELS[level]) level = 'error';
  text = String(text).trim().slice(0, 600) || 'erreur sans message';
  const now = Date.now();
  if (key && sticky) {
    const cur = NC.active.get(key);
    if (cur) {
      // Still the same condition: nothing new. Changed: shown again.
      if (cur.text === text && cur.level === level) return cur;
      Object.assign(cur, { text, level, last: now, read: NC.open });
      drawNoticesSoon();
      return cur;
    }
  } else {
    // The same one again within 2 s (a poll, a held key): one line, a count.
    const dup = NC.list.find(n => !n.sticky && now - n.last < NOTICE_DEDUP_MS
      && (key ? n.key === key : !n.key && n.source === source && n.text === text));
    if (dup) {
      dup.count++; dup.last = now; dup.read = NC.open;
      toastCount(dup); drawNoticesSoon();
      return dup;
    }
  }
  const n = { id: ++NC.seq, at: at ?? now, last: now, level, source, text, key, sticky: !!(key && sticky), count: 1, read: NC.open, resolved: false };
  NC.list.unshift(n);
  if (n.sticky) NC.active.set(key, n);
  // The oldest go first; a condition still on stays.
  for (let i = NC.list.length - 1; NC.list.length > NOTICE_MAX && i >= 0; i--) {
    if (!NC.active.has(NC.list[i].key) || NC.active.get(NC.list[i].key) !== NC.list[i]) NC.list.splice(i, 1);
  }
  if (toast) showToast(n);
  drawNoticesSoon();
  return n;
}
function resolveNotice(key) {
  const n = NC.active.get(key);
  if (!n) return;
  NC.active.delete(key);
  n.resolved = true;
  dropToast(n);
  drawNoticesSoon();
}
// A condition, checked with each state: pinned while `on`, resolved after.
function noticeCondition(key, on, notice) {
  if (on) notify({ ...notice, key, sticky: true }); else if (NC.active.has(key)) resolveNotice(key);
}

function showToast(n) {
  if (NC.toasts.some(t => t.n === n) || NC.waiting.includes(n)) return;
  if (NC.toasts.length >= TOAST_MAX) { NC.waiting.push(n); return; }
  const el = document.createElement('div');
  el.className = 'toast ' + n.level;
  if (n.level === 'error') el.setAttribute('role', 'alert');
  const head = document.createElement('div'); head.className = 'toast-head';
  const count = document.createElement('span'); count.className = 'n-count';
  head.append(`${NOTICE_LEVELS[n.level]} · ${n.source}`, count);
  const text = document.createElement('div'); text.className = 'toast-text'; text.textContent = n.text;
  const x = document.createElement('button'); x.className = 'toast-x'; x.textContent = '×';
  x.title = 'Fermer'; x.setAttribute('aria-label', 'Fermer la notification');
  const t = { n, el, count, timer: null };
  x.onclick = () => { n.read = true; dropToast(n); drawNoticesSoon(); };
  // Hovered: it stays.
  el.onmouseenter = () => clearTimeout(t.timer);
  el.onmouseleave = () => armToast(t);
  el.append(x, head, text);
  NC.toasts.push(t);
  $('toasts').appendChild(el);
  toastCount(n);
}
function armToast(t) {
  clearTimeout(t.timer);
  // A condition in error stays until closed or over.
  if ((t.n.sticky && t.n.level === 'error') || t.el.matches(':hover')) return;
  t.timer = setTimeout(() => dropToast(t.n), NOTICE_TOAST_MS[t.n.level]);
}
function toastCount(n) {
  const t = NC.toasts.find(q => q.n === n);
  if (!t) return;
  t.count.textContent = n.count > 1 ? ' ×' + n.count : '';
  armToast(t);
}
function dropToast(n) {
  NC.waiting = NC.waiting.filter(w => w !== n);
  const i = NC.toasts.findIndex(t => t.n === n);
  if (i < 0) return;
  clearTimeout(NC.toasts[i].timer);
  NC.toasts[i].el.remove();
  NC.toasts.splice(i, 1);
  const next = NC.waiting.shift();
  if (next) showToast(next);
}

// The bell, its count of unread lines (red with an error, orange with a
// warning) and, open, the panel: conditions on first, then newest first.
function drawNoticesSoon() {
  if (NC.drawPending) return;
  NC.drawPending = true;
  setTimeout(() => { NC.drawPending = false; drawNotices(); }, 0);
}
const noticeTime = ms => new Date(ms).toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
function noticeRow(n, pinned) {
  const row = document.createElement('div');
  row.className = `n-row ${n.level}${n.read ? '' : ' unread'}${n.resolved ? ' resolved' : ''}${pinned ? ' pinned' : ''}`;
  row.dataset.level = n.level;
  if (n.key) row.dataset.key = n.key;
  const lvl = document.createElement('span'); lvl.className = 'n-level'; lvl.textContent = pinned ? 'En cours' : NOTICE_LEVELS[n.level];
  const line = document.createElement('span'); line.className = 'n-line';
  line.textContent = `${noticeTime(n.at)} · ${n.source} · ${n.text}` + (n.resolved ? ' (résolu)' : '');
  const count = document.createElement('span'); count.className = 'n-count'; count.textContent = n.count > 1 ? '×' + n.count : '';
  row.append(lvl, line, count);
  return row;
}
function drawNotices() {
  const unread = NC.list.filter(n => !n.read);
  const on = [...NC.active.values()];
  const worst = list => list.some(n => n.level === 'error') ? 'error' : list.some(n => n.level === 'warning') ? 'warning' : 'info';
  const badge = $('noticeBadge'), btn = $('noticeBtn');
  badge.hidden = !unread.length;
  badge.textContent = unread.length > 99 ? '99+' : String(unread.length);
  badge.dataset.level = worst(unread);
  btn.dataset.level = on.length ? worst(on) : '';
  btn.setAttribute('aria-label', `Notifications${unread.length ? ` : ${unread.length} non lue${unread.length > 1 ? 's' : ''}` : ''}`);
  if (!NC.open) return;
  const pinned = NC.list.filter(n => NC.active.get(n.key) === n);
  $('noticePinned').replaceChildren(...pinned.map(n => noticeRow(n, true)));
  $('noticeList').replaceChildren(...NC.list.filter(n => !pinned.includes(n)).map(n => noticeRow(n, false)));
  $('noticeEmpty').hidden = NC.list.length > 0;
}
function setNoticePanel(open) {
  NC.open = open;
  $('noticePanel').hidden = !open;
  $('noticeBtn').setAttribute('aria-expanded', String(open));
  if (open) {
    const b = $('noticeBtn').getBoundingClientRect();
    $('noticePanel').style.top = Math.round(b.bottom + 6) + 'px';
    $('noticePanel').style.right = Math.max(8, Math.round(innerWidth - b.right)) + 'px';
    NC.list.forEach(n => { n.read = true; });
  }
  drawNotices();
}
$('noticeBtn').onclick = e => { e.stopPropagation(); setNoticePanel(!NC.open); };
$('noticeClear').onclick = () => {
  // The conditions still on stay.
  NC.list = NC.list.filter(n => NC.active.get(n.key) === n);
  drawNotices();
};
document.addEventListener('click', e => { if (NC.open && !e.target.closest('#noticePanel, #noticeBtn')) setNoticePanel(false); });

// ---------- Annuler / Rétablir / Historique (T-285) ----------
// The history is the studio's (history.rs): every edit route records an
// entry, with a French label. Cue presses, masters, tempo, arming, noir
// and the safety settings are never in it. Each state brings the counts
// (`history`); the panel reloads the list when its `rev` moves.
const HIST = { open: false, rev: -1, busy: false };
function showHistory(h) {
  if (!h) return;
  $('undoBtn').disabled = !h.undo;
  $('redoBtn').disabled = !h.redo;
  $('undoBtn').title = h.undo ? `Annuler : ${h.undo_label} (Cmd+Z)` : 'Rien à annuler';
  $('redoBtn').title = h.redo ? `Rétablir : ${h.redo_label} (Cmd+Maj+Z)` : 'Rien à rétablir';
  if (HIST.open && h.rev !== HIST.rev) drawHistory();
}
function historyTime(at) {
  const d = new Date(at);
  return isNaN(d) ? '' : d.toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}
async function drawHistory() {
  let h;
  try { h = await apiJson('GET', '/api/history', undefined, { quiet: true, cache: 'no-store' }); } catch { return; }
  HIST.rev = h.rev;
  const row = (e, redo) => {
    const b = document.createElement('button');
    b.className = 'h-row' + (redo ? ' redo' : '');
    b.dataset.hid = e.id;
    b.title = redo ? 'Rétablir jusqu\'à cette modification' : 'Annuler jusqu\'à cette modification (incluse)';
    const t = document.createElement('span'); t.className = 'h-time'; t.textContent = historyTime(e.at);
    const l = document.createElement('span'); l.className = 'h-label'; l.textContent = e.label;
    b.append(t, l);
    b.onclick = ev => { ev.stopPropagation(); historyStep(redo ? 'redo' : 'undo', e.id); };
    return b;
  };
  // Newest at the top: what was undone (greyed), then what can be undone.
  $('historyRedo').replaceChildren(...[...h.redo].reverse().map(e => row(e, true)));
  $('historyList').replaceChildren(...h.undo.map(e => row(e, false)));
  $('historyEmpty').hidden = h.undo.length + h.redo.length > 0;
}
function setHistoryPanel(open) {
  HIST.open = open;
  $('historyPanel').hidden = !open;
  $('historyBtn').setAttribute('aria-expanded', String(open));
  if (open) {
    const b = $('historyBtn').getBoundingClientRect();
    $('historyPanel').style.top = Math.round(b.bottom + 6) + 'px';
    $('historyPanel').style.left = Math.max(8, Math.min(Math.round(b.left), innerWidth - $('historyPanel').offsetWidth - 8)) + 'px';
    drawHistory();
  }
}
// One step (Cmd+Z, the buttons) or, with `to`, down to an entry (the panel).
async function historyStep(kind, to) {
  if (showMode) return flash(LOCKED_FR);
  if (HIST.busy) return;
  HIST.busy = true;
  let j;
  try { j = await apiJson('POST', '/api/' + kind, to === undefined ? {} : { to }, { source: 'Historique' }); } catch { /* api() said why */ } finally { HIST.busy = false; }
  if (HIST.open) drawHistory();
  if (!j) return;
  try {
    showHistory(j.history);
    const n = j.done.length;
    flash(`${kind === 'undo' ? 'Annulé' : 'Rétabli'} : ${j.done[n - 1]}${n > 1 ? ` (et ${n - 1} autre${n > 2 ? 's' : ''})` : ''}`);
    // Whatever the step touched is shown again from the studio.
    await afterOpen();
    loadCueLists(); loadProjection(); reloadVenue();
  } catch (e) { console.warn('historique', e); }
}
$('undoBtn').onclick = () => historyStep('undo');
$('redoBtn').onclick = () => historyStep('redo');
$('historyBtn').onclick = e => { e.stopPropagation(); setHistoryPanel(!HIST.open); };
document.addEventListener('click', e => { if (HIST.open && !e.target.closest('#historyPanel, #historyBtn')) setHistoryPanel(false); });

// The studio's own events (a write that failed, autosave, audio input).
async function pullNotices(last) {
  if (NC.serverBusy || last === NC.serverLast) return;
  if (last < NC.serverLast) NC.serverLast = 0; // the studio restarted
  NC.serverBusy = true;
  try {
    const j = await apiJson('GET', '/api/notices?since=' + NC.serverLast, undefined, { quiet: true });
    for (const s of j.notices) {
      const key = s.key ? 'studio:' + s.key : null;
      if (s.resolved) { if (key) resolveNotice(key); continue; }
      const at = Date.parse(s.at) || Date.now();
      // Older than the page: in the list, without a toast.
      notify({ level: s.level, source: s.source, text: s.text, key, sticky: !!key, at, toast: at >= NC.loadedAt - 5000 });
    }
    NC.serverLast = j.last;
  } catch { /* next state */ } finally { NC.serverBusy = false; }
}

// The one door to the studio: every request of the page goes through
// here (the heartbeat worker excepted: it runs apart and has no page). A
// refusal (4xx) or a failure (5xx) becomes a notification with the
// server's text (French already); a request without any answer pins
// « Studio injoignable » until the next answer. Options: `quiet` (true or
// statuses): the caller shows that refusal itself (under a field), it is
// only recorded, without a toast; `ignore` (statuses): normal flow for
// the caller, not recorded; `source`, `cache`, `keepalive`; `timeout`
// (ms): give up on an answer that never comes (throws a TimeoutError,
// left to the caller: one slow request is not a lost studio). A body is
// sent as JSON, except a file or bytes. Returns the Response; throws, as
// fetch does, when nothing answered. Nothing is awaited before the
// request leaves (the emergency stop goes through here).
const API_SOURCES = [
  [/^\/api\/(project|backup)/, 'Projet'],
  [/^\/api\/(timeline|shows|media\/audio|timecode)/, 'Timeline'],
  [/^\/api\/(figures|media\/ilda|sdbank)/, 'Figures'],
  [/^\/api\/(midi|dmx|netin|external)/, 'MIDI'],
  [/^\/api\/(arm|estop|safety|audience|checklist|presence|heartbeat|lock|data\/warnings|outputs\/limits|outputs\/color|venue)/, 'Sécurité'],
  [/^\/api\/audio/, 'Audio'],
  [/^\/api\/(cue|grid|presets|scan-presets|running|scenes|playlist|palettes|festival|control|live|lfos|layers|executors)/, 'Cues'],
];
const apiSource = path => (API_SOURCES.find(([re]) => re.test(path)) || [null, 'Réglages'])[1];
async function api(method, path, body, opts = {}) {
  const init = { method };
  if (opts.cache) init.cache = opts.cache;
  if (opts.keepalive || pageLeaving) init.keepalive = true;
  if (opts.timeout) init.signal = AbortSignal.timeout(opts.timeout);
  if (body !== undefined && method !== 'GET') {
    if (body instanceof Blob || body instanceof ArrayBuffer || ArrayBuffer.isView(body)) init.body = body;
    else { init.body = typeof body === 'string' ? body : JSON.stringify(body); init.headers = { 'Content-Type': 'application/json' }; }
  }
  if (method !== 'GET') noteEdit(path);
  let r;
  try { r = await fetch(path, init); } catch (e) { if (e?.name !== 'TimeoutError') apiUnreachable(); throw e; }
  if (NC.active.has('link')) resolveNotice('link');
  if (!r.ok && !(opts.ignore || []).includes(r.status)) apiRefused(method, path, r.clone(), opts);
  return r;
}
// api() and the JSON of a good answer; throws (after saying so) otherwise.
async function apiJson(method, path, body, opts = {}) {
  const r = await api(method, path, body, opts);
  if (!r.ok) throw new Error(`${path} : ${r.status}`);
  try { return await r.json(); } catch (e) {
    notify({ level: 'error', source: opts.source || apiSource(path), text: `réponse illisible du studio (${path.split('?')[0]})` });
    throw e;
  }
}
function apiUnreachable() {
  noticeCondition('link', true, { level: 'error', source: 'Studio', text: 'Studio injoignable : pas de réponse du serveur' });
}
async function apiRefused(method, path, r, opts) {
  let text = '';
  try { text = (await r.text()).trim(); } catch { /* body lost */ }
  try {
    const j = JSON.parse(text);
    if (j && typeof j === 'object') {
      text = j.error || j.message
        || (Array.isArray(j.blocking) ? 'armement impossible : ' + j.blocking.join(' ; ') : '')
        || (Array.isArray(j.loosen) ? 'assouplissement à confirmer : ' + j.loosen.join(' · ') : '') || text;
    }
  } catch { /* plain text */ }
  if (!text || text.startsWith('<')) text = `erreur ${r.status} (${method} ${path.split('?')[0]})`;
  const source = opts.source || apiSource(path);
  // Show mode (T-283): its own short message; recorded too.
  if (r.status === 423) { flash(text); notify({ level: 'info', source, text, toast: false }); return; }
  const quiet = opts.quiet === true || (Array.isArray(opts.quiet) && opts.quiet.includes(r.status));
  notify({ level: r.status >= 500 ? 'error' : 'warning', source, text, toast: !quiet });
}

// ---------- unsaved work (T-433) ----------
// What closing the page now would lose: the project (the studio's flag in
// each state, `project_modified`, refreshed every 500 ms, plus a local flag
// raised by every editing request so a reload within that time is caught),
// the timeline editor's show, the figure being drawn and the cue program
// being edited in CRÉATION. An autosave is not a save: the guard stays.
const NOT_EDITS = /^\/api\/(arm|estop|heartbeat|lock|notices|project|audio(\?|$)|external\/gate|timecode\/arm|data\/warnings|checklist|audience|keep_awake|testpattern|outputs|safety|presence|calibration|frame|onboarding|about|help|prepared|operators)/;
const DIRTY = { editAt: -1, savedAt: 0, server: false, serverAt: 0 };
function noteEdit(path) { if (!NOT_EDITS.test(path)) DIRTY.editAt = performance.now(); }
function takeProjectModified(modified, at) {
  if (modified === undefined) return;
  DIRTY.server = !!modified; DIRTY.serverAt = at;
}
function projectSaved() { DIRTY.savedAt = performance.now(); DIRTY.server = false; }
function projectDirty() {
  if (DIRTY.server) return true;
  // An edit the studio may not have looked at yet.
  return DIRTY.editAt > DIRTY.savedAt && DIRTY.serverAt - DIRTY.editAt < 1500;
}
// What « Modifications non enregistrées » lists, as { kind, label } (kind:
// project, timeline, figure); empty: nothing to lose. Called once the
// page has loaded (dialogs, closing).
function unsavedWork({ figure = true } = {}) {
  const out = [];
  if (projectDirty()) out.push({ kind: 'project', label: `Projet « ${project.name} »` });
  if (lastTimeline && lastTimeline.name && lastTimeline.modified) out.push({ kind: 'timeline', label: `Show de timeline « ${lastTimeline.name} »`, example: !!lastTimeline.example });
  if (figure && figureDirty()) out.push({ kind: 'figure', label: 'Figure en cours' });
  if (figure && pgDirty()) out.push({ kind: 'program', label: `Programme du cue « ${pg.info ? pg.info.name : pg.id} »` });
  return out;
}
window.addEventListener('beforeunload', e => {
  if (!unsavedWork().length) return;
  e.preventDefault();
  e.returnValue = ''; // the browser shows its own words
});

// A POST whose refusal api() reports (show mode: its short message).
async function post(path, body) {
  return (await api('POST', path, body ?? {})).ok;
}

// « La plus récente gagne » (T-484). A control moved continuously (fader,
// knob, pad, slider) posts many values; sent as they come, an older one
// can reach the studio after the newest and stay. Here one key (one
// control) has at most one request in flight: values posted meanwhile
// replace each other and only the newest leaves next, so the last value
// posted is always the last one the studio gets. Resolves like api() for
// the request that left, `null` for a value replaced before leaving.
// Never for a stop: blackout, e-stop, disarm and hold buttons call api()
// directly and are not delayed by this.
const LATEST = new Map();
function sendLatest(key, method, path, body, opts = {}) {
  let s = LATEST.get(key);
  if (!s) LATEST.set(key, s = { busy: false, next: null });
  return new Promise((resolve, reject) => {
    if (s.next) s.next.resolve(null);
    s.next = { method, path, body, opts, resolve, reject };
    if (!s.busy) latestPump(key, s);
  });
}
async function latestPump(key, s) {
  s.busy = true;
  while (s.next) {
    const { method, path, body, opts, resolve, reject } = s.next;
    s.next = null;
    try { resolve(await api(method, path, body, opts)); } catch (e) { reject(e); }
  }
  s.busy = false;
  if (LATEST.get(key) === s) LATEST.delete(key);
}
// post() through sendLatest: false only when the studio refused this value.
async function postLatest(key, path, body) {
  const r = await sendLatest(key, 'POST', path, body ?? {});
  return !r || r.ok;
}
// A control value (`/api/control`), latest wins per control id.
const controlLatest = body => postLatest('control:' + body.id, '/api/control', body);

// ---------- settings <-> controls ----------
const hex = c => '#' + c.map(v => v.toString(16).padStart(2, '0')).join('');
const rgb = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));

// The look panel follows the server's look (playlist, cues, MIDI) and
// never sends an old one back. `settingsRev` is the server revision our
// copy matches. An edit is a function setting one field; it is applied at
// once and kept until the server accepts it, so when the look changed
// elsewhere meanwhile, the page reloads it and replays its edits on top
// (the server refuses a post made on an older revision).
let settingsRev = -1, pendingEdits = [], pushTimer = null, lookQueue = Promise.resolve(), lookBusy = 0;

// A control being dragged or typed in keeps its value on screen.
const setVal = (id, v) => { if (document.activeElement !== $(id)) $(id).value = v; };
const setCheck = (id, v) => { if (document.activeElement !== $(id)) $(id).checked = v; };
// A top-bar text with a compact form (T-517): the full text stays the
// element's text (and title), the narrow bar shows data-short instead.
function setShortText(el, full, short) {
  if (el.textContent === full && el.dataset.short === short) return;
  const long = document.createElement('span');
  long.className = 'long';
  long.textContent = full;
  el.replaceChildren(long);
  el.dataset.short = short;
}

function showSettings() {
  const s = settings, kind = s.content.kind;
  document.querySelectorAll('[data-kind]').forEach(b => b.classList.toggle('active', b.dataset.kind === kind));
  $('shapePanel').hidden = kind !== 'shape';
  $('textPanel').hidden = kind !== 'text';
  $('wavePanel').hidden = kind !== 'wave';
  $('genPanel').hidden = kind !== 'generator';
  $('evoPanel').hidden = kind !== 'evolving';
  $('figNote').hidden = kind !== 'figure' && kind !== 'abstract' && kind !== 'wire';
  if (kind === 'wire') {
    const c = s.content;
    $('figNote').textContent = `Objet 3D « ${c.name} » : ${(c.edges || []).length} arêtes. Il se règle dans l'onglet Objets 3D. `
      + 'Taille, rotation et luminosité s\'appliquent par-dessus.';
  }
  if (kind === 'abstract') {
    const n = (s.content.oscs || []).length;
    $('figNote').textContent = `Abstrait : ${n} oscillateur${n > 1 ? 's' : ''}. Il se modifie dans l'onglet Abstraits (« Reprendre le look »). `
      + 'Couleur, taille, rotation et luminosité s\'appliquent par-dessus.';
  }
  if (kind === 'figure') {
    const c = s.content;
    $('figNote').textContent = `Figure « ${c.name} » : ${c.frames.length} image${c.frames.length > 1 ? 's' : ''}. `
      + 'Ses couleurs viennent de ses tracés ; elle se modifie dans l\'onglet Figures. Taille, rotation et luminosité s\'appliquent par-dessus.';
  }
  $('ildaPanel').hidden = kind !== 'ilda';
  if (kind === 'ilda') {
    $('ildaNote').textContent = `Fichier ILDA « ${s.content.file} », joué tel quel : ses points et ses couleurs. `
      + 'Il se gère dans l\'onglet Médiathèque. Taille, rotation et luminosité s\'appliquent par-dessus.';
    setVal('ildaFps', s.content.fps);
  }
  if (kind === 'evolving') {
    const c = s.content;
    $('evoPanel').textContent = `Cue évolutif : ${c.keys.length} clé${c.keys.length > 1 ? 's' : ''} sur ${c.length_beats} temps`
      + (c.loop ? ', en boucle' : ', une fois') + '. Les clés se modifient par l\'API (pas d\'éditeur pour l\'instant) ; '
      + 'Couleur et Taille viennent des clés, la luminosité s\'applique par-dessus.';
  }
  if (kind === 'generator') {
    const p = s.content.params;
    setVal('gen', s.content.generator);
    setVal('gCount', p.count); setVal('gA', p.a); setVal('gB', p.b); setVal('gSpeed', p.speed);
    setVal('gMode', p.color_mode); setVal('gColor2', hex(p.color2));
    if (document.activeElement !== $('gSync')) $('gSync').checked = !!p.beat_sync;
    $('gSyncPanel').hidden = !p.beat_sync;
    setVal('gPeriod', String(p.period_beats ?? 4)); setVal('gSteps', String(p.steps_per_beat ?? 1));
    setVal('gGate', p.gate_beats ?? 0); setVal('gDir', String(Math.sign(p.direction ?? 1)));
    setVal('gGroups', String(p.groups ?? 1)); setVal('gGroupMode', p.group_mode ?? 'unison');
    setVal('gEasing', p.easing ?? 'sine');
    setCheck('gSnap', !!p.snap);
    setCheck('gCrossFlash', !!p.cross_flash);
    setVal('gTurns', TURNS.includes(+p.a) ? String(+p.a) : '');
    setVal('gLoop', p.loop_mode ?? 'ping_pong');
    setVal('gChase', String(Math.min(7, Math.max(0, Math.round(p.a)))));
    setVal('gSolid', String(Math.min(4, Math.max(0, Math.round(p.a)))));
    setVal('gTargetMode', p.b >= 0.5 ? '1' : '0');
    if (s.content.generator === 'mirror_targets') showTargetPicker(p.a);
    showGenHelp(s.content.generator);
  }
  document.querySelectorAll('#shapes button').forEach(b => b.classList.toggle('active', kind === 'shape' && b.dataset.shape === s.content.shape));
  if (kind === 'text') showTextStyle(s.content);
  setVal('color', hex(s.color));
  setVal('scale', Math.round(s.scale * 100));
  setVal('rot', s.rotation_speed);
  setVal('bright', Math.round(s.brightness * 100));
  setVal('rGate', s.gate ?? 'off');
  setVal('rGateLen', s.gate_beats ?? 0.2);
  setCheck('rDecay', !!s.gate_decay);
  setVal('rStrobe', String(s.strobe_div ?? 0));
  setVal('rDuty', Math.round((s.strobe_duty ?? 0.4) * 100));
  $('rGateLen').disabled = (s.gate ?? 'off') === 'off' || !!s.gate_decay;
  $('rDuty').disabled = !(s.strobe_div > 0);
  setCheck('aOn', s.audio.enabled);
  setVal('aSize', Math.round(s.audio.size * 100));
  setVal('aRot', Math.round(s.audio.rotate * 100));
  setVal('aFlash', Math.round(s.audio.flash * 100));
  setCheck('aColor', s.audio.color_on_beat);
  showValues();
}

// The text options (T-439): absent style = the defaults (segment font).
const TEXT_STYLE = { font: 'segments', align: 'center', spacing: 0, fit_width: null, scroll: { enabled: false, speed: 0.25, per: 'beat' } };
const textStyle = c => {
  const st = c.style || {};
  return { ...TEXT_STYLE, ...st, scroll: { ...TEXT_STYLE.scroll, ...(st.scroll || {}) } };
};
// Which fonts draw accents, from /api/fonts (none known until it answers).
let FONT_ACCENTS = {};
apiJson('GET', '/api/fonts', undefined, { quiet: true }).then(list => {
  FONT_ACCENTS = Object.fromEntries(list.map(f => [f.id, f.accents]));
  if (settings?.content?.kind === 'text') showTextStyle(settings.content);
}).catch(() => {});
const ACCENTED = /[àâäáéèêëíìîïóòôöúùûüÿçœæ]/i;
function showTextStyle(c) {
  const st = textStyle(c);
  setVal('text', c.text);
  setVal('textFont', st.font);
  document.querySelectorAll('[data-talign]').forEach(b => b.classList.toggle('active', b.dataset.talign === st.align));
  setVal('textSpacing', st.spacing);
  setCheck('textFit', st.fit_width != null);
  setVal('textFitW', Math.round((st.fit_width ?? 1.6) * 50));
  $('textFitW').disabled = st.fit_width == null;
  setCheck('textScroll', st.scroll.enabled);
  setVal('textSpeed', st.scroll.speed);
  setVal('textPer', st.scroll.per);
  $('textSpeed').disabled = $('textPer').disabled = !st.scroll.enabled;
  document.querySelectorAll('[data-talign]').forEach(b => { b.disabled = st.scroll.enabled; });
  $('textAccentWarn').hidden = !(FONT_ACCENTS[st.font] === false && ACCENTED.test(c.text));
}

function showValues() {
  $('textSpacingV').textContent = (+$('textSpacing').value).toFixed(2);
  $('textFitWV').textContent = $('textFitW').value + ' %';
  $('textSpeedV').textContent = (+$('textSpeed').value).toFixed(2) + ($('textPer').value === 'second' ? ' /s' : ' /temps');
  $('scaleV').textContent = $('scale').value + ' %';
  $('rotV').textContent = $('rot').value + ' °/s';
  $('brightV').textContent = $('bright').value + ' %';
  $('aSizeV').textContent = $('aSize').value + ' %';
  $('aRotV').textContent = $('aRot').value + ' %';
  $('aFlashV').textContent = $('aFlash').value + ' %';
  $('gainV').textContent = '×' + (+$('gain').value).toFixed(1);
  $('gCountV').textContent = $('gCount').value;
  const a = +$('gA').value;
  $('gAV').textContent = ROTATING.has(settings?.content?.generator) ? a.toFixed(3) + ' tour/temps' : a.toFixed(2);
  $('gBV').textContent = (+$('gB').value).toFixed(2);
  $('gSpeedV').textContent = '×' + (+$('gSpeed').value).toFixed(2);
  $('gGateV').textContent = +$('gGate').value ? (+$('gGate').value).toFixed(2) : 'off';
  $('rGateLenV').textContent = (+$('rGateLen').value).toFixed(2);
  $('rDutyV').textContent = $('rDuty').value + ' %';
}

// One edit per control: the value it had when moved, written to one field.
const genEdit = f => s => { if (s.content.kind === 'generator') f(s.content); };
// A text option: on the full style (a look saved without one gets the defaults).
const textEdit = f => s => { if (s.content.kind === 'text') { s.content.style = textStyle(s.content); f(s.content.style); } };
const LOOK_EDITS = {
  color: v => s => { s.color = rgb(v); },
  scale: v => s => { s.scale = v / 100; },
  rot: v => s => { s.rotation_speed = +v; },
  bright: v => s => { s.brightness = v / 100; },
  aOn: v => s => { s.audio.enabled = v; },
  aSize: v => s => { s.audio.size = v / 100; },
  aRot: v => s => { s.audio.rotate = v / 100; },
  aFlash: v => s => { s.audio.flash = v / 100; },
  aColor: v => s => { s.audio.color_on_beat = v; },
  text: v => s => { if (s.content.kind === 'text') s.content.text = v; },
  textFont: v => textEdit(st => { st.font = v; }),
  textSpacing: v => textEdit(st => { st.spacing = +v; }),
  textFit: v => textEdit(st => { st.fit_width = v ? $('textFitW').value / 50 : null; }),
  textFitW: v => textEdit(st => { if (st.fit_width != null) st.fit_width = v / 50; }),
  textScroll: v => textEdit(st => { st.scroll.enabled = v; }),
  textSpeed: v => textEdit(st => { st.scroll.speed = +v; }),
  textPer: v => textEdit(st => { st.scroll.per = v; }),
  ildaFps: v => s => { if (s.content.kind === 'ilda' && +v > 0) s.content.fps = Math.min(100, Math.max(1, +v)); },
  rGate: v => s => { s.gate = v; },
  rGateLen: v => s => { s.gate_beats = +v; },
  rDecay: v => s => { s.gate_decay = v; },
  rStrobe: v => s => { s.strobe_div = +v; },
  rDuty: v => s => { s.strobe_duty = v / 100; },
  // Picking a festival fan or sheet starts it from its own starting values, in tempo.
  gen: v => genEdit(c => {
    c.generator = v;
    const d = GEN_DEFAULTS[v];
    if (d) Object.assign(c.params, { count: d.count, a: d.a, b: d.b, beat_sync: true, period_beats: d.period, steps_per_beat: d.steps },
      d.mode ? { color_mode: d.mode } : {});
  }),
  gCount: v => genEdit(c => { c.params.count = +v; }),
  gA: v => genEdit(c => { c.params.a = +v; }),
  gB: v => genEdit(c => { c.params.b = +v; }),
  gSpeed: v => genEdit(c => { c.params.speed = +v; }),
  gMode: v => genEdit(c => { c.params.color_mode = v; }),
  gColor2: v => genEdit(c => { c.params.color2 = rgb(v); }),
  gSync: v => genEdit(c => { c.params.beat_sync = v; }),
  gPeriod: v => genEdit(c => { c.params.period_beats = +v; }),
  gSteps: v => genEdit(c => { c.params.steps_per_beat = +v; }),
  gGate: v => genEdit(c => { c.params.gate_beats = +v; }),
  gDir: v => genEdit(c => { c.params.direction = +v; }),
  gGroups: v => genEdit(c => { c.params.groups = +v; }),
  gGroupMode: v => genEdit(c => { c.params.group_mode = v; }),
  gEasing: v => genEdit(c => { c.params.easing = v; }),
  gSnap: v => genEdit(c => { c.params.snap = v; }),
  gCrossFlash: v => genEdit(c => { c.params.cross_flash = v; }),
  gTurns: v => genEdit(c => { if (v !== '') c.params.a = +v; }),
  gLoop: v => genEdit(c => { c.params.loop_mode = v; }),
  gChase: v => genEdit(c => { c.params.a = +v; }),
  gSolid: v => genEdit(c => { c.params.a = +v; }),
  gTargetMode: v => genEdit(c => { c.params.b = +v; }),
};
for (const [id, edit] of Object.entries(LOOK_EDITS)) {
  $(id).addEventListener('input', () => {
    editLook(edit($(id).type === 'checkbox' ? $(id).checked : $(id).value));
    showValues();
  });
  // Shown again once let go, in case the look changed while it was held.
  if (id === 'gSync') $(id).addEventListener('input', () => { $('gSyncPanel').hidden = !$('gSync').checked; });
  $(id).addEventListener('blur', () => setTimeout(showSettings));
  // A new generator may bring its own values and help, and a « Vitesse »
  // preset moves Forme A: show them now.
  if (id === 'gen' || id === 'gTurns' || id === 'gChase' || id === 'gSolid' || id === 'rGate' || id === 'rDecay' || id === 'rStrobe'
    || id === 'textFont' || id === 'textFit' || id === 'textScroll' || id === 'textPer' || id === 'text') $(id).addEventListener('input', () => showSettings());
}

function editLook(fn) {
  fn(settings);
  pendingEdits.push(fn);
  clearTimeout(pushTimer);
  pushTimer = setTimeout(() => lookTask(sendLook), 40);
}

// Look requests run one at a time, in order.
function lookTask(task) {
  lookBusy++;
  lookQueue = lookQueue.then(task).catch(e => console.warn(e)).finally(() => lookBusy--);
  return lookQueue;
}

async function sendLook() {
  for (let tries = 0; tries < 3 && pendingEdits.length; tries++) {
    const sent = pendingEdits.length;
    const r = await api('POST', '/api/settings?rev=' + settingsRev, settings, { ignore: [409] });
    if (r.status === 409) { await loadLook(); continue; } // replaced meanwhile: replay on the new look
    pendingEdits.splice(0, sent);
    if (r.ok) settingsRev = (await r.json()).rev; else console.warn('/api/settings', await r.text());
    return;
  }
}

// Take the server's look (from /api/state), with our unsent edits on top.
function takeLook(st) {
  settings = st.settings; settingsRev = st.settings_rev;
  pendingEdits.forEach(f => f(settings));
  showSettings();
}
async function loadLook() {
  takeLook(await apiJson('GET', '/api/state'));
}

document.querySelectorAll('[data-talign]').forEach(b => b.addEventListener('click', () => {
  editLook(textEdit(st => { st.align = b.dataset.talign; }));
  showSettings();
}));

document.querySelectorAll('[data-kind]').forEach(b => b.addEventListener('click', () => {
  const kind = b.dataset.kind;
  let content;
  if (kind === 'shape') content = { kind, shape: settings.content.shape || 'circle' };
  if (kind === 'text') content = { kind, text: $('text').value || 'HELLO' };
  if (kind === 'wave') content = { kind };
  if (kind === 'generator') content = { kind, generator: 'lissajous',
    params: { count: 8, a: 3, b: 2, speed: 1, color_mode: 'rainbow', color2: [0, 0, 255] } };
  editLook(s => { s.content = structuredClone(content); });
  showSettings();
}));

export { $, HIST, NC, ROTATING, VUE, VUE_LAYOUT_KEYS, VUE_RENDU, api, apiJson, armed, assignArmed,
  assignCalibration, assignLinkedCue, assignPageLeaving, assignPps, calibration, controlLatest,
  editLook, genEdit, hex, historyStep, linkedCue, loadLook, lookBusy, lookTask, noticeCondition,
  notify, post, postLatest, pps, projectSaved, pullNotices, rgb, sendLatest, setCheck,
  setHistoryPanel, setNoticePanel, setShortText, setVal, settings, settingsRev, showHistory, showSettings,
  takeLook, takeProjectModified, unsavedWork, winChan };
