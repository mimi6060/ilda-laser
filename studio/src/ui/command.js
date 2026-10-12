// Laser Studio — command.js (T-435).
// Sections (as in the former index.html script):
//   ligne de commande (T-424)
//   états vides (T-437)
//   accessibilité (T-436)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { SEARCH, SEARCH_KINDS, SEARCH_ORDER, addHelpButtons, closeHelpMenu, closeSearch, helpLoad,
  maybeStartTour, searchBuild, searchIndex, searchPaintSel, searchQuery } from './help.js';
import { $, VUE, api, apiJson } from './core.js';
import { IS_MAC } from './laser.js';
import { loadPresence } from './presence.js';
import { previewLoop } from './state.js';
import { opTouched } from './workspaces.js';
import { loadTestPatterns } from './cuelist.js';
import { loadProjection } from './zones.js';
import { loadPalettes } from './direct.js';
import { loadPresets } from './cues.js';
import { loadAudioPresets, loadFxGrid, loadLfos, loadRoutes, loadTriggers } from './modulators.js';
import { dataWarnLoop } from './outputs.js';
import { loadState } from './venue.js';
import { midiLoop } from './midi.js';

// ---------- ligne de commande (T-424) ----------
// The search box in « commande » mode (Cmd+Maj+K, « ›_ Commandes », or
// « > » typed first in the search). The list: every control a line may
// drive and the cues by name (GET /api/command/actions); the line is
// parsed and run by the studio (POST /api/command), like a controller: it
// never arms, never resets the e-stop; « noir » latches the stop at once.
// Échap stays the e-stop (the page's handler runs first, then closes this).
const CMD = { items: [], nav: false, hi: -1, echo: '' };
const CMD_KINDS = [['cmd', 'Contrôles'], ['cmdcue', 'Cues']];
Object.assign(SEARCH_ORDER, { cmd: SEARCH_KINDS.length, cmdcue: SEARCH_KINDS.length + 1 });
const CMD_HISTORY_KEY = 'laserStudio.cmdHistory', CMD_HISTORY_MAX = 50;
// The last valid lines, newest first (this browser only).
function cmdHistory() {
  try { const h = JSON.parse(localStorage.getItem(CMD_HISTORY_KEY) || '[]'); return Array.isArray(h) ? h.filter(x => typeof x === 'string') : []; } catch { return []; }
}
function cmdRemember(line) {
  const h = [line, ...cmdHistory().filter(x => x !== line)].slice(0, CMD_HISTORY_MAX);
  try { localStorage.setItem(CMD_HISTORY_KEY, JSON.stringify(h)); } catch { /* private window: no history */ } opTouched();
}
const SEARCH_TEXT = {
  search: { label: 'Recherche globale', ph: 'Rechercher… (cue, page, scène, show, contrôle, réglage, aide)', input: 'Rechercher',
    hint: "↑ ↓ choisir · Entrée : y aller · Maj+Entrée : jouer le cue · « > » : commandes · Cmd+K : fermer · Échap reste l'arrêt d'urgence" },
  cmd: { label: 'Ligne de commande', ph: 'Tapez une action ou une commande…', input: 'Commande',
    hint: "↑ ↓ choisir (champ vide : dernières commandes) · Tab : compléter · Entrée : exécuter · Cmd+Maj+K : fermer · Échap reste l'arrêt d'urgence · n'arme jamais le laser (Espace, palette fermée)" },
};
function searchSetMode(mode) {
  SEARCH.mode = mode;
  const t = SEARCH_TEXT[mode], box = $('searchBox'), input = $('searchInput');
  box.classList.toggle('cmd', mode === 'cmd');
  box.setAttribute('aria-label', t.label);
  input.placeholder = t.ph;
  input.setAttribute('aria-label', t.input);
  $('searchHint').textContent = t.hint;
  $('cmdErr').hidden = true;
  $('cmdEcho').hidden = mode !== 'cmd' || !CMD.echo;
  $('searchBtn').setAttribute('aria-expanded', String(mode === 'search' && !box.hidden));
  $('cmdBtn').setAttribute('aria-expanded', String(mode === 'cmd' && !box.hidden));
}
async function cmdLoadActions() {
  const j = await apiJson('GET', '/api/command/actions', undefined, { cache: 'no-store', source: 'Commandes', quiet: true }).catch(() => null);
  if (!j) return false;
  CMD.items = searchIndex(j.actions.map(a => ({ kind: a.id ? 'cmd' : 'cmdcue', label: a.label_fr, id: a.id, extra: [a.command], action: a })));
  return true;
}
async function openCommand(text = '') {
  closeHelpMenu();
  $('searchBox').hidden = false;
  searchSetMode('cmd');
  const input = $('searchInput');
  input.value = text;
  input.focus();
  SEARCH.sel = 0; CMD.nav = false; CMD.hi = -1;
  cmdRender();
  const gen = ++SEARCH.gen;
  await cmdLoadActions();
  if (gen !== SEARCH.gen || $('searchBox').hidden || SEARCH.mode !== 'cmd') return;
  cmdRender();
  $('searchBox').dataset.ready = String(gen);
}
function toggleCommand() { if ($('searchBox').hidden || SEARCH.mode !== 'cmd') openCommand(); else closeSearch(); }
$('cmdBtn').querySelector('kbd').textContent = IS_MAC ? '⇧⌘K' : 'Ctrl+Maj+K';
$('cmdBtn').addEventListener('click', toggleCommand);
function cmdRender() {
  const box = $('searchResults');
  box.replaceChildren();
  SEARCH.shown = [];
  const text = $('searchInput').value.trim();
  const msg = t => { const d = document.createElement('div'); d.className = 'none'; d.textContent = t; box.appendChild(d); };
  if (!text) return msg('Une action par son nom (« taille », « teinte », un cue) ou une commande : master.size 80 · cue 2.14 go · page 3 · tempo 128 · tap · noir · tout arrêter. ↑ : dernières commandes.');
  // A line with a value: the action named by its first word.
  let found = searchQuery(text, CMD.items, true);
  if (!found.length && /\s/.test(text)) found = searchQuery(text.split(/\s+/)[0], CMD.items, true);
  if (!found.length) return msg(`Aucune action pour « ${text} » : Entrée l'exécute comme commande`);
  for (const [kind, title] of CMD_KINDS) {
    const group = found.filter(it => it.kind === kind);
    if (!group.length) continue;
    const h = document.createElement('h4'); h.textContent = title; box.appendChild(h);
    for (const it of group) {
      const n = SEARCH.shown.push(it) - 1, a = it.action;
      const row = document.createElement('div');
      row.className = 'sr'; row.setAttribute('role', 'option'); row.dataset.kind = kind; row.dataset.n = n;
      const l = document.createElement('span'); l.className = 'l'; l.textContent = it.label;
      const w = document.createElement('span'); w.className = 'w';
      const c = document.createElement('code'); c.textContent = a.id || a.command; w.appendChild(c);
      if (a.value_fr !== undefined) w.append(` = ${a.value_fr}`);
      else if (a.hint_fr) w.append(` · ${a.hint_fr}`);
      if (a.hint_fr) row.title = `Valeur : ${a.hint_fr}`;
      row.append(l, w);
      row.addEventListener('mousemove', () => { if (SEARCH.sel !== n) { SEARCH.sel = n; searchPaintSel(); } });
      row.addEventListener('click', () => cmdChoose(it));
      box.appendChild(row);
    }
  }
  SEARCH.sel = Math.max(0, Math.min(SEARCH.sel, SEARCH.shown.length - 1));
  searchPaintSel();
}
// An action with a value: its id in the field, waiting for the value; else run it.
function cmdFill(it) {
  const a = it.action, input = $('searchInput');
  input.value = a.command + (a.needs_value ? ' ' : '');
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
  CMD.nav = false; CMD.hi = -1;
  $('cmdErr').hidden = true;
  cmdRender();
}
function cmdChoose(it) {
  if (it.action.needs_value) cmdFill(it); else cmdRun(it.action.command);
}
function cmdShowError(j) {
  const err = $('cmdErr'), input = $('searchInput');
  err.textContent = j.message_fr || 'Commande refusée';
  err.hidden = false;
  err.dataset.refused = String(!!j.refused);
  // The faulty word, selected in the field.
  if (j.token && input.value.substr(j.pos, j.token.length) === j.token) input.setSelectionRange(j.pos, j.pos + j.token.length);
}
// `fallback`: the first result, chosen when the text is a name, not a command.
async function cmdRun(line, fallback) {
  let r, j = null;
  try { r = await api('POST', '/api/command', { line }, { quiet: [400], source: 'Commandes' }); } catch { return cmdShowError({ message_fr: 'Studio injoignable' }); }
  try { j = await r.json(); } catch { /* 423 and others: plain text */ }
  if (r.ok && j) {
    cmdRemember(line);
    CMD.echo = j.echo;
    $('cmdEcho').textContent = '✓ ' + j.echo;
    $('cmdEcho').hidden = SEARCH.mode !== 'cmd';
    $('cmdErr').hidden = true;
    $('searchInput').value = '';
    CMD.hi = -1; CMD.nav = false;
    cmdRender();
    if (await cmdLoadActions() && SEARCH.mode === 'cmd') cmdRender();
    return;
  }
  if (r.status === 400 && j) {
    if (fallback && !j.refused && j.pos === 0 && /^Mot inconnu/.test(j.message_fr) && !/\d/.test(line)) return cmdChoose(fallback);
    return cmdShowError(j);
  }
  cmdShowError({ message_fr: r.status === 423 ? 'Verrouillé en mode spectacle' : `Erreur ${r.status}` });
}
function cmdKey(e) {
  const input = $('searchInput'), n = SEARCH.shown.length;
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    const hist = cmdHistory();
    const browsing = !input.value.trim() || (CMD.hi >= 0 && input.value === hist[CMD.hi]);
    if (browsing && hist.length) {
      const hi = Math.min(hist.length - 1, CMD.hi + (e.key === 'ArrowUp' ? 1 : -1));
      CMD.hi = hi;
      input.value = hi < 0 ? '' : hist[hi];
      input.setSelectionRange(input.value.length, input.value.length);
      $('cmdErr').hidden = true;
      cmdRender();
      return;
    }
    if (n) { SEARCH.sel = (SEARCH.sel + (e.key === 'ArrowDown' ? 1 : n - 1)) % n; CMD.nav = true; searchPaintSel(); }
  } else if (e.key === 'Tab' && !e.shiftKey && SEARCH.shown[SEARCH.sel]) {
    e.preventDefault();
    cmdFill(SEARCH.shown[SEARCH.sel]);
  } else if (e.key === 'Enter') {
    e.preventDefault();
    if (e.repeat) return;
    const it = SEARCH.shown[SEARCH.sel];
    if (CMD.nav && it) return cmdChoose(it);
    const line = input.value.trim();
    if (line) cmdRun(line, it);
  }
}

// For the e2e tests (performance): build an index, query it.
window.__search = { build: items => searchBuild(items), query: text => searchQuery(text), size: () => SEARCH.items.length };

// ---------- états vides (T-437) ----------
// What an empty list is for, and a button for the first action (the same
// as the control it names). Returns the element to put in the list.
function emptyState(text, actions = []) {
  const box = document.createElement('div');
  box.className = 'empty-state';
  const p = document.createElement('p'); p.textContent = text; box.appendChild(p);
  if (actions.length) {
    const row = document.createElement('div'); row.className = 'row';
    for (const [label, run] of actions) {
      const b = document.createElement('button'); b.type = 'button'; b.textContent = label; b.onclick = run;
      row.appendChild(b);
    }
    box.appendChild(row);
  }
  return box;
}

// ---------- accessibilité (T-436) ----------
// Tab lists: only the selected tab is in the tab order; ← / → move to the
// next shown tab and select it. Never Espace or Entrée (arming, tap).
function syncTabStops(list) {
  const tabs = [...list.querySelectorAll('[role=tab]')];
  const sel = tabs.find(t => t.getAttribute('aria-selected') === 'true') || tabs.find(t => t.classList.contains('active')) || tabs[0];
  tabs.forEach(t => { t.tabIndex = t === sel ? 0 : -1; });
}
document.querySelectorAll('[role=tablist]').forEach(list => {
  syncTabStops(list);
  new MutationObserver(() => syncTabStops(list)).observe(list, { subtree: true, childList: true, attributes: true, attributeFilter: ['aria-selected', 'class'] });
  list.addEventListener('keydown', e => {
    if ((e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') || e.metaKey || e.ctrlKey || e.altKey) return;
    const tab = e.target.closest('[role=tab]');
    const tabs = [...list.querySelectorAll('[role=tab]')].filter(t => t.getClientRects().length && !t.disabled);
    const i = tabs.indexOf(tab);
    if (i < 0) return;
    e.preventDefault();
    const next = tabs[(i + (e.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length];
    next.focus();
    next.click();
  });
});
// Dialogs (.modal): the focus goes in when one opens, Tab loops inside,
// and it goes back where it was on close. Échap is NOT « close »: it stays
// the emergency stop (shortcut table); each dialog has its own button.
const FOCUSABLE = 'button, [href], input:not([type=hidden]), select, textarea, [tabindex]:not([tabindex="-1"])';
const focusablesIn = box => [...box.querySelectorAll(FOCUSABLE)].filter(el => !el.disabled && el.getClientRects().length);
const modalOpeners = new Map();
// The last control focused outside a dialog: the opener's own code may
// focus a field inside before the dialog is seen open.
let focusBeforeModal = null;
document.addEventListener('focusin', e => { if (!e.target.closest('.modal')) focusBeforeModal = e.target; });
const openModal = () => [...document.querySelectorAll('.modal')].filter(m => !m.hidden).pop() || null;
function modalToggled(m) {
  if (!m.hidden) {
    if (modalOpeners.has(m)) return;
    modalOpeners.set(m, focusBeforeModal);
    // After the opener's own code, which may focus a field itself.
    setTimeout(() => { if (!m.hidden && !m.contains(document.activeElement)) focusablesIn(m)[0]?.focus(); }, 0);
    return;
  }
  let back = modalOpeners.get(m);
  modalOpeners.delete(m);
  // Opened from a menu that has closed since: back to the menu's button.
  const shut = back && back.isConnected && !back.getClientRects().length ? back.closest('[hidden][id]') : null;
  if (shut) back = document.querySelector(`[aria-controls="${shut.id}"]`) || back;
  const lost = !document.activeElement || document.activeElement === document.body || m.contains(document.activeElement);
  // Never onto the laser button: Space arms from anywhere anyway, but the
  // focus does not go there unasked.
  if (lost && back && back.isConnected && back !== document.body && back.id !== 'armBtn') back.focus();
}
document.querySelectorAll('.modal').forEach(m => {
  if (!m.hasAttribute('role')) m.setAttribute('role', 'dialog');
  m.setAttribute('aria-modal', 'true');
  const h = m.querySelector('h3');
  if (h && !m.hasAttribute('aria-labelledby') && !m.hasAttribute('aria-label')) {
    if (!h.id) h.id = m.id + 'Title';
    m.setAttribute('aria-labelledby', h.id);
  }
  new MutationObserver(() => modalToggled(m)).observe(m, { attributes: true, attributeFilter: ['hidden'] });
});
document.addEventListener('keydown', e => {
  if (e.key !== 'Tab' || e.metaKey || e.ctrlKey || e.altKey) return;
  const m = openModal();
  const f = m ? focusablesIn(m) : [];
  if (!f.length) return;
  const i = f.indexOf(document.activeElement);
  if (e.shiftKey && i <= 0) { e.preventDefault(); f[f.length - 1].focus(); }
  else if (!e.shiftKey && (i < 0 || i === f.length - 1)) { e.preventDefault(); f[0].focus(); }
});
// A control's name: the <label> right before it names it (aria-labelledby,
// so a click on the label changes nothing), unless it has its own.
document.querySelectorAll('label:not([for])').forEach(l => {
  const c = l.nextElementSibling;
  if (!c || !c.id || !/^(INPUT|SELECT|TEXTAREA)$/.test(c.tagName) || l.querySelector('input, select, textarea')) return;
  if (c.hasAttribute('aria-label') || c.hasAttribute('aria-labelledby')) return;
  if (!l.id) l.id = 'lbl-' + c.id;
  c.setAttribute('aria-labelledby', l.id);
});
addHelpButtons();

loadState().then(loadTestPatterns).then(loadProjection).then(loadPresets).then(loadPalettes).then(loadLfos).then(loadFxGrid).then(loadRoutes).then(loadAudioPresets).then(loadTriggers).then(previewLoop);
setTimeout(dataWarnLoop, 5000);
midiLoop();
loadPresence().catch(() => {});
// T-437: the tour on a fresh data folder, once the page has its state.
if (!VUE) setTimeout(maybeStartTour, 600); // the main window only (T-281)
// T-438: the help, fetched once so it still opens if the network goes.
setTimeout(() => helpLoad().catch(() => {}), 1500);

export { CMD, cmdKey, cmdRender, emptyState, openCommand, searchSetMode, toggleCommand };
