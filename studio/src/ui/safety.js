// Laser Studio — safety.js (T-435).
// Sections (as in the former index.html script):
//   safety (T-101, T-003)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './zones.js';
import { $, api, apiJson, sendLatest } from './core.js';
import { draw } from './presence.js';
import { lastPoints, lastShown, outdoorTo3d, safetyTo3d, show2d } from './view3d.js';
import { activeWs, opTouched } from './workspaces.js';
import { fmtDb, musNat, showHealth } from './music.js';
import { renderZoneTraceMenu } from './cuelist.js';
import { reloadAudience } from './outputs.js';
import { venue } from './venue.js';
import { openHelp } from './help.js';

// ---------- safety (T-101, T-003) ----------
// `safety` is what the server applies (the overlay draws it); the controls
// may hold an edit waiting for the loosening confirmation.
let safety = null, safetyTimer = null, sfLoadError = null, sfDrawing = null;
function assignSafety(v) { return (safety = v); }
// Placing a gradient end on the 2D preview (T-446): { index, end: "from" | "to" }.
let sfGradPick = null;
const pct = v => Math.round(v * 100) + ' %';
const sfFields = {
  sfHz: [s => s.strobe_max_hz, (s, v) => { s.strobe_max_hz = v; }, v => v + ' Hz'],
  sfBurst: [s => s.strobe_burst_s, (s, v) => { s.strobe_burst_s = v; }, v => v + ' s'],
  sfFloor: [s => s.horizon.y, (s, v) => { s.horizon.y = v; }, v => (+v).toFixed(2)],
  sfRamp: [s => s.horizon.ramp, (s, v) => { s.horizon.ramp = v; }, v => (+v).toFixed(2)],
  sfLevel: [s => s.horizon.level, (s, v) => { s.horizon.level = v; }, pct],
  sfGainR: [s => s.color_gain[0], (s, v) => { s.color_gain[0] = v; }, pct],
  sfGainG: [s => s.color_gain[1], (s, v) => { s.color_gain[1] = v; }, pct],
  sfGainB: [s => s.color_gain[2], (s, v) => { s.color_gain[2] = v; }, pct],
  sfMin: [s => s.min_diode_level, (s, v) => { s.min_diode_level = v; }, pct],
  // Static beam guard (T-256): the sliders are in percent.
  sfDwellMin: [s => Math.round(s.dwell.min_extent * 100), (s, v) => { s.dwell.min_extent = v / 100; }, v => v + ' %'],
  sfDwellCap: [s => Math.round(s.dwell.max_cell_dose * 100), (s, v) => { s.dwell.max_cell_dose = v / 100; }, v => v + ' %'],
  sfDwellBeams: [s => Math.round(s.dwell.max_cell_dose_beams * 100), (s, v) => { s.dwell.max_cell_dose_beams = v / 100; }, v => v + ' %'],
};
const fmtPts = pts => pts.map(p => `${+p[0].toFixed(3)} ${+p[1].toFixed(3)}`).join(' ; ');
function parsePts(text) {
  const pts = text.split(';').map(t => t.trim()).filter(Boolean).map(t => t.split(/[\s,]+/).map(Number));
  return pts.every(p => p.length === 2 && p.every(Number.isFinite)) ? pts : null;
}
function showSafety(force = false) {
  if (!safety) return;
  for (const [id, [get, , fmt]] of Object.entries(sfFields)) {
    if (force || document.activeElement !== $(id)) $(id).value = get(safety);
    $(id + 'V').textContent = fmt(get(safety));
  }
  $('sfLines').checked = safety.horizon.lines;
  safetyTo3d();
  if (force || document.activeElement !== $('sfDwellProfile')) $('sfDwellProfile').value = safety.dwell.profile;
  $('sfError').textContent = sfLoadError || '';
  $('sfZoneAdd').disabled = safety.zones.length >= 8;
  if (document.activeElement !== $('ztOnly')) renderZoneTraceMenu();
  if (force || !$('sfZones').contains(document.activeElement)) renderZones();
  showOutdoorSettings(force);
}
function renderZones() {
  $('sfZones').innerHTML = safety.zones.map((z, i) => `
    <div class="sfzone" data-zi="${i}">
      <div class="row">
        <input type="text" class="zname" maxlength="40" title="Nom de la zone">
        <select class="zkind" title="Masquer : rien d'allumé dedans · Atténuer : luminosité réduite">
          <option value="blank">Masquer</option><option value="dim">Atténuer</option></select>
        <input type="number" class="zlevel" min="0" max="100" step="5" title="Luminosité gardée dans la zone (%)">
        <label class="check zaudl" title="Zone Public : là où se trouve le public. Hors balayage public, rien n'y est allumé."><input type="checkbox" class="zaud"> Public</label>
      </div>
      <div class="row">
        <input type="text" class="zpts" title="Sommets : x y ; x y ; … (de -1 à 1, y vers le haut)">
        <button class="zdraw">${sfDrawing && sfDrawing.index === i ? 'Terminer' : 'Dessiner'}</button>
        <button class="zdel">Supprimer</button>
      </div>
      <div class="zsoft">
        <label>Bord doux <span class="v zfeatherV"></span></label><input type="range" class="zfeather" data-default="0" min="0" max="25" step="1" title="Largeur du bord doux vers l'extérieur (% de la largeur du champ)">
        <div class="row">
          <label class="check u-m0"><input type="checkbox" class="zgrad"> Dégradé</label>
          <span class="zgradbox">
            <label for="zlevelto${i}">Luminosité à l'arrivée (%)</label>
            <input type="number" class="zlevelto" id="zlevelto${i}" min="0" max="100" step="5">
          </span>
        </div>
        <div class="row zgradbox">
          <button class="zgpick" data-end="from" title="Puis cliquez sur l'aperçu 2D">${sfGradPick && sfGradPick.index === i && sfGradPick.end === 'from' ? 'Cliquez l’aperçu…' : 'Départ'}</button>
          <button class="zgpick" data-end="to" title="Puis cliquez sur l'aperçu 2D">${sfGradPick && sfGradPick.index === i && sfGradPick.end === 'to' ? 'Cliquez l’aperçu…' : 'Arrivée'}</button>
          <button class="zgtb" title="Départ en haut de la zone, arrivée en bas">Haut → bas</button>
        </div>
      </div>
    </div>`).join('');
  $('sfZones').querySelectorAll('.sfzone').forEach(row => {
    const z = safety.zones[+row.dataset.zi];
    row.querySelector('.zname').value = z.name;
    row.querySelector('.zkind').value = z.kind;
    row.querySelector('.zlevel').value = Math.round(z.level * 100);
    row.querySelector('.zlevel').hidden = z.kind !== 'dim';
    row.querySelector('.zaud').checked = !!z.audience;
    row.querySelector('.zpts').value = fmtPts(z.points);
    // Soft edge and gradient (T-446): Atténuer zones only.
    row.querySelector('.zsoft').hidden = z.kind !== 'dim';
    const fp = Math.round((z.feather || 0) * 50);
    row.querySelector('.zfeather').value = fp;
    row.querySelector('.zfeatherV').textContent = fp + ' %';
    row.querySelector('.zgrad').checked = !!z.gradient;
    row.querySelectorAll('.zgradbox').forEach(el => { el.hidden = !z.gradient; });
    row.querySelector('.zlevelto').value = Math.round((z.gradient ? z.gradient.level_to : 0.2) * 100);
  });
}
/** A gradient from the top to the bottom of a zone's outline (T-446). */
function zoneTopDown(z, levelTo) {
  const xs = z.points.map(p => p[0]), ys = z.points.map(p => p[1]);
  const cx = Math.round((Math.min(...xs) + Math.max(...xs)) * 50) / 100;
  return { from: [cx, Math.max(...ys)], to: [cx, Math.min(...ys)], level_to: levelTo };
}
/** The settings as the controls show them (strobe, horizon, colours, zones). */
function sfFromControls() {
  const next = structuredClone(safety);
  for (const [id, [, set]] of Object.entries(sfFields)) set(next, +$(id).value);
  next.horizon.lines = $('sfLines').checked;
  next.dwell.profile = $('sfDwellProfile').value;
  $('sfZones').querySelectorAll('.sfzone').forEach(row => {
    const z = next.zones[+row.dataset.zi];
    z.name = row.querySelector('.zname').value;
    z.kind = row.querySelector('.zkind').value;
    z.level = Math.min(100, Math.max(0, +row.querySelector('.zlevel').value || 0)) / 100;
    z.audience = row.querySelector('.zaud').checked;
    // A Public zone keeps 50 % at most (T-255).
    if (z.audience) z.level = Math.min(z.level, 0.5);
    const pts = parsePts(row.querySelector('.zpts').value);
    if (pts) z.points = pts;
    // Soft edge: % of the field width (2 units), so 25 % = 0.5 (T-446).
    z.feather = Math.min(25, Math.max(0, +row.querySelector('.zfeather').value || 0)) / 50;
    if (row.querySelector('.zgrad').checked) {
      let levelTo = Math.min(100, Math.max(0, +row.querySelector('.zlevelto').value || 0)) / 100;
      if (z.audience) levelTo = Math.min(levelTo, 0.5);
      z.gradient = z.gradient ? { ...z.gradient, level_to: levelTo } : zoneTopDown(z, levelTo);
    } else z.gradient = null;
  });
  return next;
}
// RÉGLAGES › Journal de sécurité (T-259): one day of the log, filtered by
// type, newest first; refreshed every 2 s while the panel is open.
const SL_GROUPS = {
  arm: ['arm', 'arm_refused', 'disarm'],
  estop: ['estop', 'estop_reset'],
  locks: ['interlock', 'presence', 'watchdog', 'checklist', 'checklist_reset', 'checklist_edit', 'show_mode', 'safety_lock', 'input_lost', 'input_back'],
  limiter: ['limiter'],
  play: ['cue', 'scene', 'show'],
  settings: ['safety_settings', 'presence_settings', 'midi_safety', 'power_caps', 'netin_settings', 'audience_mode', 'safety_profile'],
  studio: ['app_start', 'app_stop', 'project_open', 'autosave_recovered', 'log_overflow', 'output_select', 'output'],
};
let slLog = null;
// Exposure estimator (T-257): read-only figures, never an action on the output.
const exFmt = (v, digits, unit) => v == null ? '—' : v.toLocaleString('fr-FR', { maximumFractionDigits: digits }) + unit;
function showExposure(j) {
  $('exBanner').textContent = j.disclaimer;
  $('exProjector').textContent = (j.projector_name || 'projecteur sans nom') + ', plafond ' + Math.round(j.max_power * 100) + ' %';
  if (document.activeElement !== $('exDistance')) $('exDistance').value = j.settings.distance_m;
  if (document.activeElement !== $('exAttenuation')) $('exAttenuation').value = j.settings.attenuation_pct;
  const r = j.result;
  const over = r ? r.ratio > 1 : null;
  $('exVerdict').dataset.over = over == null ? '' : String(over);
  $('exVerdict').textContent = r == null ? 'Pas d\'estimation : fiche projecteur incomplète.'
    : over ? 'Estimation : au-dessus de l\'EMP (× ' + exFmt(r.ratio, 1, '') + ') à ' + exFmt(j.settings.distance_m, 1, ' m')
      : 'Estimation : sous l\'EMP à ' + exFmt(j.settings.distance_m, 1, ' m') + ' — à confirmer par une mesure, ce n\'est pas une garantie';
  $('exPower').textContent = exFmt(j.power_capped_mw, 0, ' mW') + ' (fiche : ' + exFmt(j.power_full_mw, 0, ' mW') + ')';
  $('exBeam').textContent = r ? exFmt(r.beam_mm, 1, ' mm') : '—';
  $('exIrr').textContent = r ? exFmt(r.irradiance_mw_cm2, 2, ' mW/cm²') : '—';
  $('exMpe').textContent = r ? exFmt(r.mpe_mw_cm2, 2, ' mW/cm²') : '—';
  $('exRatio').textContent = r ? '× ' + exFmt(r.ratio, 1, '') : '—';
  const nohd = (res, d) => res == null ? '—' : d == null ? 'illimitée (divergence inconnue)' : exFmt(d, 1, ' m');
  $('exNohd').textContent = nohd(r, r && r.nohd_m);
  $('exRatioFull').textContent = j.ratio_full == null ? '—' : '× ' + exFmt(j.ratio_full, 1, '');
  $('exNohdFull').textContent = nohd(j.ratio_full, j.nohd_full_m);
  $('exIldaStatic').textContent = j.ratio_ilda_static == null ? '—' : '× ' + exFmt(j.ratio_ilda_static, 1, '');
  $('exIldaScan').textContent = j.ratio_ilda_scanned == null ? '—' : '× ' + exFmt(j.ratio_ilda_scanned, 1, '');
  $('exSuggest').textContent = j.suggested_max_power == null ? ''
    : 'À titre indicatif, un plafond de puissance d\'environ ' + exFmt(j.suggested_max_power * 100, 2, ' %') + ' ramènerait cette estimation à l\'EMP (à régler vous-même dans RÉGLAGES › Sorties et projecteurs, puis à mesurer).';
  $('exWarnings').textContent = j.warnings.join(' ');
  $('exAssumptions').replaceChildren(...j.assumptions.map(a => { const li = document.createElement('li'); li.textContent = a; return li; }));
}
async function loadExposure() {
  let r;
  try { r = await api('GET', '/api/safety/exposure', undefined, { quiet: true }); } catch { return; }
  if (r.ok) { $('exError').textContent = ''; showExposure(await r.json()); }
}
async function saveExposure() {
  const body = { exposure: { distance_m: +$('exDistance').value, attenuation_pct: +$('exAttenuation').value } };
  let r;
  try { r = await api('POST', '/api/safety/exposure', body, { quiet: true }); } catch { return; }
  if (r.ok) { $('exError').textContent = ''; showExposure(await r.json()); }
  else { const msg = await r.text(); await loadExposure(); $('exError').textContent = msg; }
}
['exDistance', 'exAttenuation'].forEach(id => $(id).addEventListener('change', saveExposure));
$('exposurePanel').addEventListener('toggle', () => { if ($('exposurePanel').open) loadExposure(); });
setInterval(() => { if (activeWs === 'settings' && $('exposurePanel').open && !document.hidden && !$('exposurePanel').contains(document.activeElement)) loadExposure(); }, 3000);
async function loadSafetyLog() {
  const day = $('slDay').value;
  let j;
  try {
    const r = await api('GET', '/api/safety/log' + (day ? '?date=' + encodeURIComponent(day) : ''), undefined, { quiet: true });
    if (!r.ok) { $('slError').textContent = await r.text(); return; }
    j = await r.json();
  } catch { return; }
  slLog = j;
  const days = j.days.includes(j.today) ? j.days : [j.today, ...j.days];
  if ($('slDay').dataset.days !== days.join()) {
    $('slDay').dataset.days = days.join();
    $('slDay').replaceChildren(...days.map(d => {
      const o = document.createElement('option');
      o.value = d; o.textContent = d === j.today ? `Aujourd'hui (${d})` : d;
      return o;
    }));
  }
  $('slDay').value = j.date;
  const st = j.status || {};
  $('slError').textContent = st.last_error ? `Écriture du journal impossible : ${st.last_error}` : (st.dropped ? `${st.dropped} événement(s) perdu(s) (journal saturé).` : '');
  showSafetyLog();
}
// T-482: the newest rows only, and rebuilt only when the log changed. A
// show day logs thousands of events; with the panel open, rebuilding them
// all every 2 s grew the page and froze it for seconds. The CSV has all.
const SL_MAX_ROWS = 300;
function showSafetyLog() {
  if (!slLog) return;
  const group = SL_GROUPS[$('slKind').value];
  const last = slLog.events[slLog.events.length - 1];
  const key = [slLog.date, slLog.events.length, last ? last.ts : '', $('slKind').value].join('|');
  if ($('slRows').dataset.key === key) return;
  $('slRows').dataset.key = key;
  const all = slLog.events.filter(e => !group || group.includes(e.kind)).reverse();
  const rows = all.slice(0, SL_MAX_ROWS);
  $('slRows').replaceChildren(...rows.map(e => {
    const tr = document.createElement('tr');
    tr.className = 'k-' + e.kind;
    tr.dataset.kind = e.kind;
    tr.title = JSON.stringify(e.detail);
    for (const [cls, text] of [['t', e.ts.slice(11, 19)], ['e', e.text], ['s', e.source_fr]]) {
      const td = document.createElement('td');
      td.className = cls; td.textContent = text;
      tr.append(td);
    }
    return tr;
  }));
  if (all.length > rows.length) {
    const tr = document.createElement('tr'), td = document.createElement('td');
    tr.id = 'slMore';
    td.colSpan = 3; td.className = 't';
    td.textContent = `… et ${all.length - rows.length} événement(s) plus ancien(s) ce jour-là : « Exporter CSV » les contient tous.`;
    tr.append(td); $('slRows').append(tr);
  }
  if (!rows.length) {
    const tr = document.createElement('tr'), td = document.createElement('td');
    td.colSpan = 3; td.className = 't'; td.textContent = 'Aucun événement.';
    tr.append(td); $('slRows').replaceChildren(tr);
  }
}
$('safetyLogPanel').addEventListener('toggle', () => { if ($('safetyLogPanel').open) loadSafetyLog(); });
$('slDay').onchange = loadSafetyLog;
$('slKind').onchange = showSafetyLog;
$('slRefresh').onclick = loadSafetyLog;
$('slCsv').onclick = () => {
  const a = document.createElement('a');
  a.href = '/api/safety/log.csv?date=' + encodeURIComponent($('slDay').value || (slLog && slLog.date) || '');
  a.download = '';
  document.body.append(a); a.click(); a.remove();
};
setInterval(() => { if (activeWs === 'settings' && $('safetyLogPanel').open && !document.hidden) loadSafetyLog(); }, 2000);

// RÉGLAGES › Fiche sécurité (T-263): the event form and the printable
// sheet. The event fields (never the mobile number) are remembered in this
// browser only; the studio saves none of them.
const SS_KEY = 'laserStudio.safetySheet';
const SS_MAX_DATES = 10;
function ssAddDate(d = {}) {
  const box = $('ssDates');
  if (box.children.length >= SS_MAX_DATES) return;
  const n = box.children.length + 1;
  const row = document.createElement('div');
  row.className = 'ss-date';
  for (const [key, label, ph] of [['day', 'Date', 'jj/mm/aaaa'], ['start', 'Début', 'hh:mm'], ['end', 'Fin', 'hh:mm']]) {
    const input = document.createElement('input');
    input.type = 'text'; input.maxLength = 200; input.dataset.k = key; input.placeholder = ph;
    input.setAttribute('aria-label', `${label} (${n})`);
    input.value = d[key] || '';
    input.oninput = ssRemember;
    row.append(input);
  }
  box.append(row);
  $('ssAddDate').disabled = box.children.length >= SS_MAX_DATES;
}
function ssEvent() {
  const dates = [...$('ssDates').children].map(r => Object.fromEntries([...r.querySelectorAll('input')].map(i => [i.dataset.k, i.value.trim()])))
    .filter(d => d.day || d.start || d.end);
  return { name: $('ssName').value.trim(), venue: $('ssVenue').value.trim(), operator: $('ssOperator').value.trim(), phone: $('ssPhone').value.trim(), dates };
}
function ssRemember() {
  const { phone, ...kept } = ssEvent();
  try { localStorage.setItem(SS_KEY, JSON.stringify(kept)); } catch { /* storage unavailable: nothing kept */ } opTouched();
}
(function ssInit() {
  let kept = {};
  try { kept = JSON.parse(localStorage.getItem(SS_KEY) || '{}') || {}; } catch { kept = {}; }
  $('ssName').value = kept.name || ''; $('ssVenue').value = kept.venue || ''; $('ssOperator').value = kept.operator || '';
  const dates = Array.isArray(kept.dates) && kept.dates.length ? kept.dates.slice(0, SS_MAX_DATES) : [{}];
  dates.forEach(d => ssAddDate(d));
  for (const id of ['ssName', 'ssVenue', 'ssOperator']) $(id).oninput = ssRemember;
})();
$('ssAddDate').onclick = () => { ssAddDate(); ssRemember(); };
$('ssGenerate').onclick = async () => {
  $('ssError').textContent = '';
  // Opened inside the click so the browser does not block it.
  const w = window.open('', '_blank');
  let r;
  try {
    r = await api('POST', '/api/safety/sheet', { event: ssEvent(), include_log: $('ssLog').checked, include_notes: $('ssNotes').checked }, { quiet: true });
  } catch { if (w) w.close(); $('ssError').textContent = 'Studio injoignable : fiche non générée.'; return; }
  if (!r.ok) {
    if (w) w.close();
    $('ssError').textContent = (await r.text().catch(() => '')) || `Fiche non générée (${r.status}).`;
    return;
  }
  const url = URL.createObjectURL(new Blob([await r.text()], { type: 'text/html' }));
  if (w) { w.opener = null; w.location.href = url; } else { $('ssError').textContent = 'Le navigateur a bloqué le nouvel onglet : autorisez les fenêtres de Laser Studio.'; }
  setTimeout(() => URL.revokeObjectURL(url), 60000);
};

// RÉGLAGES › Diagnostic (T-403): the application log's tail and state,
// and the diagnostic report download.
async function loadDiag() {
  let j;
  try {
    const r = await api('GET', '/api/logs/tail?n=200');
    if (!r.ok) return;
    j = await r.json();
  } catch { return; }
  const st = j.status || {};
  $('diagDir').textContent = st.dir || '(journal désactivé)';
  $('diagWarn').textContent = st.write_failed ? `Journal : écriture impossible (${st.last_error || 'erreur inconnue'})`
    : (st.dropped ? `Journal : ${st.dropped} ligne(s) perdue(s) (journal saturé)` : '');
  const tail = $('diagTail'), atEnd = tail.scrollTop + tail.clientHeight >= tail.scrollHeight - 4;
  tail.textContent = j.lines.length ? j.lines.join('\n') : 'Aucune ligne pour le moment.';
  if (atEnd) tail.scrollTop = tail.scrollHeight;
}
$('diagPanel').addEventListener('toggle', () => { if ($('diagPanel').open) loadDiag(); });

// RÉGLAGES › Entrée audio (T-242): read-only diagnostics of the native input.
const MIC_STATE_FR = { off: 'arrêtée', disabled: 'désactivée (--no-audio)', running: 'en cours', no_device: 'aucune entrée', device_lost: 'entrée perdue', permission_denied: 'accès refusé par macOS', error: 'erreur' };
const MIC_SOURCE_FR = { native: 'Native (le studio écoute)', system: 'Son du Mac (le studio écoute la sortie)', browser: 'Navigateur', none: 'Aucune' };
async function loadMicDiag() {
  let d;
  try { d = await apiJson('GET', '/api/audio/diagnostics', undefined, { quiet: true }); } catch { return; }
  const native = musNat(d.source);
  showHealth($('micDiagHealth'), d.health, native ? 'En attente du signal' : 'Capture native arrêtée');
  $('micDiagHelp').textContent = d.health ? d.health.help : native ? '' : 'Choisissez la source « Native » dans LIVE › Musique pour que le studio écoute une entrée du Mac.';
  $('micDiagSource').textContent = MIC_SOURCE_FR[d.source] ?? d.source;
  $('micDiagState').textContent = (MIC_STATE_FR[d.state] ?? d.state) + (d.message ? ` · ${d.message}` : '');
  $('micDiagDevice').textContent = d.device || (d.wanted ? `${d.wanted} (choisie)` : 'entrée par défaut du Mac');
  $('micDiagRate').textContent = d.sample_rate ? `${(d.sample_rate / 1000).toFixed(1).replace('.', ',')} kHz · ${d.channels} canal${d.channels > 1 ? 'aux' : ''}` : '–';
  $('micDiagBuffer').textContent = `${d.buffer_frames} échantillons`;
  $('micDiagLevel').textContent = d.level_db != null ? `${fmtDb(d.level_db)} / ${fmtDb(d.peak_db)}` : '–';
  $('micDiagSignal').textContent = d.signal_present ? 'oui' : 'non';
  $('micDiagSignal').dataset.present = String(!!d.signal_present);
  $('micDiagOverruns').textContent = String(d.overruns ?? 0);
  const sys = d.system_audio;
  $('micDiagSystem').textContent = !sys ? '–' : sys.available ? `disponible (macOS ${sys.macos})` : `indisponible : ${sys.reason}`;
  $('micDiagError').textContent = d.last_error ? `${d.last_error.message} (il y a ${Math.round(d.last_error.ago_s)} s)` : 'aucune';
  const list = $('micDiagDevices');
  list.replaceChildren(...(d.devices.length ? d.devices : [null]).map(dev => {
    const li = document.createElement('li');
    li.textContent = dev ? dev.name + (dev.is_default ? ' (par défaut)' : '') : (d.capture ? 'Aucune entrée trouvée (ou capture native pas encore lancée).' : 'Capture désactivée (--no-audio).');
    return li;
  }));
}
$('micDiagPanel').addEventListener('toggle', () => { if ($('micDiagPanel').open) loadMicDiag(); });
$('micDiagRefresh').onclick = loadMicDiag;
$('micDiagSysHelp').onclick = () => openHelp('son-du-mac');
setInterval(() => { if (activeWs === 'settings' && $('micDiagPanel').open && !document.hidden) loadMicDiag(); }, 1000);

// RÉGLAGES › Performance (T-448): the engine's stage timings over 10 s,
// read once a second while the section is open and shown.
const PERF_LABELS = ['Normal', 'Chargé', 'Images en retard'];
const perfMs = v => (+v || 0).toFixed(2).replace('.', ',') + ' ms';
async function loadPerf() {
  let j;
  try { j = await apiJson('GET', '/api/perf', undefined, { quiet: true }); } catch { return; }
  for (const [id, key] of [['Render', 'render'], ['Safety', 'safety'], ['Output', 'output'], ['Frame', 'frame']]) {
    $('perf' + id + 'Mean').textContent = perfMs(j[key].mean_ms);
    $('perf' + id + 'Worst').textContent = perfMs(j[key].worst_ms);
  }
  $('perfPoints').textContent = j.points;
  $('perfLate').textContent = `${j.late_in_window} sur 10 s · ${j.late_frames} depuis le démarrage`;
  $('perfClients').textContent = j.clients;
  const level = j.late_in_window > 0 ? 2 : j.frame.worst_ms > j.warn_worst_ms ? 1 : 0;
  $('perfState').dataset.level = level;
  $('perfState').textContent = PERF_LABELS[level];
  showPerfLevel(level);
}
// The points indicator under the preview: orange or red like the panel
// (`perf_level` of the pushed state, or the panel's own reading).
function showPerfLevel(level) {
  const el = $('stPoints');
  el.classList.toggle('perf-warn', level === 1);
  el.classList.toggle('perf-late', level === 2);
  if (level) el.title = (level === 2 ? 'Images en retard' : 'Moteur chargé') + ' : voir Réglages › Performance';
  else el.removeAttribute('title');
}
$('perfPanel').addEventListener('toggle', () => { if ($('perfPanel').open) loadPerf(); });
setInterval(() => { if (activeWs === 'settings' && $('perfPanel').open && !document.hidden) loadPerf(); }, 1000);
$('diagExport').onclick = () => {
  const a = document.createElement('a');
  a.href = '/api/diagnostics/report' + ($('diagProject').checked ? '?project=1' : '');
  a.download = '';
  document.body.append(a); a.click(); a.remove();
};
setInterval(() => { if (activeWs === 'settings' && $('diagPanel').open && !document.hidden) loadDiag(); }, 2000);

async function reloadSafety() {
  const j = await apiJson('GET', '/api/safety');
  safety = j.settings; sfLoadError = j.load_error;
  showSafety(true);
}
// Tighter or equal: applied at once. Looser: the server answers 409 with
// what would be loosened, and only « Confirmer l'assouplissement » sends it.
let sfPending = null;
async function saveSafety(next, confirm = false) {
  const r = await sendLatest('safety', 'POST', '/api/safety', confirm ? { ...next, confirm_loosen: true } : next, { quiet: true });
  if (!r) return; // replaced by a newer edit (T-484)
  if (r.ok) {
    safety = (await r.json()).settings; sfLoadError = null; sfPending = null;
    $('sfLoosen').hidden = true;
    showSafety();
    reloadAudience(); // Public zones: the interlock hint and the wizard (T-255)
  } else if (r.status === 409) {
    const j = await r.json();
    sfPending = next;
    $('sfLoosenText').textContent = 'Assouplir la sécurité ? ' + j.loosen.join(' · ');
    $('sfLoosen').hidden = false;
  } else {
    const msg = await r.text();
    await reloadSafety();
    $('sfError').textContent = msg;
  }
}
function sfChanged() {
  clearTimeout(safetyTimer); safetyTimer = setTimeout(() => saveSafety(sfFromControls()), 60);
}
Object.keys(sfFields).forEach(id => $(id).addEventListener('input', () => {
  $(id + 'V').textContent = sfFields[id][2](+$(id).value);
  sfChanged();
}));
$('sfLines').addEventListener('change', sfChanged);
$('sfDwellProfile').addEventListener('change', sfChanged);
$('sfZones').addEventListener('change', e => { if (e.target.matches('input, select')) saveSafety(sfFromControls()); });
$('sfZones').addEventListener('click', e => {
  const row = e.target.closest('.sfzone');
  if (!row) return;
  const i = +row.dataset.zi;
  if (e.target.matches('.zdel')) {
    const next = sfFromControls();
    next.zones.splice(i, 1);
    saveSafety(next);
  } else if (e.target.matches('.zdraw')) {
    if (sfDrawing && sfDrawing.index === i) finishDrawing();
    else { sfDrawing = { index: i, points: [] }; $('sfDrawHint').hidden = false; renderZones(); }
  } else if (e.target.matches('.zgpick')) {
    // The next click on the 2D preview places this end of the gradient.
    const end = e.target.dataset.end;
    sfGradPick = sfGradPick && sfGradPick.index === i && sfGradPick.end === end ? null : { index: i, end };
    renderZones();
  } else if (e.target.matches('.zgtb')) {
    const next = sfFromControls();
    const z = next.zones[i];
    z.gradient = zoneTopDown(z, z.gradient ? z.gradient.level_to : 0.2);
    saveSafety(next);
  }
});
$('sfZones').addEventListener('input', e => {
  if (e.target.matches('.zfeather')) e.target.closest('.zsoft').querySelector('.zfeatherV').textContent = e.target.value + ' %';
  // Shown at once: the row is not redrawn while it has the focus.
  const row = e.target.closest('.sfzone');
  if (e.target.matches('.zgrad')) row.querySelectorAll('.zgradbox').forEach(el => { el.hidden = !e.target.checked; });
  if (e.target.matches('.zkind')) row.querySelector('.zsoft').hidden = e.target.value !== 'dim';
});
function finishDrawing() {
  const d = sfDrawing;
  sfDrawing = null; $('sfDrawHint').hidden = true;
  if (d && d.points.length >= 3) {
    const next = sfFromControls();
    next.zones[d.index].points = d.points;
    saveSafety(next);
  } else renderZones();
}
$('sfZoneAdd').addEventListener('click', () => {
  const next = sfFromControls();
  // Default: a band over the lower part of the field (where the public is).
  next.zones.push({ id: 0, name: `Zone ${next.zones.length + 1}`, kind: 'blank', level: 0.3, points: [[-1, -1], [1, -1], [1, -0.5], [-1, -0.5]] });
  saveSafety(next);
});
$('sfLoosenOk').addEventListener('click', () => { if (sfPending) saveSafety(sfPending, true); });
// RÉGLAGES › Mode extérieur (T-262): part of the safety settings, saved
// through POST /api/safety like the zones (tighter at once, looser after
// « Confirmer l'assouplissement »). « Ciel coupé » is session state:
// POST /api/safety/sky, the blue button next to ARRÊT and Alt+C.
let outdoorState = null, odPending = null, odBadMount = false;
const odNum = (id, def) => { const v = parseFloat($(id).value); return Number.isFinite(v) ? v : def; };
function showOutdoorSettings(force = false) {
  if (!safety || !safety.outdoor) return;
  const o = safety.outdoor, panel = $('outdoorPanel');
  if (!force && panel.contains(document.activeElement)) return;
  $('odEnabled').checked = o.enabled;
  $('odRef').value = o.authorization_ref;
  $('odLat').value = o.wgs84 ? o.wgs84[0] : '';
  $('odLon').value = o.wgs84 ? o.wgs84[1] : '';
  $('odScan').value = o.mounting.scan_angle_deg;
  $('odTilt').value = o.mounting.tilt_deg;
  $('odAz').value = o.mounting.azimuth_deg;
  $('odHeight').value = o.mounting.height_m;
  $('odVert').value = o.max_from_vertical_deg;
  $('odWaive').checked = o.vertical_limit_waived;
  $('odWaiveRef').value = o.waiver_ref;
  $('odRefWarn').hidden = !(o.enabled && !o.authorization_ref.trim());
  $('odSectorAdd').disabled = o.sectors.length >= 16;
  $('odSectors').innerHTML = o.sectors.map((s, i) => `
    <div class="odsector row" data-si="${i}">
      <span class="odnum">${i + 1}</span>
      <label for="odAzMin${i}">Horiz. de</label><input type="number" class="odazmin" id="odAzMin${i}" min="0" max="360" step="1" value="${s.az_min}">
      <label for="odAzMax${i}">à</label><input type="number" class="odazmax" id="odAzMax${i}" min="0" max="360" step="1" value="${s.az_max}">
      <label for="odElMin${i}">Vert. de</label><input type="number" class="odelmin" id="odElMin${i}" min="-90" max="90" step="1" value="${s.el_min}">
      <label for="odElMax${i}">à</label><input type="number" class="odelmax" id="odElMax${i}" min="-90" max="90" step="1" value="${s.el_max}">
      <button class="oddel" aria-label="Supprimer le secteur ${i + 1}">Supprimer</button>
    </div>`).join('');
  odMountRows(o);
}
// The venue's other projectors' mountings (T-477): one row per projector
// of the venue (and per mounting left from one that is gone). Without a
// mounting a projector is fully cut; declaring, changing or moving one
// goes through the same confirmation as the rest (409 → « Confirmer »).
const OD_MOUNT_FIELDS = [['scan_angle_deg', 'Angle de balayage (°)', 1, 120, 1], ['tilt_deg', 'Inclinaison (°)', -90, 90, 1],
  ['azimuth_deg', "Azimut de l'axe (°, 0 = nord)", 0, 360, 1], ['height_m', 'Hauteur au-dessus du sol (m)', 0, 10000, 0.1]];
function odMountRows(o) {
  const extras = venue ? venue.projectors.slice(1) : [], mounts = o.mountings || {};
  const ids = [...extras.map(p => p.id), ...Object.keys(mounts).filter(id => !extras.some(p => p.id === id))];
  const el = (tag, props = {}) => Object.assign(document.createElement(tag), props);
  if (!ids.length) { $('odMounts').replaceChildren(el('div', { className: 'hint', textContent: 'Aucun autre projecteur dans le lieu (RÉGLAGES › Lieu).' })); return; }
  $('odMounts').replaceChildren(...ids.map((id, n) => {
    const p = extras.find(q => q.id === id), m = mounts[id], name = (p && p.name) || id;
    const row = el('div', { className: 'odmount' });
    row.dataset.pid = id;
    row.append(el('div', { className: 'odmount-name', textContent: p ? name : `${name} (n'est plus dans le lieu)` }));
    if (m) {
      const grid = el('div', { className: 'row' });
      for (const [k, label, min, max, step] of OD_MOUNT_FIELDS) {
        const cell = el('div'), inputId = `odM${n}_${k}`;
        cell.append(el('label', { htmlFor: inputId, textContent: label }),
          el('input', { type: 'number', id: inputId, className: 'odm', min, max, step, value: m[k] }));
        cell.lastChild.dataset.k = k;
        grid.append(cell);
      }
      row.append(grid, el('button', { className: 'odmdel', textContent: 'Retirer le montage', ariaLabel: `Retirer le montage de ${name}` }));
    } else {
      row.append(el('div', { className: 'hint sf-warn', textContent: 'Montage non déclaré : entièrement éteint en mode extérieur et quand le ciel est coupé.' }),
        el('button', { className: 'odmadd', textContent: 'Déclarer un montage', ariaLabel: `Déclarer le montage de ${name}` }));
    }
    return row;
  }));
}
/** The other projectors' mountings from their rows; null when a value is not a number. */
function odMountsFromControls() {
  const out = {};
  for (const row of $('odMounts').querySelectorAll('.odmount')) {
    const inputs = row.querySelectorAll('input.odm');
    if (!inputs.length) continue;
    const m = {};
    for (const i of inputs) {
      const v = parseFloat(i.value);
      if (!Number.isFinite(v)) return null;
      m[i.dataset.k] = v;
    }
    out[row.dataset.pid] = m;
  }
  return out;
}
function odFromControls() {
  const next = structuredClone(safety), o = next.outdoor;
  o.enabled = $('odEnabled').checked;
  o.authorization_ref = $('odRef').value.trim();
  const lat = parseFloat($('odLat').value), lon = parseFloat($('odLon').value);
  o.wgs84 = Number.isFinite(lat) && Number.isFinite(lon) ? [lat, lon] : null;
  o.mounting = {
    scan_angle_deg: odNum('odScan', o.mounting.scan_angle_deg), tilt_deg: odNum('odTilt', o.mounting.tilt_deg),
    azimuth_deg: odNum('odAz', o.mounting.azimuth_deg), height_m: odNum('odHeight', o.mounting.height_m),
  };
  // A mounting value that is not a number is refused, never guessed (T-477).
  const mounts = odMountsFromControls();
  odBadMount = !mounts;
  if (mounts) o.mountings = mounts;
  o.max_from_vertical_deg = odNum('odVert', o.max_from_vertical_deg);
  o.vertical_limit_waived = $('odWaive').checked;
  o.waiver_ref = $('odWaiveRef').value.trim();
  o.sectors = [...$('odSectors').querySelectorAll('.odsector')].map((row, i) => {
    const old = safety.outdoor.sectors[i] || {};
    const n = (cls, def) => { const v = parseFloat(row.querySelector(cls).value); return Number.isFinite(v) ? v : def; };
    return { az_min: n('.odazmin', old.az_min), az_max: n('.odazmax', old.az_max), el_min: n('.odelmin', old.el_min), el_max: n('.odelmax', old.el_max) };
  });
  return next;
}
async function odSave(next, confirm = false) {
  const r = await api('POST', '/api/safety', confirm ? { ...next, confirm_loosen: true } : next, { quiet: true });
  if (r.ok) {
    safety = (await r.json()).settings; odPending = null;
    $('odLoosen').hidden = true; $('odError').textContent = '';
    showSafety(); showOutdoorSettings(true);
  } else if (r.status === 409) {
    const j = await r.json();
    odPending = next;
    $('odLoosenText').textContent = 'Assouplir la sécurité ? ' + j.loosen.join(' · ');
    $('odLoosen').hidden = false;
  } else {
    const msg = await r.text();
    await reloadSafety();
    showOutdoorSettings(true);
    $('odError').textContent = msg;
  }
}
$('outdoorPanel').addEventListener('change', e => {
  if (!e.target.matches('input')) return;
  const next = odFromControls();
  if (odBadMount) { $('odError').textContent = 'Montage : chaque valeur doit être un nombre. Rien n\'a été changé.'; return; }
  odSave(next);
});
$('odMounts').addEventListener('click', e => {
  const row = e.target.closest('.odmount');
  if (!row || !e.target.matches('.odmadd, .odmdel')) return;
  const next = odFromControls();
  if (odBadMount) { $('odError').textContent = 'Montage : chaque valeur doit être un nombre. Rien n\'a été changé.'; return; }
  const id = row.dataset.pid;
  // Declared from the main projector's mounting: the operator then sets
  // this one's own orientation; both steps need the confirmation.
  if (e.target.matches('.odmadd')) next.outdoor.mountings[id] = { ...next.outdoor.mounting };
  else delete next.outdoor.mountings[id];
  odSave(next);
});
$('outdoorPanel').addEventListener('toggle', () => { if ($('outdoorPanel').open) showOutdoorSettings(true); });
$('odSectors').addEventListener('click', e => {
  const row = e.target.closest('.odsector');
  if (!row || !e.target.matches('.oddel')) return;
  const next = odFromControls();
  next.outdoor.sectors.splice(+row.dataset.si, 1);
  odSave(next);
});
$('odSectorAdd').addEventListener('click', () => {
  const next = odFromControls(), m = next.outdoor.mounting, half = m.scan_angle_deg / 2;
  // Default: the field as mounted, the projector's own angles.
  const az = a => Math.round(((a % 360) + 360) % 360);
  next.outdoor.sectors.push({ az_min: az(m.azimuth_deg - half), az_max: az(m.azimuth_deg + half),
    el_min: Math.max(-90, Math.round(m.tilt_deg - half)), el_max: Math.min(90, Math.round(m.tilt_deg + half)) });
  odSave(next);
});
$('odExport').addEventListener('click', () => {
  const a = document.createElement('a');
  a.href = '/api/safety/outdoor/export';
  a.download = 'secteurs-dgta.csv';
  document.body.append(a); a.click(); a.remove();
});
$('odLoosenOk').addEventListener('click', () => { if (odPending) odSave(odPending, true); });
$('odLoosenCancel').addEventListener('click', () => { odPending = null; $('odLoosen').hidden = true; showOutdoorSettings(true); });
/** The outdoor summary from /api/state or a frame: the blue button. */
function showOutdoor(o) {
  const was = outdoorState;
  outdoorState = o || null;
  const btn = $('skyBtn'), on = !!(o && o.sky_off);
  btn.hidden = !(o && (o.enabled || o.sky_off));
  btn.setAttribute('aria-pressed', String(on));
  btn.title = on ? 'Ciel coupé : rien au-dessus de l\'horizon. Cliquer (ou Alt+C) pour rétablir le ciel.' : 'Ciel coupé (Alt+C) : éteint tout au-dessus de l\'horizon sans désarmer · safety.sky_off';
  if (o && was && JSON.stringify(o.cut) !== JSON.stringify(was.cut) && lastPoints && show2d) draw(lastShown);
  const key = o => o ? JSON.stringify([o.enabled, o.sky_off, o.cut, o.cuts]) : '';
  if (key(o) !== key(was)) outdoorTo3d();
}
async function toggleSky(source) {
  const r = await api('POST', '/api/safety/sky', { toggle: true, source }, { quiet: true });
  if (r.ok) showOutdoor({ ...(outdoorState || {}), ...(await r.json()) });
}
$('skyBtn').addEventListener('click', () => toggleSky('ui'));

$('sfLoosenCancel').addEventListener('click', () => { sfPending = null; $('sfLoosen').hidden = true; reloadSafety(); });
$('sfOverlay').addEventListener('change', () => { if (lastPoints && show2d) draw(lastShown); });
// Drawing a zone: each click on the 2D preview adds a vertex.
$('preview').addEventListener('click', e => {
  if (!sfDrawing && !sfGradPick) return;
  const r = $('preview').getBoundingClientRect();
  const q = v => Math.round(Math.min(1, Math.max(-1, v)) * 100) / 100;
  if (sfGradPick) {
    const pick = sfGradPick;
    sfGradPick = null;
    const next = sfFromControls();
    const z = next.zones[pick.index];
    renderZones();
    if (z && z.gradient) {
      z.gradient[pick.end] = [q((e.clientX - r.left) / r.width * 2 - 1), q(1 - (e.clientY - r.top) / r.height * 2)];
      saveSafety(next);
    }
    return;
  }
  sfDrawing.points.push([q((e.clientX - r.left) / r.width * 2 - 1), q(1 - (e.clientY - r.top) / r.height * 2)]);
  if (sfDrawing.points.length >= 24) finishDrawing();
});

export { assignSafety, outdoorState, reloadSafety, safety, sfDrawing, sfGradPick, showOutdoor,
  showOutdoorSettings, showPerfLevel, showSafety, toggleSky };
