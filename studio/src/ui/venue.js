// Laser Studio — venue.js (T-435).
// Sections (as in the former index.html script):
//   avant le show (T-258)
//   RÉGLAGES › Lieu et projecteurs (T-277)
//   safety profiles per venue (T-260)
//   boot
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { loadArtnet, loadAudioOut, loadDmxOut, reloadAudience, reloadDevices, reloadOutColor, reloadOutputs,
  showAudienceMode, showDataWarnings } from './outputs.js';
import { $, api, apiJson, armed, assignArmed, assignCalibration, editLook, showSettings, takeLook } from './core.js';
import { assignArm, assignChecklistState, assignShownetState, checklistState, clock, showArmed,
  showShownet, shownetState } from './laser.js';
import { GALVO, galvoFilter, galvoPps, takeGalvo } from './presence.js';
import { assignCam3dRestored, cam3dRestored, outdoorTo3d, show3d, showCam3d, view3d } from './view3d.js';
import { activeWs, setWorkspace, showShowMode } from './workspaces.js';
import { showMusSafety } from './music.js';
import { assignScenes, renderScenes, showCalibration } from './cuelist.js';
import { proj, showProjection } from './zones.js';
import { assignSafety, reloadSafety, showOutdoorSettings, showSafety } from './safety.js';
import { GEN_NAMES } from './layers.js';

// ---------- avant le show (T-258) ----------
const CL_WHEN = { always: 'Toujours', outdoor: 'Extérieur', audience_scan: 'Balayage du public' };
let checklist = null, clEditing = null, clOpTimer = null;
async function clPost(action, body) {
  const r = await api('POST', '/api/checklist/' + action, body ?? {}, { quiet: true });
  $('clError').textContent = '';
  if (r.ok) { showChecklist(await r.json()); return true; }
  if (r.status === 409) {
    const j = await r.json();
    $('clError').textContent = 'À faire encore : ' + j.missing.join(' · ');
    showChecklist(j.checklist);
  } else {
    // Refused: show the reason and the list as the server has it.
    const why = await r.text();
    await reloadChecklist();
    $('clError').textContent = why;
  }
  return false;
}
async function reloadChecklist() {
  const r = await api('GET', '/api/checklist');
  if (r.ok) showChecklist(await r.json());
}
function openChecklist() {
  setWorkspace('settings');
  $('checklistPanel').open = true;
  $('checklistPanel').scrollIntoView({ block: 'start', behavior: 'smooth' });
}
function showChecklist(c) {
  checklist = c;
  assignChecklistState({ enforced: c.enforced, complete: c.complete });
  assignShownetState({ ...(shownetState || {}), active: c.output === 'shownet', hw_stop_ok: c.hw_stop_ok });
  $('clShownetHelp').hidden = c.output !== 'shownet';
  showArmed();
  const led = $('clLed');
  const hwTodo = c.output === 'shownet' && !c.hw_stop_ok;
  led.className = c.complete && !hwTodo ? 'done' : c.enforced || hwTodo ? 'todo' : '';
  led.textContent = c.complete && !hwTodo ? 'Terminée' : c.enforced ? 'À faire avant d\'armer' : hwTodo ? 'Arrêt matériel à vérifier' : 'Aperçu : facultative';
  if (c.load_error) $('clError').textContent = c.load_error;
  // Never overwrite a name being typed or not sent yet.
  if (document.activeElement !== $('clOperator') && !clOpTimer) $('clOperator').value = c.operator;
  $('clOutdoor').checked = c.outdoor; $('clScan').checked = c.audience_scan;
  for (const id of ['clOperator', 'clOutdoor', 'clScan', 'clComplete']) $(id).disabled = c.complete;
  $('clRun').hidden = !!clEditing;
  $('clDone').hidden = !c.complete;
  if (c.complete) $('clDone').textContent = `Liste terminée par ${c.operator} à ${clock(c.completed_at)}. Le laser peut être armé (Espace ou bouton).`;
  const ul = $('clItems');
  ul.innerHTML = '';
  for (const it of c.items.filter(i => i.visible)) {
    const li = document.createElement('li');
    li.className = it.done ? 'done' : '';
    li.dataset.item = it.id;
    const label = document.createElement('label'); label.className = 'check';
    const box = document.createElement('input'); box.type = 'checkbox'; box.checked = it.done; box.disabled = c.complete;
    const txt = document.createElement('span'); txt.textContent = it.text_fr;
    label.append(box, txt);
    if (it.when !== 'always') { const w = document.createElement('span'); w.className = 'when'; w.textContent = CL_WHEN[it.when]; label.append(w); }
    if (it.only_for === 'shownet') { const o = document.createElement('span'); o.className = 'only'; o.textContent = 'ShowNET'; label.append(o); }
    li.append(label);
    let field = null;
    if (it.needs_value) {
      field = document.createElement('input'); field.type = 'text'; field.maxLength = 200;
      field.placeholder = it.value_label_fr || 'Valeur'; field.value = it.value ?? ''; field.disabled = c.complete || it.done;
      li.append(field);
    }
    if (it.done) { const at = document.createElement('div'); at.className = 'at'; at.textContent = 'Coché à ' + clock(it.at); li.append(at); }
    box.onchange = () => clPost('check', { id: it.id, on: box.checked, value: field ? field.value : null });
    ul.append(li);
  }
}
function sendOperator() {
  clearTimeout(clOpTimer); clOpTimer = null;
  return clPost('operator', { name: $('clOperator').value });
}
$('clOperator').addEventListener('input', () => { clearTimeout(clOpTimer); clOpTimer = setTimeout(sendOperator, 300); });
$('clOperator').addEventListener('change', sendOperator);
$('clComplete').onclick = async () => { await sendOperator(); clPost('complete'); };
$('clRestart').onclick = () => {
  if (armed && checklistState?.enforced && !confirm('Recommencer la liste désarme le laser. Continuer ?')) return;
  clPost('restart');
};
function clConfig(extra) {
  return { items: checklist.items.map(({ id, text_fr, when, needs_value, value_label_fr, only_for }) => ({ id, text_fr, when, needs_value, value_label_fr, only_for })),
    outdoor: checklist.outdoor, audience_scan: checklist.audience_scan, ...extra };
}
$('clOutdoor').onchange = () => clPost('config', clConfig({ outdoor: $('clOutdoor').checked }));
$('clScan').onchange = () => clPost('config', clConfig({ audience_scan: $('clScan').checked }));
function showClEditor() {
  $('clEditor').hidden = !clEditing; $('clRun').hidden = !!clEditing;
  const box = $('clEditRows'); box.innerHTML = '';
  (clEditing || []).forEach((it, i) => {
    const row = document.createElement('div'); row.className = 'cl-row';
    const ta = document.createElement('textarea'); ta.value = it.text_fr; ta.maxLength = 400; ta.oninput = () => { it.text_fr = ta.value; };
    const del = document.createElement('button'); del.textContent = 'Supprimer'; del.onclick = () => { clEditing.splice(i, 1); showClEditor(); };
    const opts = document.createElement('div'); opts.className = 'cl-opts';
    // The ShowNET items (T-472) stay: their wording only can change.
    if (it.only_for === 'shownet') {
      del.hidden = true;
      const o = document.createElement('span'); o.className = 'hint hint--inline';
      o.textContent = 'Sortie ShowNET uniquement — obligatoire';
      opts.append(o);
      row.append(ta, del, opts);
      box.append(row);
      return;
    }
    const sel = document.createElement('select');
    sel.innerHTML = Object.entries(CL_WHEN).map(([k, v]) => `<option value="${k}">${v}</option>`).join('');
    sel.value = it.when; sel.onchange = () => { it.when = sel.value; };
    const nv = document.createElement('label'); nv.className = 'check u-m0';
    const nvBox = document.createElement('input'); nvBox.type = 'checkbox'; nvBox.checked = it.needs_value;
    nv.append(nvBox, ' Valeur à saisir');
    const lbl = document.createElement('input'); lbl.type = 'text'; lbl.placeholder = 'Nom du champ'; lbl.value = it.value_label_fr; lbl.maxLength = 400;
    lbl.hidden = !it.needs_value; lbl.oninput = () => { it.value_label_fr = lbl.value; };
    nvBox.onchange = () => { it.needs_value = nvBox.checked; lbl.hidden = !nvBox.checked; };
    opts.append(sel, nv, lbl);
    row.append(ta, del, opts);
    box.append(row);
  });
}
$('clEdit').onclick = () => { clEditing = structuredClone(clConfig().items); showClEditor(); };
$('clCancel').onclick = () => { clEditing = null; showClEditor(); };
$('clAdd').onclick = () => { clEditing.push({ id: '', text_fr: '', when: 'always', needs_value: false, value_label_fr: '' }); showClEditor(); $('clEditRows').lastChild.querySelector('textarea').focus(); };
$('clDefaults').onclick = async () => {
  if (!confirm('Revenir à la liste par défaut ? Vos éléments personnalisés seront perdus.')) return;
  if (await clPost('config', { defaults: true })) { clEditing = null; showClEditor(); }
};
$('clSave').onclick = async () => { if (await clPost('config', clConfig({ items: clEditing }))) { clEditing = null; showClEditor(); } };

// ---------- RÉGLAGES › Lieu et projecteurs (T-277) ----------
// The venue lives on the server (GET/POST /api/venue). Each projector is
// an output stage of its own there; here: the form, the projector list of
// the caps panel and of the zones' « Sortie », and the 3D view's venue.
let venue = null, venueLinks = [], vnSel = 0, vnMax = 8, vnTimer = 0, vnDevices = [], vnFramesTimer = 0;
const VN_NUM = { vnWidth: ['room', 'width_m'], vnDepth: ['room', 'depth_m'], vnHeight: ['room', 'height_m'], vnAudH: ['audience', 'min_height_m'] };
const VN_PROJ = { vnPosX: p => [p.pos_m, 0], vnPosY: p => [p.pos_m, 1], vnPosZ: p => [p.pos_m, 2], vnYaw: p => [p, 'yaw_deg'], vnPitch: p => [p, 'pitch_deg'], vnRoll: p => [p, 'roll_deg'], vnScan: p => [p, 'scan_deg'] };
const VN_STATE = { connected: 'connectée', connecting: 'connexion…', lost: 'perdue', available: 'non connectée' };
async function reloadVenue() {
  try {
    const r = await api('GET', '/api/venue');
    if (r.ok) applyVenue(await r.json());
  } catch (e) { console.warn('/api/venue', e); }
}
function applyVenue(j) {
  venue = j.venue; venueLinks = j.links || []; vnMax = j.max_projectors || 8;
  takeGalvo(venue.render);
  const pre = $('vnPreset');
  if (pre.options.length <= 1) for (const p of j.presets || []) pre.add(new Option(`${p.label_fr} (${p.depth_m} × ${p.width_m}${p.ceiling ? ' × ' + p.height_m : ''} m)`, p.id));
  if (vnSel >= venue.projectors.length) vnSel = 0;
  showVenue(); vnTargets(); venueTo3d(); pollFrames3d(); showOutdoorSettings();
}
function vnProj() { return venue && venue.projectors[vnSel]; }
function vnSet(id, v) { if (document.activeElement !== $(id)) $(id).value = v; }
function showVenue() {
  if (!venue) return;
  $('vnPreset').value = venue.preset || '';
  for (const [id, [a, k]] of Object.entries(VN_NUM)) vnSet(id, venue[a][k]);
  $('vnCeiling').checked = venue.room.ceiling;
  venue.audience.rect_m.forEach((v, i) => vnSet('vnAud' + i, v));
  $('vnScreens').textContent = venue.screens.length;
  $('vnList').replaceChildren(...venue.projectors.map((p, i) => {
    const b = document.createElement('button');
    b.textContent = p.name; b.setAttribute('role', 'tab'); b.dataset.projector = p.id;
    b.classList.toggle('active', i === vnSel); b.classList.toggle('main', i === 0);
    b.title = i === 0 ? 'Projecteur principal : la sortie choisie dans « Sorties et projecteurs »' : p.id;
    b.onclick = () => { vnSel = i; showVenue(); venueTo3d(); };
    return b;
  }));
  const p = vnProj(), link = venueLinks.find(l => l.id === p.id) || {};
  vnSet('vnName', p.name);
  for (const [id, f] of Object.entries(VN_PROJ)) { const [o, k] = f(p); vnSet(id, o[k]); }
  vnSet('vnBright', Math.round(p.brightness * 100));
  $('vnMirror').checked = p.mirror_x;
  const sel = $('vnOutput');
  if (vnSel === 0) {
    sel.replaceChildren(new Option(link.output_name || link.output || 'Aucune (aperçu seulement)', ''));
    sel.disabled = true;
    sel.title = 'Le projecteur principal utilise la sortie choisie dans « Sorties et projecteurs »';
  } else {
    const main = venueLinks[0] && venueLinks[0].output, used = venue.projectors.filter((q, i) => i > 0 && i !== vnSel && q.output).map(q => q.output);
    const opts = [new Option('Aucune (aperçu seulement)', '')];
    for (const d of vnDevices) if (d.id !== main && !used.includes(d.id)) opts.push(new Option(`${d.name} (${d.id})`, d.id));
    if (p.output && !opts.some(o => o.value === p.output)) opts.push(new Option(p.output, p.output));
    sel.replaceChildren(...opts); sel.value = p.output || ''; sel.disabled = false; sel.title = '';
  }
  showVnOutState();
  $('vnDel').disabled = vnSel === 0;
  $('vnAdd').disabled = $('vnDup').disabled = venue.projectors.length >= vnMax;
}
// The selected projector's output line: refreshed on its own while the
// form has the focus (T-474), so « connexion… » never stays after the link
// has connected or been lost.
function showVnOutState() {
  const p = vnProj();
  if (!p) return;
  const link = venueLinks.find(l => l.id === p.id) || {};
  const st = $('vnOutState');
  const state = link.state, ok = state === 'connected' && (vnSel === 0 || link.output_name);
  st.textContent = !(vnSel === 0 ? link.output : p.output) ? 'Aperçu seulement : ce projecteur ne pilote aucune sortie.' + (vnSel > 0 && !vnDevices.length ? ' « Rechercher » (Sorties et projecteurs) liste les sorties disponibles.' : '')
    : `Sortie ${VN_STATE[state] || '…'}${link.output_name ? ' : ' + link.output_name : ''}${link.error ? ' — ' + link.error : ''}`;
  st.className = 'vn-out' + (ok ? ' ok' : state === 'lost' ? ' bad' : '');
}
// The caps panel and the zones' « Sortie »: one entry per projector.
function vnTargets() {
  const t = $('pwTarget'), keep = t.value || 'main';
  t.replaceChildren(...venue.projectors.map(p => new Option(p.name, p.id)));
  t.value = venue.projectors.some(p => p.id === keep) ? keep : 'main';
  $('pwTargetRow').hidden = venue.projectors.length <= 1;
  if (t.value !== keep) { reloadOutputs(); reloadOutColor(); }
  const z = $('pzOutput');
  for (const o of [...z.options]) if (o.dataset.projector) o.remove();
  venue.projectors.forEach((p, i) => {
    if (i === 0 && venue.projectors.length <= 1) return;
    const o = new Option('Projecteur : ' + p.name, p.id); o.dataset.projector = '1'; z.add(o);
  });
  if (typeof showProjection === 'function' && proj) showProjection();
}
// The 3D view: the room and every projector; the other projectors' frames
// come from /api/frames while it is shown (the main one's is the preview's).
function venueTo3d() {
  if (!view3d || !venue) return;
  view3d.setVenue({
    room: { w: venue.room.width_m, d: venue.room.depth_m, h: venue.room.height_m },
    projectors: venue.projectors.map(p => ({ pos: p.pos_m, yaw: p.yaw_deg, pitch: p.pitch_deg, roll: p.roll_deg, scanDeg: p.scan_deg, mirrorX: p.mirror_x, brightness: p.brightness })),
    selected: venue.projectors.length > 1 && $('venuePanel').open ? vnSel : -1,
    audience: venue.audience,
  });
  // The saved viewpoint, once: later reloads of the venue keep the camera.
  if (!cam3dRestored) { assignCam3dRestored(true); view3d.restoreCamera(venue.camera); showCam3d(); }
  outdoorTo3d();
}
async function pollFrames3d() {
  clearTimeout(vnFramesTimer);
  if (!show3d || !view3d || !venue || venue.projectors.length <= 1) return;
  try {
    const j = await apiJson('GET', '/api/frames', undefined, { cache: 'no-store' });
    // T-280: the galvo simulation at the main output's point rate (display only).
    j.frames.forEach((f, i) => {
      if (i === 0) return;
      const shown = GALVO.enabled ? galvoFilter(f.points, galvoPps(), GALVO.response_ms) : f.points;
      view3d.setProjectorFrame(i, shown, shown === f.points ? null : f.points);
    });
  } catch { /* next time */ }
  vnFramesTimer = setTimeout(pollFrames3d, 100);
}
async function saveVenue(body = venue, path = '/api/venue') {
  $('vnError').textContent = '';
  const r = await api('POST', path, body, { quiet: true });
  if (r.ok) { applyVenue(await r.json()); pollFrames3d(); return true; }
  const msg = r.status === 409 && (r.headers.get('content-type') || '').includes('json') ? (await r.json()).error : await r.text();
  await reloadVenue();
  $('vnError').textContent = msg;
  return false;
}
function vnChanged() { showVenue(); venueTo3d(); clearTimeout(vnTimer); vnTimer = setTimeout(() => { vnTimer = 0; saveVenue(); }, 200); }
const vnNum = (id, lo, hi) => { const v = +$(id).value; return Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : null; };
for (const [id, [a, k]] of Object.entries(VN_NUM)) $(id).addEventListener('change', () => { const v = vnNum(id, 0, 500); if (v !== null) { venue[a][k] = v; venue.preset = ''; vnChanged(); } });
[0, 1, 2, 3].forEach(i => $('vnAud' + i).addEventListener('change', () => { const v = vnNum('vnAud' + i, -500, 500); if (v !== null) { venue.audience.rect_m[i] = v; vnChanged(); } }));
$('vnCeiling').addEventListener('change', () => { venue.room.ceiling = $('vnCeiling').checked; venue.preset = ''; vnChanged(); });
$('vnPreset').addEventListener('change', () => { if ($('vnPreset').value) saveVenue({ name: $('vnPreset').value }, '/api/venue/preset'); });
$('vnName').addEventListener('change', () => { vnProj().name = $('vnName').value; vnChanged(); });
for (const [id, f] of Object.entries(VN_PROJ)) $(id).addEventListener('change', () => { const v = vnNum(id, -500, 500); if (v !== null) { const [o, k] = f(vnProj()); o[k] = v; vnChanged(); } });
$('vnBright').addEventListener('change', () => { const v = vnNum('vnBright', 0, 100); if (v !== null) { vnProj().brightness = v / 100; vnChanged(); } });
$('vnMirror').addEventListener('change', () => { vnProj().mirror_x = $('vnMirror').checked; vnChanged(); });
$('vnOutput').addEventListener('change', () => { vnProj().output = $('vnOutput').value || null; clearTimeout(vnTimer); saveVenue(); });
function vnNewId() { for (let n = 2; ; n++) if (!venue.projectors.some(p => p.id === 'p' + n)) return 'p' + n; }
$('vnAdd').onclick = () => {
  const p = { ...venue.projectors[0], id: vnNewId(), name: 'Projecteur ' + (venue.projectors.length + 1), output: null, mirror_x: false, pos_m: [...venue.projectors[0].pos_m] };
  venue.projectors.push(p); vnSel = venue.projectors.length - 1; clearTimeout(vnTimer); saveVenue();
};
$('vnDup').onclick = () => {
  const src = vnProj(), p = { ...src, id: vnNewId(), name: (src.name + ' (copie)').slice(0, 40), output: null, pos_m: [...src.pos_m] };
  // A copy faces the other way across the room (Miroir X): symmetric rigs.
  p.pos_m[0] = -p.pos_m[0]; p.yaw_deg = -p.yaw_deg; p.mirror_x = !src.mirror_x;
  venue.projectors.push(p); vnSel = venue.projectors.length - 1; clearTimeout(vnTimer); saveVenue();
};
$('vnDel').onclick = () => {
  if (vnSel === 0) return;
  venue.projectors.splice(vnSel, 1); vnSel = Math.max(0, vnSel - 1); clearTimeout(vnTimer); saveVenue();
};
$('pwTarget').addEventListener('change', () => { reloadOutputs(); reloadOutColor(); });
// While the panel is open, the projectors' outputs follow connections and
// losses (never over an edit still to be sent). While a field of the panel
// has the focus (a button just clicked, a select just chosen), only the
// output lines follow: the form is left alone (T-474).
async function reloadVenueLinks() {
  try {
    const j = await apiJson('GET', '/api/venue', undefined, { quiet: true });
    if (!venue || vnTimer) return;
    venueLinks = j.links || [];
    showVnOutState();
  } catch { /* next time */ }
}
setInterval(() => {
  if (activeWs !== 'settings' || !$('venuePanel').open || vnTimer) return;
  if ($('venuePanel').contains(document.activeElement)) reloadVenueLinks(); else reloadVenue();
}, 1000);
// The output list when the panel opens (no scan: what the studio knows).
$('venuePanel').addEventListener('toggle', async () => {
  venueTo3d();
  if (!$('venuePanel').open) return;
  try { vnDevices = (await apiJson('GET', '/api/outputs')).devices || []; } catch { /* keep the list */ }
  reloadVenue();
});

// ---------- safety profiles per venue (T-260) ----------
// The active profile is a label over the settings in force. Loading one
// disarms; what it would loosen asks for a confirmation (409 → sfpLoosen).
let spView = null, spPending = null;
function showProfilePill(p) {
  const el = $('profilePill');
  if (p === undefined) return;
  if (!p || !p.name) { el.hidden = true; return; }
  el.hidden = false;
  // « Profil : » in its own span: a narrow top bar drops it (T-517).
  const text = 'Profil : ' + p.name + (p.modified ? ' (modifié)' : '');
  if (el.textContent !== text) {
    const pfx = document.createElement('span');
    pfx.className = 'pfx';
    pfx.textContent = 'Profil : ';
    el.replaceChildren(pfx, text.slice(pfx.textContent.length));
  }
  el.classList.toggle('warn', !!(p.modified || p.stale));
  el.title = el.textContent + ' — profil de sécurité actif' + (p.stale ? ' — vérification sur place à refaire' : '') + ' (RÉGLAGES › Sécurité)';
}
async function reloadProfiles() {
  try {
    const r = await api('GET', '/api/safety/profiles', undefined, { quiet: true });
    if (r.ok) showProfiles(await r.json());
  } catch { /* server gone */ }
}
function showProfiles(v) {
  spView = v;
  const sel = $('sfpSelect'), keep = sel.value;
  sel.replaceChildren(...v.profiles.map(p => {
    const o = document.createElement('option');
    o.value = p.name;
    o.textContent = p.name + (p.builtin ? ' (intégré)' : '') + (p.damaged ? ' — abîmé' : '') + (p.name === v.active ? ' ✓' : '');
    o.disabled = !!p.damaged;
    return o;
  }));
  sel.value = v.profiles.some(p => p.name === keep) ? keep : (v.active || v.builtin);
  showProfileInfo();
}
function showProfileInfo() {
  const v = spView;
  if (!v) return;
  const p = v.profiles.find(x => x.name === $('sfpSelect').value);
  const a = v.active_profile;
  let info = a ? `Actif : ${a.name}${a.venue ? ' — ' + a.venue : ''}${v.modified ? ' (réglages modifiés depuis, à enregistrer)' : ''}.` : 'Aucun profil actif.';
  if (a) info += a.audience_scan_allowed ? ' Balayage public autorisé dans ce lieu.' : ' Balayage public interdit dans ce lieu.';
  if (v.load_error) info += ' ' + v.load_error;
  $('sfpInfo').textContent = info;
  const stale = a && a.stale;
  $('sfpStale').hidden = !stale;
  $('sfpStale').textContent = !stale ? '' : a.verified_on
    ? `⚠ Dernière vérification sur place le ${a.verified_on} (plus de ${v.verify_max_days} jours) : revérifiez le lieu.`
    : '⚠ Ce profil n\'a jamais été vérifié sur place.';
  const builtin = !p || p.builtin;
  $('sfpVerify').disabled = builtin;
  $('sfpDelete').disabled = builtin;
  $('sfpLoad').disabled = !p || !!p.damaged;
}
$('sfpSelect').addEventListener('change', () => { spPending = null; $('sfpLoosen').hidden = true; showProfileInfo(); });
$('safetyPanel').addEventListener('toggle', () => { if ($('safetyPanel').open) reloadProfiles(); });
/** Answers of the profile routes: 409 → the loosening confirmation. */
async function profileReply(r, retry) {
  $('sfpError').textContent = '';
  if (r.ok) {
    spPending = null; $('sfpLoosen').hidden = true;
    const v = await r.json();
    showProfiles(v);
    showProfilePill({ name: v.active, modified: v.modified, stale: !!(v.active_profile && v.active_profile.stale) });
    reloadSafety(); reloadOutputs(); reloadChecklist(); reloadAudience();
    return true;
  }
  if (r.status === 409) {
    const j = await r.json();
    spPending = retry;
    $('sfpLoosenText').textContent = 'Ce profil assouplit la sécurité : ' + j.loosen.join(' · ');
    $('sfpLoosen').hidden = false;
  } else if (r.status === 423) {
    $('sfpError').textContent = 'Verrouillé en mode spectacle';
  } else {
    $('sfpError').textContent = await r.text();
    reloadProfiles();
  }
  return false;
}
async function loadProfile(name, confirm = false) {
  const r = await api('POST', '/api/safety/profiles/load', { name, confirm_loosen: confirm, source: 'ui' }, { quiet: true });
  await profileReply(r, () => loadProfile(name, true));
}
async function saveProfile(body, confirm = false) {
  const r = await api('POST', '/api/safety/profiles', { ...body, action: 'save', confirm_loosen: confirm, source: 'ui' }, { quiet: true });
  if (await profileReply(r, () => saveProfile(body, true))) $('sfpSaveBox').hidden = true;
}
$('sfpLoad').addEventListener('click', () => loadProfile($('sfpSelect').value));
$('sfpLoosenOk').addEventListener('click', () => { const f = spPending; spPending = null; if (f) f(); });
$('sfpLoosenCancel').addEventListener('click', () => { spPending = null; $('sfpLoosen').hidden = true; });
$('sfpSaveAs').addEventListener('click', () => {
  const a = spView && spView.active_profile;
  const mine = a && !a.builtin;
  $('sfpName').value = mine ? a.name : '';
  $('sfpVenue').value = mine ? a.venue : '';
  $('sfpNotes').value = mine ? a.notes : '';
  $('sfpAudience').checked = !!(mine && a.audience_scan_allowed);
  $('sfpSaveBox').hidden = false;
  $('sfpName').focus();
});
$('sfpSaveCancel').addEventListener('click', () => { $('sfpSaveBox').hidden = true; });
$('sfpSaveOk').addEventListener('click', () => saveProfile({
  name: $('sfpName').value.trim(), venue: $('sfpVenue').value, notes: $('sfpNotes').value, audience_scan_allowed: $('sfpAudience').checked,
}));
$('sfpVerify').addEventListener('click', async () => {
  const r = await api('POST', '/api/safety/profiles', { action: 'verify', name: $('sfpSelect').value, source: 'ui' }, { quiet: true });
  await profileReply(r, null);
});
$('sfpDelete').addEventListener('click', async () => {
  const name = $('sfpSelect').value;
  if (!confirm(`Supprimer le profil « ${name} » ? Les réglages en vigueur ne changent pas.`)) return;
  const r = await api('POST', '/api/safety/profiles', { action: 'delete', name, source: 'ui' }, { quiet: true });
  await profileReply(r, null);
});

// ---------- boot ----------
async function loadState() {
  const st = await apiJson('GET', '/api/state');
  showDataWarnings(st.data_warnings);
  assignSafety(st.safety); showSafety();
  reloadSafety();
  reloadOutputs();
  reloadOutColor();
  reloadDevices();
  reloadVenue();
  loadArtnet();
  loadAudioOut();
  loadDmxOut();
  assignCalibration(st.calibration); assignArmed(st.armed); assignArm(st.arm); assignScenes(st.scenes);
  showShowMode(st.lock);
  assignChecklistState(st.checklist); showShownet(st.shownet); reloadChecklist();
  showAudienceMode(st.audience); reloadAudience();
  showProfilePill(st.safety_profile); reloadProfiles();
  if (!$('gen').children.length) {
    $('gen').innerHTML = st.generators.map(g => `<option value="${g}">${GEN_NAMES[g] || g}</option>`).join('');
  }
  if (!$('shapes').children.length) {
    const names = { circle: 'Cercle', square: 'Carré', triangle: 'Triangle', cross: 'Croix', line: 'Ligne', star: 'Étoile', spiral: 'Spirale', dots: 'Points' };
    st.shapes.forEach(name => {
      const b = document.createElement('button');
      b.textContent = names[name] || name; b.dataset.shape = name;
      b.onclick = () => { editLook(s => { s.content = { kind: 'shape', shape: name }; }); showSettings(); };
      $('shapes').appendChild(b);
    });
  }
  takeLook(st); showCalibration(); showArmed(); renderScenes(); showMusSafety();
}

// Tooltips show each control's stable id, to help with MIDI mapping.
for (const [el, id] of [['scale', 'look.size'], ['bright', 'look.brightness'], ['rot', 'look.rotation_speed'],
  ['aOn', 'audio.enabled'], ['aSize', 'audio.size'], ['aRot', 'audio.rotate'], ['aFlash', 'audio.flash'],
  ['aColor', 'audio.color_on_beat']]) $(el).title = $(el).dataset.control = id;
$('armBtn').title += ' · transport.arm';

export { checklist, loadState, openChecklist, pollFrames3d, reloadChecklist, reloadVenue,
  showProfilePill, venue, venueTo3d };
