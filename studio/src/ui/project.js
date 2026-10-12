// Laser Studio — project.js (T-435).
// Sections (as in the former index.html script):
//   project (T-286)
//   site profiles and packages (T-289)
//   autosave and recovery (T-287)
//   named versions (T-288)
//   copy to a second location (T-450)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './apc40.js';
import { $, VUE, api, apiJson, projectSaved, takeProjectModified, unsavedWork } from './core.js';
import { LOCKED_FR, flash, showMode } from './workspaces.js';
import { loadProjection } from './zones.js';
import { loadPalettes } from './direct.js';
import { loadPresets } from './cues.js';
import { lastTimeline, loadShows, loadSongs } from './shows.js';
import { loadAudioPresets, loadFxGrid, loadLfos, loadRoutes, loadTriggers } from './modulators.js';
import { loadState } from './venue.js';
import { esc } from './midi.js';
import { figureDirty, loadFigLib, saveFigure } from './figures.js';
import { loadIldaLib } from './media.js';
import { pg, pgDirty, pgSaveNow } from './program.js';

// ---------- project (T-286) ----------
// One .lsproj file per show, in the studio's projects folder only (no
// access to the rest of the disk from the browser). Opening a project
// never arms the laser and never changes calibration or safety settings.
let project = { name: 'Sans titre', file: null, modified: false, projects: [], recent: [], dir: '' };
let projDialogMode = 'open';
// A version opened as a copy (T-288): the name offered by « Enregistrer sous… ».
let projSuggested = '';
function showProject() {
  $('projName').textContent = project.name + (project.modified ? ' •' : '') + (project.read_only ? ' (lecture seule)' : '');
  $('projName').classList.toggle('modified', project.modified);
  // T-288: a project from a newer version: « Enregistrer » is off.
  $('projName').classList.toggle('ro', !!project.read_only);
  $('projName').title = project.read_only ? project.read_only_message : 'Projet ouvert (• = modifications non enregistrées)';
  $('projMenu').querySelector('[data-proj="save"]').disabled = !!project.read_only;
  showAutosave(project.autosave);
  showBackup(project.backup);
  $('projRecent').innerHTML = project.recent.length
    ? project.recent.map(n => `<button data-recent="${esc(n)}">${esc(n)}</button>`).join('') : '<div class="hint">Aucun projet récent : « Enregistrer » garde tout le show dans un projet.</div>';
  $('projRecent').querySelectorAll('[data-recent]').forEach(b => b.onclick = () => { $('projMenu').hidden = true; projOpen(b.dataset.recent); });
}
async function loadProject() {
  const at = performance.now();
  try { project = await apiJson('GET', '/api/project'); takeProjectModified(project.modified, at); showProject(); } catch { /* studio restarting */ }
}
// ---------- site profiles and packages (T-289) ----------
// A site profile holds the venue's calibration, safety settings and
// projection zones. Loading one disarms; a looser one needs the
// confirmation, as a safety profile. A project may carry one: opening the
// project never applies it, the page asks (« Garder le profil actuel »
// by default). Packages (.lspack) carry a project and its media.
let siteView = null, sitePending = null;
async function loadSites() {
  try { siteView = await apiJson('GET', '/api/sites', undefined, { quiet: true }); showSites(); } catch { /* studio restarting */ }
}
function showSites() {
  const v = siteView;
  if (!v) return;
  const sel = $('siteSelect'), keep = sel.value;
  sel.replaceChildren(...v.sites.map(e => {
    const o = document.createElement('option');
    o.value = e.name; o.textContent = e.name + (e.damaged ? ' (abîmé)' : '') + (e.name === v.active ? ' (actif)' : '');
    return o;
  }));
  sel.value = v.sites.some(e => e.name === keep) ? keep : (v.active || '');
  const cur = v.sites.find(e => e.name === sel.value);
  $('siteInfo').textContent = v.active
    ? `Profil actif : ${v.active}${v.modified ? ' (réglages modifiés depuis, à enregistrer)' : ''}.`
    : 'Aucun profil de site actif.';
  if (cur && cur.damaged) $('siteInfo').textContent += ` « ${cur.name} » : ${cur.damaged}`;
  $('siteLoad').disabled = !cur || !!cur.damaged;
  $('siteDelete').disabled = !cur;
  const ps = v.project_site;
  $('siteProject').hidden = !ps;
  if (ps) {
    $('siteProjectText').textContent = ps.error ? 'Profil de site du projet illisible : ' + ps.error
      : `Le projet contient le profil « ${ps.name} »` + (ps.differences.length ? ' : ' + ps.differences.join(' · ') : ' (identique aux réglages en vigueur).');
    $('siteProjectUse').disabled = !!ps.error;
  }
}
// After a site change: calibration, safety, zones and the pill follow.
async function afterSiteChange() {
  try { await loadState(); } catch { /* studio restarting */ }
  loadProjection();
}
/** The answer of a site route: 409 → the loosening list and confirmation. */
async function siteReply(r, retry, box) {
  $('siteError').textContent = '';
  if (r.ok) {
    sitePending = null; $('siteLoosen').hidden = true;
    siteView = await r.json(); showSites();
    await afterSiteChange();
    return true;
  }
  if (r.status === 409) {
    const j = await r.json();
    sitePending = retry;
    const text = 'Le profil est moins restrictif : ' + j.loosen.join(' · ');
    if (box === 'ask') { $('siteAskLoosenText').textContent = text.replace('Le profil', 'Le profil du projet'); $('siteAskLoosen').hidden = false; $('siteAskConfirm').focus(); }
    else { $('siteLoosenText').textContent = text; $('siteLoosen').hidden = false; }
  } else {
    const msg = r.status === 423 ? LOCKED_FR : await r.text();
    if (box === 'ask') $('siteAskError').textContent = msg; else $('siteError').textContent = msg;
    loadSites();
  }
  return false;
}
async function siteLoad(name, confirm = false) {
  const r = await api('POST', '/api/sites', { action: 'load', name, confirm_loosen: confirm, source: 'ui' }, { quiet: true });
  await siteReply(r, () => siteLoad(name, true));
}
async function siteUseProject(confirm = false, box = 'panel') {
  const r = await api('POST', '/api/sites/use-project', { confirm_loosen: confirm, source: 'ui' }, { quiet: true });
  const ok = await siteReply(r, () => siteUseProject(true, box), box);
  if (ok && box === 'ask') $('siteAsk').hidden = true;
  return ok;
}
$('sitePanel').addEventListener('toggle', () => { if ($('sitePanel').open) loadSites(); });
$('siteSelect').addEventListener('change', () => { sitePending = null; $('siteLoosen').hidden = true; showSites(); });
$('siteLoad').addEventListener('click', () => siteLoad($('siteSelect').value));
$('siteLoosenOk').addEventListener('click', () => { const f = sitePending; sitePending = null; if (f) f(); });
$('siteLoosenCancel').addEventListener('click', () => { sitePending = null; $('siteLoosen').hidden = true; });
$('siteSaveAs').addEventListener('click', () => { $('siteName').value = (siteView && siteView.active) || ''; $('siteSaveBox').hidden = false; $('siteName').focus(); });
$('siteSaveCancel').addEventListener('click', () => { $('siteSaveBox').hidden = true; });
$('siteSaveOk').addEventListener('click', async () => {
  const name = $('siteName').value.trim();
  if (!name) { $('siteError').textContent = 'Donnez un nom au profil de site.'; return; }
  if (siteView && siteView.sites.some(e => e.name === name) && name !== siteView.active && !confirm(`Remplacer le profil de site « ${name} » ?`)) return;
  const r = await api('POST', '/api/sites', { action: 'save', name, source: 'ui' }, { quiet: true });
  if (await siteReply(r, null)) $('siteSaveBox').hidden = true;
});
$('siteDelete').addEventListener('click', async () => {
  const name = $('siteSelect').value;
  if (!name || !confirm(`Supprimer le profil de site « ${name} » ? Les réglages en vigueur ne changent pas.`)) return;
  const r = await api('POST', '/api/sites', { action: 'delete', name, source: 'ui' }, { quiet: true });
  await siteReply(r, null);
});
$('siteProjectUse').addEventListener('click', () => siteUseProject(false, 'panel'));
// After opening a project that carries a different site profile.
async function offerSite() {
  if (!project.site_offer) return;
  await loadSites();
  const ps = siteView && siteView.project_site;
  if (!ps || ps.error) return;
  $('siteAskWhat').textContent = `Profil du projet : « ${ps.name} ». Profil en vigueur : « ${siteView.active || 'aucun'} ». Rien n'a changé tant que vous ne choisissez pas « Utiliser celui du projet ».`;
  $('siteAskDiff').replaceChildren(...ps.differences.map(d => { const li = document.createElement('li'); li.textContent = d; return li; }));
  $('siteAskLoosen').hidden = true; $('siteAskError').textContent = '';
  $('siteAsk').hidden = false;
  $('siteAskKeep').focus();
}
$('siteAskKeep').addEventListener('click', () => { sitePending = null; $('siteAsk').hidden = true; });
$('siteAskUse').addEventListener('click', () => siteUseProject(false, 'ask'));
$('siteAskConfirm').addEventListener('click', () => { const f = sitePending; sitePending = null; if (f) f(); });
// Exporter un paquet… : a download (no fetch: the browser saves the file).
$('packExportOk').addEventListener('click', () => {
  const a = document.createElement('a');
  a.href = '/api/pack/export?site=' + ($('packSite').checked ? 1 : 0);
  a.download = (project.name || 'projet') + '.lspack';
  document.body.append(a); a.click(); a.remove();
  $('packDialog').hidden = true;
});
$('packExportClose').addEventListener('click', () => { $('packDialog').hidden = true; });
// Importer un paquet… : the file's bytes; the studio checks everything.
let packImported = null;
function packImportDialog() {
  packImported = null;
  $('packFile').value = ''; $('packName').value = '';
  $('packImportMsg').textContent = ''; $('packImportError').textContent = '';
  $('packImportOpen').hidden = true;
  $('packImportDialog').hidden = false;
  $('packFile').focus();
}
$('packImportOk').addEventListener('click', async () => {
  const file = $('packFile').files[0];
  $('packImportError').textContent = '';
  if (!file) { $('packImportError').textContent = 'Choisissez un fichier .lspack.'; return; }
  $('packImportMsg').textContent = `Lecture de « ${file.name} »…`;
  const name = $('packName').value.trim();
  let r;
  try { r = await api('POST', '/api/pack/import' + (name ? '?name=' + encodeURIComponent(name) : ''), file, { quiet: true }); }
  catch { $('packImportMsg').textContent = ''; $('packImportError').textContent = 'Le studio ne répond pas.'; return; }
  if (!r.ok) { $('packImportMsg').textContent = ''; $('packImportError').textContent = 'Import impossible : ' + (r.status === 423 ? LOCKED_FR : await r.text()); return; }
  const d = await r.json();
  packImported = d.project;
  const parts = [`Projet « ${d.project} » importé avec ${d.media.length} média${d.media.length > 1 ? 's' : ''}.`];
  if (d.renamed.length) parts.push('Renommés : ' + d.renamed.map(([a, b]) => `${a} → ${b}`).join(', ') + '.');
  if (d.missing.length) parts.push('Absents du paquet : ' + d.missing.join(', ') + '.');
  if (d.site) parts.push(`Profil de site « ${d.site} » proposé à l'ouverture.`);
  $('packImportMsg').textContent = parts.join(' ');
  $('packImportOpen').hidden = false; $('packImportOpen').focus();
  loadProject(); loadIldaLib(); loadSongs();
});
$('packImportOpen').addEventListener('click', async () => {
  if (!packImported) return;
  $('packImportDialog').hidden = true;
  await projOpen(packImported);
});
$('packImportClose').addEventListener('click', () => { $('packImportDialog').hidden = true; });
// Importer depuis un projet… : chosen parts, conflicts by name.
async function partialDialog() {
  await loadProject();
  const others = project.projects.filter(n => n !== project.name);
  $('partialFrom').replaceChildren(...others.map(n => { const o = document.createElement('option'); o.value = n; o.textContent = n; return o; }));
  $('partialPreview').textContent = others.length ? '' : 'Aucun autre projet enregistré.';
  $('partialError').textContent = '';
  $('partialOk').disabled = $('partialCheck').disabled = !others.length;
  $('partialDialog').hidden = false;
  $('partialFrom').focus();
}
const PARTIAL_LABELS = { pages: 'Pages', user_cues: 'Cues utilisateur', scenes: 'Scènes', midi: 'Correspondances MIDI', site: 'Lieu', timelines: 'Timelines' };
function partialBody(dry) {
  const sections = [...$('partialParts').querySelectorAll('input:checked')].map(i => i.value);
  return { from: $('partialFrom').value, sections, conflict: $('partialConflict').value, dry_run: dry };
}
function partialSummary(rep) {
  return Object.entries(rep || {}).map(([k, r]) => {
    const bits = [`${r.added} ajouté${r.added > 1 ? 's' : ''}`];
    if (r.replaced) bits.push(`${r.replaced} remplacé${r.replaced > 1 ? 's' : ''}`);
    if (r.renamed.length) bits.push(`${r.renamed.length} renommé${r.renamed.length > 1 ? 's' : ''}`);
    if (r.skipped) bits.push(`${r.skipped} ignoré${r.skipped > 1 ? 's' : ''}`);
    return `${PARTIAL_LABELS[k] || k} : ${bits.join(', ')}` + (r.conflicts.length ? ` (noms déjà pris : ${r.conflicts.join(', ')})` : '');
  }).join(' · ');
}
async function partialRun(dry) {
  $('partialError').textContent = '';
  const body = partialBody(dry);
  if (!body.sections.length) { $('partialError').textContent = 'Cochez au moins une partie.'; return; }
  try {
    await projPost('import-partial', body);
    $('partialPreview').textContent = (dry ? 'Aperçu — ' : 'Importé — ') + partialSummary(project.import);
    if (!dry) { await afterOpen(); }
  } catch (e) { $('partialError').textContent = e.message; }
}
$('partialCheck').addEventListener('click', () => partialRun(true));
$('partialOk').addEventListener('click', () => partialRun(false));
$('partialClose').addEventListener('click', () => { $('partialDialog').hidden = true; });
// ---------- autosave and recovery (T-287) ----------
// The studio writes autosaves on its own thread; the page only shows the
// status, the settings and, after a crash, the recovery question.
// Recovering never arms the laser.
const hhmm = ms => new Date(ms).toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' });
function showAutosave(a) {
  if (!a) return;
  const st = $('autosaveStatus');
  st.classList.toggle('err', !!a.error);
  if (a.error) { st.textContent = '⚠ Échec de la sauvegarde auto'; st.title = a.error; }
  else if (!a.enabled) { st.textContent = ''; st.title = ''; }
  else if (a.last_ok_ms) { st.textContent = 'Sauvegarde auto ' + hhmm(a.last_ok_ms); st.title = 'Dernière sauvegarde auto, dans ' + a.dir; }
  else { st.textContent = ''; st.title = ''; }
  st.hidden = !st.textContent;
  if (document.activeElement !== $('asEnabled')) $('asEnabled').checked = a.enabled;
  if (document.activeElement !== $('asDelay')) { $('asDelay').min = a.min_delay_s; $('asDelay').max = a.max_delay_s; $('asDelay').value = a.delay_s; }
  $('asDelay').disabled = !a.enabled;
  $('asInfo').textContent = a.enabled ? `Sauvegarde ${a.delay_s} s après la dernière modification, dans ${a.dir}` : 'Désactivée';
  const offer = a.offer;
  if (offer && $('recoverAsk').hidden && !VUE) { // asked in the main window only (T-281)
    const when = new Date(offer.saved_at_ms).toLocaleString('fr-FR', { dateStyle: 'short', timeStyle: 'short' });
    $('recoverWhen').textContent = `Projet « ${offer.project} » : sauvegarde auto du ${when}, plus récente que le projet enregistré.`;
    $('recoverError').textContent = '';
    $('recoverAsk').hidden = false;
  } else if (!offer) $('recoverAsk').hidden = true;
}
$('recoverOk').onclick = async () => {
  try { await projPost('recover'); $('recoverAsk').hidden = true; await afterOpen(); }
  catch (e) { $('recoverError').textContent = e.message; }
};
$('recoverIgnore').onclick = async () => {
  try { await projPost('recover-ignore'); $('recoverAsk').hidden = true; }
  catch (e) { $('recoverError').textContent = e.message; }
};
$('asEnabled').onchange = e => projPost('autosave', { enabled: e.target.checked }).catch(err => alert(err.message));
$('asDelay').addEventListener('change', e => projPost('autosave', { delay_s: Math.round(+e.target.value) || 30 }).catch(err => alert(err.message)));
async function projPost(action, body) {
  const at = performance.now();
  const r = await api('POST', '/api/project/' + action, body ?? {}, { quiet: true });
  if (!r.ok) throw new Error(await r.text());
  project = await r.json();
  // Saved, opened or new: nothing unsaved any more (T-433).
  if (['save', 'save-as', 'open', 'new'].includes(action)) projectSaved();
  if (action !== 'version-save') projSuggested = project.suggested_name || '';
  takeProjectModified(project.modified, at);
  showProject();
  if (project.notice) flash(project.notice);
  if (project.warnings?.length) alert(project.warnings.join('\n'));
}
// Everything a project brings back, reloaded into the page.
async function afterOpen() {
  await loadState(); await loadPresets(); await loadPalettes(); await loadLfos(); await loadFxGrid(); await loadRoutes(); await loadAudioPresets(); await loadTriggers(); await loadShows();
  if (!$('figEditor').hidden) loadFigLib();
}
// Unsaved changes (T-433), before « Nouveau », « Ouvrir » and quitting:
// one question, listing what would be lost (`what`, from unsavedWork).
// Resolves true to go on, false to stay. « Enregistrer » saves the
// timeline's show, then the project; a *Sans titre* project opens
// « Enregistrer sous… » instead, and a failed save stays. Entrée =
// « Enregistrer » (the shortcut table); Échap stays the emergency stop
// and leaves the question open.
function askUnsaved(what = unsavedWork({ figure: false })) {
  if (!what.length) return Promise.resolve(true);
  $('projAskWhat').replaceChildren(...what.map(w => { const li = document.createElement('li'); li.textContent = w.label; return li; }));
  $('projAsk').hidden = false;
  $('askSave').focus();
  return new Promise(resolve => {
    const done = go => { $('projAsk').hidden = true; resolve(go); };
    $('askDiscard').onclick = () => done(true);
    $('askCancel').onclick = () => done(false);
    $('askSave').onclick = async () => {
      try {
        // The figure under its name (CRÉATION › Figures says why if it can't).
        if (what.some(w => w.kind === 'figure') && !await saveFigure()) { done(false); return; }
        if (what.some(w => w.kind === 'program') && !await pgSaveNow()) { done(false); return; }
        const tl = what.find(w => w.kind === 'timeline');
        if (tl) {
          if (tl.example) throw new Error('Show de timeline : un exemple s\'enregistre sous un nom, avec TIMELINE › Enregistrer.');
          const r = await api('POST', '/api/timeline/save', {}, { quiet: true });
          if (!r.ok) throw new Error('Show de timeline : ' + await r.text());
        }
        // A saved show changes the project's files: the project is saved after it.
        if (tl || what.some(w => w.kind === 'project')) {
          if (!project.file || project.read_only) { done(false); projDialog('save-as'); return; }
          await projPost('save');
        }
        done(true);
      } catch (e) { alert(e.message); done(false); }
    };
  });
}
// Quitting the application (T-413 calls it from « Quitter »): what the
// studio says would be lost, plus the figure being drawn here. Resolves
// true when quitting may go on. Never holds up a stop: the studio quits
// and disarms on a signal whatever is asked here.
async function askBeforeQuit() {
  let check = null;
  try { check = await apiJson('GET', '/api/project/quit-check', undefined, { quiet: true }); } catch { /* asked from what the page knows */ }
  if (!check) return askUnsaved(unsavedWork());
  const what = [];
  if (check.unsaved.includes('projet')) what.push({ kind: 'project', label: `Projet « ${check.name} »` });
  if (check.timeline) what.push({ kind: 'timeline', label: `Show de timeline « ${check.timeline} »`, example: !!(lastTimeline && lastTimeline.example) });
  if (figureDirty()) what.push({ kind: 'figure', label: 'Figure en cours' });
  if (pgDirty()) what.push({ kind: 'program', label: `Programme du cue « ${pg.info ? pg.info.name : pg.id} »` });
  return askUnsaved(what);
}
window.laserStudioQuitCheck = askBeforeQuit;
function projDialog(mode) {
  projDialogMode = mode;
  $('projDlgTitle').textContent = mode === 'open' ? 'Ouvrir…' : 'Enregistrer sous…';
  $('projOk').textContent = mode === 'open' ? 'Ouvrir' : 'Enregistrer';
  $('projDir').textContent = 'Dossier : ' + project.dir;
  $('projPath').value = mode === 'open' ? '' : (project.file && !project.read_only ? project.name : projSuggested);
  $('projError').textContent = '';
  $('projFiles').innerHTML = project.projects.length
    ? project.projects.map(n => `<button data-pfile="${esc(n)}">${esc(n)}</button>`).join('') : '<div class="hint">Aucun projet enregistré.</div>';
  $('projFiles').querySelectorAll('[data-pfile]').forEach(b => b.onclick = () => { $('projPath').value = b.dataset.pfile; if (mode === 'open') projDialogOk(); });
  $('projDialog').hidden = false;
  $('projPath').focus();
}
async function projDialogOk() {
  const path = $('projPath').value.trim();
  if (!path) { $('projError').textContent = 'Donnez un nom au projet.'; return; }
  try {
    if (projDialogMode === 'open') {
      await projPost('open', { path }); $('projDialog').hidden = true; await afterOpen(); offerSite();
    } else {
      const exists = project.projects.includes(path.replace(/\.lsproj$/, '')) && path !== project.name;
      if (exists && !confirm(`Remplacer le projet « ${path} » ?`)) return;
      await projPost('save-as', { path }); $('projDialog').hidden = true;
    }
  } catch (e) { $('projError').textContent = e.message; }
}
async function projOpen(name) {
  if (showMode) return flash(LOCKED_FR);
  if (!await askUnsaved()) return;
  try { await projPost('open', { path: name }); await afterOpen(); offerSite(); } catch (e) { alert(e.message); }
}
async function projAction(kind) {
  $('projMenu').hidden = true;
  // À propos (T-452): reads only, so allowed in show mode too.
  if (kind === 'about') return openAbout();
  // T-283: in show mode, only « Enregistrer » (the open project, in place).
  if (showMode && kind !== 'save') return flash(LOCKED_FR);
  await loadProject();
  if (kind === 'save') {
    if (project.read_only) { flash(project.read_only_message); return; }
    if (!project.file) { projDialog('save-as'); return; }
    try { await projPost('save'); } catch (e) { alert(e.message); }
  } else if (kind === 'save-as') {
    projDialog('save-as');
  } else if (kind === 'open') {
    if (await askUnsaved()) projDialog('open');
  } else if (kind === 'new') {
    if (!await askUnsaved()) return;
    newProjDialog();
  } else if (kind === 'save-template') {
    projTplDialog();
  } else if (kind === 'version-save' || kind === 'versions') {
    verDialog(kind === 'version-save');
  } else if (kind === 'pack-export') {
    $('packDialog').hidden = false; $('packExportOk').focus();
  } else if (kind === 'pack-import') {
    packImportDialog();
  } else if (kind === 'partial') {
    partialDialog();
  }
}
// Nouveau projet (T-451): what is taken from the open project. The
// machine's settings (calibration, safety, outputs) are never part of a
// project, so they always stay; creating a project never arms. The last
// choices are remembered in this browser only.
const NEW_KEEP_KEY = 'ls.newProjectKeep';
function newProjDialog() {
  let saved = null;
  try { saved = JSON.parse(localStorage.getItem(NEW_KEEP_KEY) || 'null'); } catch { saved = null; }
  if (saved && typeof saved === 'object') {
    $('newProjKeep').querySelectorAll('input').forEach(i => { if (typeof saved[i.value] === 'boolean') i.checked = saved[i.value]; });
  }
  $('newProjError').textContent = '';
  $('newProjDialog').hidden = false;
  $('newProjOk').focus();
}
$('newProjOk').addEventListener('click', async () => {
  const keep = {};
  $('newProjKeep').querySelectorAll('input').forEach(i => { keep[i.value] = i.checked; });
  try { localStorage.setItem(NEW_KEEP_KEY, JSON.stringify(keep)); } catch { /* private window: not remembered */ }
  try { await projPost('new', { keep }); $('newProjDialog').hidden = true; await afterOpen(); offerSite(); }
  catch (e) { $('newProjError').textContent = e.message; }
});
$('newProjCancel').addEventListener('click', () => { $('newProjDialog').hidden = true; });
// Templates (T-451): projects in <data-dir>/templates/. « Nouveau depuis
// un modèle » opens an untitled copy (the template is never written over
// by « Enregistrer »).
async function showProjTemplates() {
  const box = $('projTemplates');
  const btn = $('projMenu').querySelector('[data-proj="new-template"]');
  if (!box.hidden) { box.hidden = true; btn.setAttribute('aria-expanded', 'false'); return; }
  let names = [];
  try { names = (await apiJson('GET', '/api/project/templates', undefined, { quiet: true })).templates || []; } catch (e) { flash(e.message); }
  box.replaceChildren();
  if (!names.length) {
    const d = document.createElement('div'); d.className = 'hint'; d.textContent = 'Aucun modèle'; box.append(d);
  }
  for (const n of names) {
    const row = document.createElement('div'); row.className = 'tpl-row';
    const open = document.createElement('button'); open.dataset.template = n; open.textContent = n;
    open.onclick = () => newFromTemplate(n);
    const del = document.createElement('button'); del.dataset.templateDelete = n; del.textContent = '✕';
    del.setAttribute('aria-label', `Supprimer le modèle « ${n} »`); del.title = 'Supprimer ce modèle';
    del.onclick = e => { e.stopPropagation(); deleteProjTemplate(n); };
    row.append(open, del); box.append(row);
  }
  box.hidden = false; btn.setAttribute('aria-expanded', 'true');
}
async function newFromTemplate(name) {
  $('projMenu').hidden = true; $('projTemplates').hidden = true;
  if (showMode) return flash(LOCKED_FR);
  await loadProject();
  if (!await askUnsaved()) return;
  try { await projPost('new', { template: name }); await afterOpen(); offerSite(); } catch (e) { alert(e.message); }
}
async function deleteProjTemplate(name) {
  if (!confirm(`Supprimer le modèle « ${name} » ?`)) return;
  try { await projPost('delete-template', { name }); } catch (e) { alert(e.message); }
  $('projTemplates').hidden = true; showProjTemplates();
}
function projTplDialog() {
  $('tplName').value = project.file ? project.name : '';
  $('tplError').textContent = '';
  $('tplDialog').hidden = false;
  $('tplName').focus();
}
async function projTplSave() {
  const name = $('tplName').value.trim();
  if (!name) { $('tplError').textContent = 'Donnez un nom au modèle.'; return; }
  const save = async replace => {
    const r = await api('POST', '/api/project/save-template', { name, replace }, { quiet: true });
    if (r.status === 409 && !replace) return confirm('Un modèle porte déjà ce nom : le remplacer ?') ? save(true) : false;
    if (!r.ok) throw new Error(await r.text());
    const v = await r.json();
    if (v.notice) flash(v.notice);
    return true;
  };
  try { if (await save(false)) $('tplDialog').hidden = true; } catch (e) { $('tplError').textContent = e.message; }
}
$('tplOk').addEventListener('click', projTplSave);
$('tplName').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); projTplSave(); } });
$('tplCancel').addEventListener('click', () => { $('tplDialog').hidden = true; });
$('projBtn').onclick = e => { e.stopPropagation(); $('projMenu').hidden = !$('projMenu').hidden; if (!$('projMenu').hidden) loadProject(); };
$('projMenu').querySelectorAll('[data-proj]').forEach(b => b.onclick = () => b.dataset.proj === 'new-template' ? (showMode ? flash(LOCKED_FR) : showProjTemplates()) : projAction(b.dataset.proj));
document.addEventListener('click', e => { if (!e.target.closest('.proj')) $('projMenu').hidden = true; });
// À propos de Laser Studio (T-452): everything comes from the studio
// itself (version, folders, licences, release notes); no update check.
// Notes are simple Markdown (### titles, - lists, paragraphs), turned into
// elements with textContent: never interpreted as HTML.
function renderNotesMd(md) {
  const root = document.createElement('div');
  root.className = 'about-md';
  let list = null, para = null;
  for (const raw of md.split('\n')) {
    const line = raw.trim();
    if (!line) { list = null; para = null; continue; }
    if (line.startsWith('### ')) {
      list = null; para = null;
      const h = document.createElement('h4'); h.textContent = line.slice(4); root.append(h);
    } else if (line.startsWith('- ')) {
      para = null;
      if (!list) { list = document.createElement('ul'); root.append(list); }
      const li = document.createElement('li'); li.textContent = line.slice(2); list.append(li);
    } else if (para) {
      para.textContent += ' ' + line;
    } else {
      list = null;
      para = document.createElement('p'); para.textContent = line; root.append(para);
    }
  }
  return root;
}
function releaseBlock(r, withTitle) {
  const div = document.createElement('div');
  div.className = 'about-release';
  if (withTitle) { const h = document.createElement('h4'); h.textContent = `Version ${r.version} — ${r.date}`; div.append(h); }
  div.append(renderNotesMd(r.notes_md));
  return div;
}
function aboutTab(which) {
  const notes = which === 'notes';
  $('aboutTabInfo').setAttribute('aria-selected', String(!notes)); $('aboutTabInfo').tabIndex = notes ? -1 : 0;
  $('aboutTabNotes').setAttribute('aria-selected', String(notes)); $('aboutTabNotes').tabIndex = notes ? 0 : -1;
  $('aboutInfo').hidden = notes; $('aboutNotes').hidden = !notes;
}
async function openAbout() {
  aboutTab('info');
  $('aboutError').textContent = ''; $('aboutCopied').textContent = '';
  $('aboutDialog').hidden = false;
  let a;
  try { a = await apiJson('GET', '/api/about', undefined, { quiet: true }); }
  catch (e) { $('aboutError').textContent = 'Informations indisponibles : ' + e.message; return; }
  $('aboutVersion').textContent = a.name + ' ' + a.version + (a.commit ? ` (${a.commit})` : '');
  $('aboutBuild').textContent = a.build_date;
  $('aboutDataDir').textContent = a.data_dir;
  $('aboutProjDir').textContent = a.projects_dir;
  $('aboutFormat').textContent = String(a.project_format);
  $('aboutSafety').textContent = a.safety;
  $('aboutNetwork').textContent = a.network;
  $('aboutLibs').textContent = a.libraries;
  const third = $('aboutThird');
  third.replaceChildren();
  if (!a.third_party.length) {
    const p = document.createElement('p'); p.className = 'hint'; p.textContent = a.third_party_note; third.append(p);
  } else {
    const t = document.createElement('table'); t.className = 'about-third';
    const head = t.createTHead().insertRow();
    for (const h of ['Fichiers', 'Source', 'Licence', 'Vérifié le']) { const th = document.createElement('th'); th.textContent = h; head.append(th); }
    const body = t.createTBody();
    for (const f of a.third_party) { const row = body.insertRow(); for (const v of [f.files, f.url, f.licence, f.checked_on]) row.insertCell().textContent = v; }
    third.append(t);
  }
  $('aboutNotes').replaceChildren(...a.changelog.map(r => releaseBlock(r, true)));
}
async function copyAboutPath(id) {
  const path = $(id).textContent;
  try { await navigator.clipboard.writeText(path); $('aboutCopied').textContent = 'Chemin copié.'; }
  catch { $('aboutCopied').textContent = 'Copie impossible : sélectionnez le chemin à la main.'; }
}
$('aboutClose').onclick = () => { $('aboutDialog').hidden = true; };
$('aboutTabInfo').onclick = () => aboutTab('info');
$('aboutTabNotes').onclick = () => aboutTab('notes');
for (const id of ['aboutTabInfo', 'aboutTabNotes']) $(id).addEventListener('keydown', e => {
  if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
  e.preventDefault(); e.stopPropagation();
  const toNotes = id === 'aboutTabInfo';
  aboutTab(toNotes ? 'notes' : 'info'); $(toNotes ? 'aboutTabNotes' : 'aboutTabInfo').focus();
});
$('aboutCopyData').onclick = () => copyAboutPath('aboutDataDir');
$('aboutCopyProj').onclick = () => copyAboutPath('aboutProjDir');
// After an update: the new version's notes, once. On a new data folder the
// studio records the version itself and sends nothing to show.
async function checkWhatsNew() {
  let a;
  try { a = await apiJson('GET', '/api/about', undefined, { quiet: true }); } catch { return; }
  if (!a.unseen) return;
  $('whatsNewTitle').textContent = `Nouveautés de la version ${a.unseen.version}`;
  $('whatsNewBody').replaceChildren(releaseBlock(a.unseen, false));
  $('whatsNew').hidden = false;
}
$('whatsNewClose').onclick = async () => {
  $('whatsNew').hidden = true;
  try { await api('POST', '/api/about/seen', {}); } catch { /* shown again on the next load */ }
};
if (!VUE) checkWhatsNew(); // the main window only (T-281)
$('projOk').onclick = projDialogOk;
$('projCancel').onclick = () => { $('projDialog').hidden = true; };
$('projPath').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); projDialogOk(); } });
loadProject();
setInterval(loadProject, 2000);

// ---------- named versions (T-288) ----------
// « Enregistrer une version… » keeps the show as it is now under a free
// name (versions/<projet>/ in the data folder); « Ouvrir comme copie »
// opens one as a new unsaved project, never over the one it came from.
// Nothing here arms the laser.
async function loadVersions() {
  try {
    const v = await apiJson('GET', '/api/project/versions');
    $('verProject').textContent = `Projet « ${v.project} » — ${v.versions.length} version${v.versions.length > 1 ? 's' : ''}`;
    $('verList').innerHTML = v.versions.length ? v.versions.map(x => `<div class="ver" data-file="${esc(x.file)}">
      <span class="n">${esc(x.name)}</span><span class="d">${esc(new Date(x.at_ms).toLocaleString('fr-FR', { dateStyle: 'short', timeStyle: 'short' }))}</span>
      <button data-ver-open>Ouvrir comme copie</button><button data-ver-del>Supprimer</button></div>`).join('')
      : '<div class="hint">Aucune version : donnez un nom (« avant festival », « version courte ») et enregistrez.</div>';
  } catch (e) { $('verError').textContent = e.message; }
}
function verDialog(focusName) {
  if (showMode) return flash(LOCKED_FR);
  $('verError').textContent = '';
  $('verName').value = '';
  $('verDialog').hidden = false;
  loadVersions();
  if (focusName) $('verName').focus();
}
async function verSave() {
  const name = $('verName').value.trim();
  if (!name) { $('verError').textContent = 'Donnez un nom à la version.'; return; }
  try { await projPost('version-save', { name }); $('verName').value = ''; $('verError').textContent = ''; loadVersions(); }
  catch (e) { $('verError').textContent = e.message; }
}
$('verSave').onclick = verSave;
$('verName').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); verSave(); } });
$('verClose').onclick = () => { $('verDialog').hidden = true; };
$('verList').addEventListener('click', async e => {
  const row = e.target.closest('.ver'); if (!row) return;
  const file = row.dataset.file, name = row.querySelector('.n').textContent;
  if (e.target.matches('[data-ver-del]')) {
    if (!confirm(`Supprimer la version « ${name} » ?`)) return;
    try { await projPost('version-delete', { file }); loadVersions(); } catch (err) { $('verError').textContent = err.message; }
  } else if (e.target.matches('[data-ver-open]')) {
    $('verDialog').hidden = true;
    if (!await askUnsaved()) return;
    try { await projPost('version-open', { file }); await afterOpen(); } catch (err) { alert(err.message); }
  }
});

// ---------- copy to a second location (T-450) ----------
// The studio writes the copy on its own thread after each save and
// autosave; the page shows the settings, the status line and, when the
// disk space is low, an orange banner that can be closed.
const goFr = b => (b / 1e9).toFixed(1).replace('.', ',') + ' Go';
let spaceDismissed = '';
function showBackup(b) {
  if (!b) return;
  const st = b.status, set = b.settings;
  const el = $('backupStatus');
  el.classList.toggle('err', !!st.warning);
  if (st.error) { el.textContent = '⚠ Copie de sauvegarde indisponible'; el.title = st.error; }
  else if (st.low_space) { el.textContent = '⚠ Espace disque faible'; el.title = st.low_space; }
  else if (set.enabled && set.dir && st.last_backup_ms) { el.textContent = 'Copie ' + hhmm(st.last_backup_ms); el.title = `Dernière copie, dans ${set.dir}/${b.folder}`; }
  else { el.textContent = ''; el.title = ''; }
  el.hidden = !el.textContent;
  if (document.activeElement !== $('bkEnabled')) $('bkEnabled').checked = set.enabled;
  if (document.activeElement !== $('bkDir')) $('bkDir').value = set.dir || '';
  $('bkNow').disabled = !(set.enabled && set.dir);
  const parts = [];
  if (!set.enabled) parts.push('Désactivée');
  else if (!set.dir) parts.push('Aucun dossier choisi');
  else {
    parts.push(st.last_backup_ms ? 'Dernière copie : ' + hhmm(st.last_backup_ms) : 'Aucune copie encore');
    if (st.backup_free_bytes != null) parts.push(goFr(st.backup_free_bytes) + ' libres');
  }
  $('bkInfo').textContent = parts.join(' — ');
  const low = st.low_space || '';
  $('spaceWarnText').textContent = low;
  $('spaceWarn').hidden = !low || low === spaceDismissed;
}
$('spaceWarnClose').onclick = () => { spaceDismissed = $('spaceWarnText').textContent; $('spaceWarn').hidden = true; };
async function backupPost(path, body) {
  const r = await api('POST', path, body, { quiet: true });
  if (!r.ok) throw new Error(await r.text());
  const b = await r.json();
  project.backup = b; showBackup(b);
  return b;
}
$('bkEnabled').onchange = async e => {
  $('bkError').textContent = '';
  try { await backupPost('/api/backup', { enabled: e.target.checked }); } catch (err) { $('bkError').textContent = err.message; e.target.checked = !e.target.checked; }
};
$('bkDir').addEventListener('change', async e => {
  $('bkError').textContent = '';
  try { await backupPost('/api/backup', { dir: e.target.value.trim() || null }); } catch (err) { $('bkError').textContent = err.message; }
});
$('bkDir').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); e.target.blur(); } });
$('bkNow').onclick = async () => {
  $('bkResult').textContent = 'Copie en cours…';
  try {
    const before = await backupPost('/api/backup/now', {});
    const was = before.status.last_backup_ms;
    // Written by the studio's copy thread: wait for its result (10 s at most).
    for (let i = 0; i < 40; i++) {
      await new Promise(r => setTimeout(r, 250));
      const b = await apiJson('GET', '/api/backup', undefined, { quiet: true });
      project.backup = b; showBackup(b);
      if (b.status.last_backup_ms !== was && b.status.backup_ok) { $('bkResult').textContent = 'Copie faite à ' + hhmm(b.status.last_backup_ms); return; }
      if (b.status.backup_ok === false) { $('bkResult').textContent = b.status.error; return; }
    }
    $('bkResult').textContent = 'Copie toujours en cours (dossier lent)';
  } catch (e) { $('bkResult').textContent = e.message; }
};

export { afterOpen, askBeforeQuit, projAction, project };
