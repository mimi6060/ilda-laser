// Laser Studio — help.js (T-435).
// Sections (as in the former index.html script):
//   aide intégrée (T-438)
//   visite guidée (T-437)
//   recherche globale (T-429)
//   the palette
//   going there
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './program.js';
import { $, api, apiJson } from './core.js';
import { IS_MAC, closeKeysHelp, openKeysHelp } from './laser.js';
import { LOCKED_FR, activeWs, flash, setLivePanel, setWorkspace, showMode } from './workspaces.js';
import { scenes } from './cuelist.js';
import { cueDown, cueUp, grid, gridPage, pageKey, presetById, presets, selectPage, setCueMore, slots } from './cues.js';
import { setRender3dOpen, setView, show3d } from './view3d.js';
import { fmtTime, timeline } from './shows.js';
import { controlLabels } from './midi.js';
import { setCreationTab } from './figures.js';
import { loadIldaLib } from './media.js';
import { CMD, cmdKey, cmdRender, openCommand, searchSetMode } from './command.js';

// ---------- aide intégrée (T-438) ----------
// Pages compiled into the studio (GET /api/help?full=1), fetched once in
// the background so the help keeps working even if the network goes. The
// drawer is not modal: it never takes a global key (Espace arms, Échap is
// the e-stop, never « fermer »). Each panel's « ? » comes from its
// data-help attribute; links between pages are #aide/<page>.
const HELP = { pages: null, loading: null, slug: null };
const helpFold = t => String(t).normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
function helpLoad() {
  if (HELP.pages) return Promise.resolve(HELP.pages);
  HELP.loading ??= apiJson('GET', '/api/help?full=1', undefined, { source: 'Aide' }).then(list => {
    const div = document.createElement('div');
    HELP.pages = list.map(p => { div.innerHTML = p.body_html; return { ...p, text: helpFold(p.title_fr + ' ' + div.textContent) }; });
    return HELP.pages;
  }).finally(() => { HELP.loading = null; });
  return HELP.loading;
}
function helpShowToc() {
  HELP.slug = null;
  $('helpArticle').hidden = true;
  $('helpToc').hidden = false;
  $('helpTocBtn').hidden = true;
  const box = $('helpToc');
  box.replaceChildren();
  if (!HELP.pages) { box.innerHTML = '<div class="hint">Chargement de l\'aide…</div>'; return; }
  const q = helpFold($('helpSearch').value.trim());
  const words = q.split(/\s+/).filter(Boolean);
  const found = HELP.pages.filter(p => words.every(w => p.text.includes(w)));
  if (!found.length) { box.innerHTML = '<div class="hint">Aucune page ne correspond.</div>'; return; }
  const groups = [...new Set(found.map(p => p.group_fr))];
  for (const g of groups) {
    const h = document.createElement('h4'); h.textContent = g; box.appendChild(h);
    for (const p of found.filter(x => x.group_fr === g)) {
      const b = document.createElement('button');
      b.textContent = p.title_fr; b.dataset.helpSlug = p.slug;
      b.onclick = () => openHelp(p.slug);
      box.appendChild(b);
    }
  }
}
// Opens the drawer on `slug` (or on the table of contents).
async function openHelp(slug = null) {
  closeHelpMenu();
  $('helpDrawer').hidden = false;
  if (!HELP.pages) {
    helpShowToc();
    try { await helpLoad(); } catch { $('helpToc').innerHTML = '<div class="hint">Aide indisponible : le studio ne répond pas.</div>'; return; }
  }
  const page = slug && HELP.pages.find(p => p.slug === slug);
  if (!page) { helpShowToc(); return; }
  HELP.slug = page.slug;
  $('helpToc').hidden = true;
  $('helpTocBtn').hidden = false;
  $('helpArticle').hidden = false;
  $('helpTitle').textContent = page.title_fr;
  $('helpBody').innerHTML = page.body_html; // our own pages, compiled into the studio
  $('helpArticle').scrollTop = 0;
}
function closeHelp() { $('helpDrawer').hidden = true; HELP.slug = null; }
$('helpClose').onclick = closeHelp;
$('helpTocBtn').onclick = () => helpShowToc();
$('helpSearch').addEventListener('input', () => { if (HELP.pages) helpShowToc(); else openHelp(); });
$('helpBody').addEventListener('click', e => {
  const a = e.target.closest('a[href^="#aide/"]');
  if (!a) return;
  e.preventDefault();
  openHelp(a.getAttribute('href').slice('#aide/'.length));
});
window.addEventListener('hashchange', () => {
  if (location.hash.startsWith('#aide/')) openHelp(decodeURIComponent(location.hash.slice('#aide/'.length)));
});
// « ? » on every panel: next to its title (h2, summary or [data-help-title]).
function addHelpButtons() {
  document.querySelectorAll('[data-help]').forEach(panel => {
    const title = panel.querySelector('[data-help-title]') || panel.querySelector('h2, summary');
    if (!title || title.querySelector(':scope > .help-q, :scope > .help-title > .help-q')) return;
    const det = title.tagName === 'SUMMARY' ? title.parentElement : null;
    if (det && det.previousElementSibling?.classList.contains('det-q')) return;
    const q = document.createElement('button');
    q.type = 'button'; q.className = 'help-q'; q.textContent = '?';
    q.title = 'Aide sur ce panneau'; q.setAttribute('aria-label', 'Aide sur ce panneau');
    q.dataset.helpFor = panel.dataset.help;
    q.addEventListener('click', e => {
      e.preventDefault(); e.stopPropagation(); // a « ? » in a summary must not fold the panel
      helpFrom(q);
    });
    const first = title.firstChild;
    if (title.tagName === 'H2' && first && first.nodeType === Node.TEXT_NODE && title.children.length) {
      // Keep the title and its « ? » together on the left of the heading.
      const wrap = document.createElement('span'); wrap.className = 'help-title';
      title.insertBefore(wrap, first);
      wrap.append(first, q);
    } else if (det) {
      // Before the <details>, floated beside its summary (T-436: a button
      // inside a summary is a control nested in a control).
      q.classList.add('det-q');
      det.before(q);
    } else title.appendChild(q);
  });
}
// From a « ? » inside a modal dialog (Importer…): the drawer goes into
// the dialog's top layer while it is open, else it would sit behind it.
function helpFrom(btn) {
  const dlg = btn.closest('dialog[open]');
  const drawer = $('helpDrawer');
  if (dlg && drawer.parentElement !== dlg) {
    dlg.appendChild(drawer);
    dlg.addEventListener('close', () => document.body.appendChild(drawer), { once: true });
  }
  openHelp(btn.dataset.helpFor);
}
// « ? Aide » menu.
function closeHelpMenu() { $('helpMenu').hidden = true; $('helpBtn').setAttribute('aria-expanded', 'false'); }
$('helpBtn').onclick = e => {
  e.stopPropagation();
  if (!$('helpMenu').hidden) { closeHelpMenu(); return; }
  const r = $('helpBtn').getBoundingClientRect();
  const menu = $('helpMenu');
  menu.hidden = false;
  menu.style.top = (r.bottom + 4) + 'px';
  menu.style.left = Math.max(8, Math.min(r.left, innerWidth - menu.offsetWidth - 8)) + 'px';
  $('helpBtn').setAttribute('aria-expanded', 'true');
};
document.addEventListener('click', e => { if (!$('helpMenu').hidden && !e.target.closest('#helpMenu')) closeHelpMenu(); });
$('helpMenuToc').onclick = () => { $('helpSearch').value = ''; openHelp(); };
$('helpMenuFirst').onclick = () => openHelp('premier-show');
$('helpMenuKeys').onclick = () => { closeHelpMenu(); openKeysHelp(); };
$('helpMenuTour').onclick = () => { closeHelpMenu(); startTour(); };
$('keysOpenHelp').onclick = () => { closeKeysHelp(); openHelp(); };

// ---------- visite guidée (T-437) ----------
// Shown once on a fresh data folder (onboarding.json on the studio, not
// localStorage: another browser doesn't replay it), replayed from
// « ? Aide ». It only highlights: never clicks a control, never arms,
// never connects an output. It catches no key and no click outside its
// bubble: Espace and Échap keep their role, cues stay clickable. A step
// on another workspace switches to it (a view, like clicking its tab).
const TOUR_STEPS = [
  { target: '#armBtn, #estopBtn, #blackoutBtn', title: 'Allumer et arrêter',
    text: ['Le laser démarre toujours éteint. « LASER OFF » (ou <kbd>Espace</kbd>) l\'arme ; « Noir » coupe tout de suite.',
      '« ARRÊT » (<kbd>Échap</kbd>, dans tous les onglets) est un arrêt d\'urgence verrouillé : il faut le réinitialiser avant de réarmer. L\'arrêt d\'urgence matériel du projecteur reste votre vraie sécurité.'] },
  { target: '#cueRegion', ws: 'live', title: 'La grille de cues',
    text: ['Cliquez une case, ou tapez la lettre du clavier affichée dans son coin, pour jouer un cue. Laser éteint, seul l\'aperçu change : essayez sans crainte.',
      'Les pages changent avec leurs onglets ou <kbd>F5</kbd> à <kbd>F12</kbd>.'] },
  { target: 'header .tempo', title: 'Le tempo',
    text: ['Le tempo cale modulateurs, chenillards et shows. Tapez le BPM, ou « Tap » (<kbd>Entrée</kbd>) sur la musique.',
      '« Sync 1 » (<kbd>⌫</kbd>) recale le premier temps ; « Auto » suit la musique détectée.'] },
  { target: '#wsTabs [data-ws-tab]', title: 'Les espaces de travail',
    text: ['LIVE pour jouer, TIMELINE pour les shows, CRÉATION pour vos looks et figures, RÉGLAGES pour la sortie et la sécurité (<kbd>F1</kbd> à <kbd>F4</kbd>).',
      'Changer d\'onglet ne change que l\'affichage : ce qui joue continue.'] },
  { target: '#outputsPanel', ws: 'settings', title: 'Sortie vers MadMapper',
    text: ['Pour votre ShowNET : dans MadMapper, laissez la sortie laser coupée ; ici, « Connecter » sur la ligne PONK. Une source Laser Studio apparaît dans MadMapper : donnez-la à la surface laser du ShowNET.',
      'Connecter une sortie n\'arme jamais le laser. Le pas à pas complet est dans l\'aide : « Premier show avec MadMapper et le ShowNET ».'] },
  { target: '#checklistPanel', ws: 'settings', title: 'Les verrous d\'armement',
    text: ['Avec une vraie sortie, le laser refuse de s\'armer tant que la liste « Avant le show » n\'est pas terminée (avec PONK : arrêt matériel testé compris) et que le public n\'est pas déclaré dans « Sécurité » (une zone Public ou l\'horizon complet).',
      'Les raisons du refus s\'affichent sous le bouton du laser. Ces verrous sont un aide-mémoire : ils ne remplacent ni l\'arrêt matériel ni l\'évaluation des risques.'] },
  { target: '#tlExamples, #tlAutoRow', ws: 'timeline', title: 'Des shows tout de suite',
    text: ['« Ouvrir un exemple » charge un show prêt à jouer, calé sur le tempo. Avec un morceau importé, « Générer un show depuis le morceau » en propose un à retoucher.',
      'Rien n\'est enregistré tant que vous ne le décidez pas, et rien ne touche au laser.'] },
  { target: '#helpBtn', title: 'L\'aide, même sans réseau',
    text: ['« ? Aide » ouvre le sommaire, les raccourcis et cette visite. Chaque panneau a son « ? ».',
      'Bon show ! Armez seulement quand le projecteur, la salle et l\'arrêt matériel sont prêts.'] },
];
const TOUR = { on: false, i: 0, ws: null, timer: 0 };
function tourTargets(step) {
  return [...document.querySelectorAll(step.target)].filter(el => el.getClientRects().length && el.getBoundingClientRect().width > 0);
}
function tourPlace() {
  if (!TOUR.on) return;
  const step = TOUR_STEPS[TOUR.i];
  const els = tourTargets(step);
  const ring = $('tourRing'), bub = $('tourBubble');
  const bw = bub.offsetWidth, bh = bub.offsetHeight, m = 12;
  if (!els.length) {
    ring.hidden = true;
    bub.style.left = Math.max(m, (innerWidth - bw) / 2) + 'px';
    bub.style.top = Math.max(m, (innerHeight - bh) / 2) + 'px';
    return;
  }
  const rs = els.map(el => el.getBoundingClientRect());
  const r = { left: Math.min(...rs.map(x => x.left)), top: Math.min(...rs.map(x => x.top)), right: Math.max(...rs.map(x => x.right)), bottom: Math.max(...rs.map(x => x.bottom)) };
  const pad = 6;
  ring.hidden = false;
  Object.assign(ring.style, { left: (r.left - pad) + 'px', top: (r.top - pad) + 'px', width: (r.right - r.left + 2 * pad) + 'px', height: (r.bottom - r.top + 2 * pad) + 'px' });
  let top = r.bottom + pad + m;
  if (top + bh > innerHeight - m) top = r.top - pad - m - bh;              // above
  if (top < m) top = Math.min(innerHeight - bh - m, Math.max(m, r.top));    // beside
  let left = Math.min(Math.max(m, r.left), innerWidth - bw - m);
  if (top + bh > r.top - pad && top < r.bottom + pad) {                     // beside: left or right of the target
    left = r.left - pad - m - bw >= m ? r.left - pad - m - bw : Math.min(r.right + pad + m, innerWidth - bw - m);
  }
  bub.style.left = Math.max(m, left) + 'px';
  bub.style.top = Math.max(m, top) + 'px';
}
function tourShow(i) {
  TOUR.i = Math.max(0, Math.min(TOUR_STEPS.length - 1, i));
  const step = TOUR_STEPS[TOUR.i];
  if (step.ws && activeWs !== step.ws) setWorkspace(step.ws);
  $('tourTitle').textContent = step.title;
  $('tourText').innerHTML = step.text.map(t => `<p>${t}</p>`).join(''); // our own text
  $('tourCount').textContent = `${TOUR.i + 1} / ${TOUR_STEPS.length}`;
  $('tourPrev').disabled = TOUR.i === 0;
  const last = TOUR.i === TOUR_STEPS.length - 1;
  $('tourNext').textContent = last ? 'Terminer' : 'Suivant';
  $('tourSkip').hidden = last;
  $('tourBubble').hidden = false;
  tourTargets(step)[0]?.scrollIntoView({ block: 'nearest' });
  requestAnimationFrame(tourPlace);
}
function startTour() {
  if (TOUR.on) { tourShow(0); return; }
  TOUR.on = true;
  TOUR.ws = activeWs;
  tourShow(0);
  TOUR.timer = setInterval(tourPlace, 400); // panels load and move under the ring
}
// Finished or skipped: recorded on the studio, so a reload or another
// browser doesn't show it again.
async function endTour() {
  if (!TOUR.on) return;
  TOUR.on = false;
  clearInterval(TOUR.timer);
  $('tourBubble').hidden = true;
  $('tourRing').hidden = true;
  if (TOUR.ws && activeWs !== TOUR.ws) setWorkspace(TOUR.ws);
  try { await api('POST', '/api/onboarding', { tour_done: true }, { source: 'Aide' }); } catch { /* said by api() */ }
}
$('tourPrev').onclick = () => tourShow(TOUR.i - 1);
$('tourNext').onclick = () => { if (TOUR.i >= TOUR_STEPS.length - 1) endTour(); else tourShow(TOUR.i + 1); };
$('tourSkip').onclick = endTour;
window.addEventListener('resize', tourPlace);
document.addEventListener('scroll', tourPlace, true);
async function maybeStartTour() {
  try {
    const o = await apiJson('GET', '/api/onboarding', undefined, { source: 'Aide' });
    if (!o.tour_done) startTour();
  } catch { /* the studio said why */ }
}
// For the e2e tests: what the tour and the help hold.
window.helpDebug = { steps: () => TOUR_STEPS.map(s => ({ target: s.target, ws: s.ws || null, title: s.title })), tourOn: () => TOUR.on, page: () => HELP.slug, loaded: () => !!HELP.pages };

// ---------- recherche globale (T-429) ----------
// One field finds a place: cues (and user cues), pages, figures, ILDA
// files, scenes, cue lists, shows and their markers, controls (French
// label or id), RÉGLAGES sections ([data-search] and their sub-titles and
// labels) and the help pages. The index lives here, rebuilt each time the
// field opens (lists fetched again, so a figure made since is found), not
// at each key. Choosing a result only shows it: workspace, tab, page,
// highlight. « Jouer » (or Maj+Entrée) is the one explicit action, and it
// is the grid's own press (POST /api/cue down then up): it never arms.
const SEARCH_KINDS = [['cue', 'Cues'], ['page', 'Pages'], ['figure', 'Figures'], ['ilda', 'Médiathèque ILDA'], ['scene', 'Scènes'],
  ['cuelist', 'Listes de cues'], ['show', 'Shows'], ['marker', 'Marqueurs de timeline'], ['control', 'Contrôles'], ['setting', 'Réglages'], ['help', 'Aide']];
const SEARCH_ORDER = Object.fromEntries(SEARCH_KINDS.map(([k], i) => [k, i]));
const SEARCH_MAX = 30;
const SEARCH = { items: [], shown: [], sel: 0, gen: 0, markTimer: 0 };
const WS_FR = { live: 'LIVE', timeline: 'TIMELINE', creation: 'CRÉATION', settings: 'RÉGLAGES' };
// The same folding as the help's own search: lower case, no accents.
const searchFold = helpFold;
// The first text of an element (a summary without its LEDs, a tab without its counter).
const ownText = el => {
  if (!el) return '';
  for (const n of el.childNodes) if (n.nodeType === 3 && n.textContent.trim()) return n.textContent.trim();
  return el.textContent.trim();
};
// Where an element lives, in words: « RÉGLAGES › Sécurité », « LIVE › Direct ».
function searchWhere(el) {
  const ws = el.closest('[data-ws]')?.dataset.ws;
  const parts = [ws ? WS_FR[ws] : el.closest('#stage') ? 'Aperçu' : 'barre du haut'];
  const panel = el.closest('#livePanel [data-panel]');
  if (panel) parts.push(ownText(document.querySelector(`#liveTabs [data-panel-tab="${panel.dataset.panel}"]`)));
  const cp = el.closest('#creationPanel [data-cpanel]');
  if (cp) parts.push(ownText(document.querySelector(`#creationTabs [data-ctab="${cp.dataset.cpanel}"]`)));
  const sec = el.closest('[data-search]');
  if (sec && sec !== el) parts.push(searchSectionName(sec));
  else if (!sec) {
    const det = el.closest('details');
    if (det && det !== el) parts.push(ownText(det.querySelector(':scope > summary')));
  }
  return parts.filter(Boolean).join(' › ');
}
function searchSectionName(el) {
  return ownText(el.querySelector(':scope > summary, :scope > h2')) || el.dataset.search;
}
// SearchItem: { kind, label, sub, id?, extra?: [], deep?: text, target: {…}, missing?, play? }
function searchIndex(items) {
  return items.map(it => {
    const l = searchFold(it.label || '');
    return { ...it, l, lw: ' ' + l.replace(/[^a-z0-9]+/g, ' '), idf: it.id ? searchFold(it.id) : '',
      key: searchFold([it.label, it.sub, it.id, ...(it.extra || [])].filter(Boolean).join(' ')),
      deep: it.deep ? searchFold(it.deep) : '' };
  });
}
function searchBuild(items) {
  SEARCH.items = searchIndex(items);
  return SEARCH.items.length;
}
// The letters of `q` in order in the name or the id (« tlmtr » finds « Taille maître »).
function searchSubseq(it, q) {
  return [it.l, it.idf].some(t => { let i = 0; for (const c of t) if (c === q[i] && ++i === q.length) return true; return false; });
}
// 0: its exact id (master.size); 1: the name starts with the text; 2: each
// word starts a word of the name; 3: in the name; 4: in where it is (or
// its id); 5: a help page's text.
function searchRank(it, q, words) {
  if (words.every(w => it.key.includes(w))) {
    if (it.idf === q) return 0;
    if (it.l.startsWith(q)) return 1;
    if (words.every(w => it.lw.includes(' ' + w))) return 2;
    if (words.every(w => it.l.includes(w))) return 3;
    return 4;
  }
  return it.deep && words.every(w => it.deep.includes(w)) ? 5 : -1;
}
// `fuzzy` (the command palette): also the names holding the letters in order, last.
function searchQuery(text, items = SEARCH.items, fuzzy = false) {
  const q = searchFold(text).trim().replace(/\s+/g, ' ');
  if (!q) return [];
  const words = q.split(' ');
  const hits = [];
  const sq = q.replace(/ /g, '');
  for (const it of items) {
    let r = searchRank(it, q, words);
    if (r < 0 && fuzzy && searchSubseq(it, sq)) r = 6;
    if (r >= 0) hits.push([r, it]);
  }
  hits.sort((a, b) => a[0] - b[0] || SEARCH_ORDER[a[1].kind] - SEARCH_ORDER[b[1].kind] || a[1].l.length - b[1].l.length);
  return hits.slice(0, SEARCH_MAX).map(h => h[1]);
}
// The page holding a cue: the one on screen if it is there, else the first.
function searchCuePage(id) {
  if (grid.pages[gridPage]?.cells.includes(id)) return gridPage;
  return grid.pages.findIndex(pg => pg.cells.includes(id));
}
async function searchCollect() {
  const get = path => apiJson('GET', path, undefined, { cache: 'no-store', source: 'Recherche' }).catch(() => null);
  const [figs, shows, lists, ilda] = await Promise.all([get('/api/figures'), get('/api/shows'), get('/api/cuelists'), get('/api/media/ilda'),
    helpLoad().catch(() => null)]);
  const items = [];
  for (const p of presets) {
    const i = searchCuePage(p.id);
    items.push({ kind: 'cue', label: p.name, play: p.id,
      sub: [i >= 0 ? `page ${grid.pages[i].name}` : `${p.category} · hors grille`, p.user ? 'cue utilisateur' : ''].filter(Boolean).join(' · '),
      // The cell's notes and tags (T-430).
      id: p.id, extra: [p.category, slots[p.id]?.notes, ...(slots[p.id]?.tags || [])], target: { cue: p.id } });
  }
  grid.pages.forEach((pg, i) => items.push({ kind: 'page', label: pg.name,
    sub: `LIVE › page ${i + 1}` + (pageKey(i) ? ` · ${pageKey(i)}` : ''), extra: [`page.${i + 1}`], target: { page: i } }));
  for (const f of figs || []) items.push({ kind: 'figure', label: f.name, play: f.id,
    sub: `figure · ${f.frames} image${f.frames > 1 ? 's' : ''}`, target: { cue: f.id, fig: f.name } });
  for (const f of (ilda && ilda.files) || []) items.push({ kind: 'ilda', label: f.file, play: f.id,
    sub: 'CRÉATION › Médiathèque', target: { file: f.file } });
  for (const sc of scenes) items.push({ kind: 'scene', label: sc.name, sub: 'LIVE › Scènes', target: { scene: sc.name } });
  for (const l of (lists && lists.lists) || []) items.push({ kind: 'cuelist', label: l.name,
    sub: `LIVE › Liste de cues · ${l.steps.length} pas`, target: { cuelist: l.name } });
  for (const s of shows || []) {
    items.push({ kind: 'show', label: s.name, sub: `TIMELINE · ${s.tracks} piste${s.tracks > 1 ? 's' : ''}`, target: { show: s.name } });
    for (const m of s.markers || []) items.push({ kind: 'marker', label: m.name || 'Marqueur',
      sub: `show ${s.name} · ${s.time_base === 'beats' ? 'temps ' + (+m.at).toFixed(1) : fmtTime(m.at)}`, target: { show: s.name } });
  }
  for (const [id, label] of Object.entries(controlLabels)) {
    if (/^(grid|page)\./.test(id)) continue; // the cells and pages are found by their names
    const el = document.querySelector(`[data-control="${CSS.escape(id)}"]`);
    items.push({ kind: 'control', label: label || id, sub: el ? searchWhere(el) : id, id, missing: !el, target: { control: id } });
  }
  const seen = new Set();
  document.querySelectorAll('[data-search]').forEach(sec => {
    const name = searchSectionName(sec);
    items.push({ kind: 'setting', label: name, sub: searchWhere(sec), extra: [sec.dataset.search], target: { el: sec } });
    // A list's choices (the fonts of « Police du texte ») lead to it.
    if (sec.tagName === 'SELECT') for (const o of sec.options) items.push({ kind: 'setting', label: o.textContent.trim(), sub: `${searchWhere(sec)} › ${name}`, target: { el: sec } });
    sec.querySelectorAll('.sf-sub, label, h2, summary').forEach(el => {
      const text = ownText(el).replace(/\s+/g, ' ').slice(0, 80);
      if (!text || text === name || el.closest('[data-search]') !== sec) return;
      const k = name + '|' + text;
      if (seen.has(k)) return;
      seen.add(k);
      items.push({ kind: 'setting', label: text, sub: searchWhere(el), target: { el } });
    });
  });
  for (const p of HELP.pages || []) items.push({ kind: 'help', label: p.title_fr, sub: `Aide › ${p.group_fr}`, deep: p.text, target: { help: p.slug } });
  return items;
}

// ----- the palette -----
function searchRender() {
  const box = $('searchResults');
  box.replaceChildren();
  const text = $('searchInput').value;
  const found = searchQuery(text);
  SEARCH.shown = [];
  const msg = t => { const d = document.createElement('div'); d.className = 'none'; d.textContent = t; box.appendChild(d); };
  if (!text.trim()) return msg('Un nom ou une partie : cue, page, scène, show, contrôle (« Taille maître » ou master.size), réglage, page d\'aide.');
  if (!found.length) return msg(`Rien trouvé pour « ${text.trim()} »`);
  for (const [kind, title] of SEARCH_KINDS) {
    const group = found.filter(it => it.kind === kind);
    if (!group.length) continue;
    const h = document.createElement('h4'); h.textContent = title; box.appendChild(h);
    for (const it of group) {
      const n = SEARCH.shown.push(it) - 1;
      const row = document.createElement('div');
      row.className = 'sr'; row.setAttribute('role', 'option'); row.dataset.kind = kind; row.dataset.n = n;
      row.classList.toggle('missing', !!it.missing);
      const l = document.createElement('span'); l.className = 'l'; l.textContent = it.label;
      const w = document.createElement('span'); w.className = 'w';
      if (it.missing) {
        // Not on screen: greyed, with the id to copy (MIDI, OSC, macros).
        const c = document.createElement('code'); c.textContent = it.target.control; w.append('pas à l\'écran · ', c);
        row.title = 'Pas d\'élément à l\'écran : cliquer copie l\'identifiant';
      } else w.textContent = it.sub;
      row.append(l, w);
      if (it.play && presetById.has(it.play)) {
        const b = document.createElement('button');
        b.type = 'button'; b.className = 'play'; b.textContent = '▶ Jouer';
        b.title = 'Jouer ce cue, comme un clic sur sa case (Maj+Entrée). N\'arme pas le laser.';
        b.addEventListener('click', e => { e.stopPropagation(); searchPlay(it); });
        row.appendChild(b);
      }
      row.addEventListener('mousemove', () => { if (SEARCH.sel !== n) { SEARCH.sel = n; searchPaintSel(); } });
      row.addEventListener('click', () => searchChoose(it));
      box.appendChild(row);
    }
  }
  SEARCH.sel = Math.max(0, Math.min(SEARCH.sel, SEARCH.shown.length - 1));
  searchPaintSel();
}
function searchPaintSel() {
  document.querySelectorAll('#searchResults .sr').forEach(r => {
    const on = +r.dataset.n === SEARCH.sel;
    r.classList.toggle('sel', on);
    r.setAttribute('aria-selected', String(on));
    if (on) r.scrollIntoView({ block: 'nearest' });
  });
}
async function openSearch() {
  closeHelpMenu();
  $('searchBox').hidden = false;
  searchSetMode('search');
  $('searchBtn').setAttribute('aria-expanded', 'true');
  $('searchInput').focus();
  $('searchInput').select();
  SEARCH.sel = 0;
  searchRender(); // the last index at once, the fresh one in a moment
  const gen = ++SEARCH.gen;
  const items = await searchCollect();
  if (gen !== SEARCH.gen || $('searchBox').hidden) return;
  searchBuild(items);
  searchRender();
  $('searchBox').dataset.ready = String(gen);
}
function closeSearch() {
  if ($('searchBox').hidden) return;
  $('searchBox').hidden = true;
  $('searchBtn').setAttribute('aria-expanded', 'false');
  $('cmdBtn').setAttribute('aria-expanded', 'false');
  if (document.activeElement === $('searchInput')) $('searchInput').blur();
}
function toggleSearch() { if ($('searchBox').hidden || SEARCH.mode !== 'search') openSearch(); else closeSearch(); }
$('searchBtn').querySelector('kbd').textContent = IS_MAC ? '⌘K' : 'Ctrl+K';
$('searchBtn').addEventListener('click', toggleSearch);
$('searchInput').addEventListener('input', () => {
  SEARCH.sel = 0;
  // « > » first: the command line, as in a code editor.
  if (SEARCH.mode === 'search' && $('searchInput').value.startsWith('>')) { openCommand($('searchInput').value.slice(1).trimStart()); return; }
  if (SEARCH.mode === 'cmd') { CMD.nav = false; CMD.hi = -1; $('cmdErr').hidden = true; cmdRender(); } else searchRender();
});
// ↑ ↓ Entrée here; Échap is left to the page (the e-stop, which also closes this).
$('searchInput').addEventListener('keydown', e => {
  if (SEARCH.mode === 'cmd') { cmdKey(e); return; }
  const n = SEARCH.shown.length;
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    if (n) { SEARCH.sel = (SEARCH.sel + (e.key === 'ArrowDown' ? 1 : n - 1)) % n; searchPaintSel(); }
  } else if (e.key === 'Enter') {
    e.preventDefault();
    const it = SEARCH.shown[SEARCH.sel];
    if (!it || e.repeat) return;
    if (e.shiftKey) { if (it.play) searchPlay(it); } else searchChoose(it);
  }
});
document.addEventListener('pointerdown', e => {
  if (!$('searchBox').hidden && !e.target.closest('#searchBox, #searchBtn, #cmdBtn')) closeSearch();
}, true);

// ----- going there -----
// Outlines what get() finds for 2 s, again if a redraw replaces it.
function searchMark(get) {
  clearInterval(SEARCH.markTimer);
  document.querySelectorAll('.found').forEach(el => el.classList.remove('found'));
  const until = performance.now() + 2000;
  let first = true;
  const tick = () => {
    if (performance.now() > until) {
      clearInterval(SEARCH.markTimer);
      document.querySelectorAll('.found').forEach(x => x.classList.remove('found'));
      return;
    }
    const el = get();
    if (el && !el.classList.contains('found')) {
      el.classList.add('found');
      if (first) { first = false; el.scrollIntoView({ block: 'nearest' }); }
    }
  };
  tick();
  SEARCH.markTimer = setInterval(tick, 150);
}
// The workspace, tab and folded sections around `el`; false if show mode keeps it hidden.
function searchReveal(el) {
  const ws = el.closest('[data-ws]')?.dataset.ws;
  if (ws && showMode && (ws === 'creation' || ws === 'settings')) { flash(LOCKED_FR); return false; }
  if (ws && ws !== activeWs) setWorkspace(ws);
  const panel = el.closest('#livePanel [data-panel]');
  if (panel) setLivePanel(panel.dataset.panel);
  const cp = el.closest('#creationPanel [data-cpanel]');
  if (cp) setCreationTab(cp.dataset.cpanel);
  for (let d = el.closest('details'); d; d = d.parentElement && d.parentElement.closest('details')) d.open = true;
  // Behind « Plus… » of the cue bar (T-506), or in the 3D settings drawer (T-503).
  if (el.closest('#cueMore')) setCueMore(true);
  if (el.closest('#render3d')) { if (!show3d) setView('3d'); setRender3dOpen(true); }
  // A tempo control behind « ⋯ » (T-504): open it after this click has
  // bubbled (a click elsewhere closes the popover).
  const more = document.getElementById('tbMoreBtn');
  if (el.closest('#tbMore') && more.offsetParent && more.getAttribute('aria-expanded') !== 'true') setTimeout(() => more.click(), 0);
  return true;
}
function searchGoCue(id) {
  const i = searchCuePage(id);
  if (i < 0) return false;
  setWorkspace('live');
  if (i !== gridPage) selectPage(i);
  searchMark(() => document.querySelector(`#cues .cue[data-id="${CSS.escape(id)}"]`));
  return true;
}
function searchChoose(it) {
  const t = it.target;
  closeSearch();
  if (it.missing) {
    navigator.clipboard?.writeText(t.control).catch(() => {});
    flash(`« ${it.label} » n'est pas à l'écran · identifiant copié : ${t.control}`);
  } else if (t.fig !== undefined) {
    // A figure: its cell if it has one, else the library in CRÉATION.
    if (searchGoCue(t.cue)) return;
    if (showMode) { flash(LOCKED_FR); return; }
    setWorkspace('creation'); setCreationTab('figures');
    searchMark(() => document.querySelector(`#figLib [data-fig="${CSS.escape(t.fig)}"]`));
  } else if (t.cue !== undefined) {
    if (!searchGoCue(t.cue)) flash(`« ${it.label} » n'est sur aucune page de la grille (agrandir la grille ?)`);
  } else if (t.page !== undefined) {
    if (!grid.pages[t.page]) return;
    setWorkspace('live');
    selectPage(t.page);
    searchMark(() => $('cueTabs').children[t.page]);
  } else if (t.file !== undefined) {
    if (!searchReveal($('ildaLib'))) return;
    loadIldaLib();
    searchMark(() => document.querySelector(`#ildaLib .ildaitem[data-file="${CSS.escape(t.file)}"]`));
  } else if (t.scene !== undefined) {
    setWorkspace('live'); setLivePanel('scenes');
    searchMark(() => { const i = scenes.findIndex(s => s.name === t.scene); return i < 0 ? null : document.querySelector(`#sceneList .scene[data-index="${i}"]`); });
  } else if (t.cuelist !== undefined) {
    // Shown, not loaded: loading a list is the operator's choice.
    setWorkspace('live'); setLivePanel('cuelist');
    searchMark(() => $('cqSelect'));
    flash(`Liste « ${t.cuelist} » : la choisir ici pour la charger`);
  } else if (t.show !== undefined) {
    setWorkspace('timeline');
    searchMark(() => $('tlShow'));
    if (!timeline || timeline.name !== t.show) flash(`Show « ${t.show} » : le choisir dans la liste pour le charger`);
  } else if (t.control !== undefined) {
    const get = () => document.querySelector(`[data-control="${CSS.escape(t.control)}"]`);
    const el = get();
    if (el && searchReveal(el)) searchMark(get);
  } else if (t.el) {
    if (searchReveal(t.el)) searchMark(() => t.el);
  } else if (t.help !== undefined) {
    openHelp(t.help);
  }
}
// The one action: the grid's own press and release. Never arms.
async function searchPlay(it) {
  if (!it.play || !presetById.has(it.play)) return;
  await cueDown(it.play, null);
  await cueUp(it.play);
}

export { SEARCH, SEARCH_KINDS, SEARCH_ORDER, addHelpButtons, closeHelpMenu, closeSearch, helpLoad,
  maybeStartTour, openHelp, searchBuild, searchIndex, searchPaintSel, searchQuery, toggleSearch };
