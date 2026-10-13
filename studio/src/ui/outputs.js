// Laser Studio — outputs.js (T-435).
// Sections (as in the former index.html script):
//   power caps and projector sheet (T-254)
//   Sortie DMX (T-453): haze machine and simple lights
//   damaged data files (T-402)
//   mode de sortie (T-255)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './modulators.js';
import { $, api, apiJson, noticeCondition, sendLatest } from './core.js';
import { clock } from './laser.js';
import { draw, showGalvo } from './presence.js';
import { lastPoints, lastShown, safetyTo3d, show2d } from './view3d.js';
import { activeWs, setWorkspace } from './workspaces.js';
import { postTestPattern } from './cuelist.js';
import { reloadSafety } from './safety.js';
import { assignAudienceMode, audienceMode } from './targets.js';
import { assignTlDmxKey } from './timeline-markers.js';

// ---------- power caps and projector sheet (T-254) ----------
// `outputCfg` is what the server applies to the active output. Lowering a
// cap is saved at once; raising one answers 409 and only « Confirmer la
// hausse » sends it (never while armed: the server refuses it anyway).
let outputCfg = null, pwTimer = null, pwPending = null, pwLoadError = null;
const pwFields = { pwPower: l => l.max_power, pwRed: l => l.max_color[0], pwGreen: l => l.max_color[1], pwBlue: l => l.max_color[2] };
function showCapHint() {
  if (!outputCfg) return;
  $('capHint').textContent = `Plafond de la sortie : ${Math.round(outputCfg.limits.max_power * 100)} % (appliqué après cette luminosité)`;
}
function showOutputs(force = false) {
  if (!outputCfg) return;
  showGalvo(); // its « fiche projecteur » divergence
  for (const [id, get] of Object.entries(pwFields)) {
    if (force || document.activeElement !== $(id)) $(id).value = Math.round(get(outputCfg.limits) * 100);
    $(id + 'V').textContent = Math.round(get(outputCfg.limits) * 100) + ' %';
  }
  $('pwError').textContent = pwLoadError || '';
  const p = outputCfg.projector;
  if (force || !$('outputsPanel').contains(document.activeElement)) {
    $('pjName').value = p.name; $('pjClass').value = p.class; $('pjNotes').value = p.notes;
    ['pjPowR', 'pjPowG', 'pjPowB'].forEach((id, i) => { $(id).value = p.power_mw[i]; });
    ['pjWlR', 'pjWlG', 'pjWlB'].forEach((id, i) => { $(id).value = p.wavelength_nm[i]; });
    $('pjAperture').value = p.aperture_mm; $('pjDiv').value = p.divergence_mrad; $('pjScan').value = p.scan_angle_deg;
    $('pjScanFail').value = p.hw_scan_fail === true ? 'yes' : p.hw_scan_fail === false ? 'no' : '';
    $('pjLens').checked = p.divergence_lens;
    $('pjModel').value = p.dac_model; $('pjXyBits').value = p.xy_bits; $('pjColorBits').value = p.color_bits; $('pjMaxPps').value = p.max_pps;
  }
  $('pjShownetHelp').hidden = p.dac_model !== 'shownet';
  showCapHint();
}
// Interface models (T-472): choosing one fills its resolution and rate.
let pjModels = [];
// The projector whose caps, sheet and colours are shown (T-277).
const pwTargetId = () => $('pwTarget').value || 'main';
async function reloadOutputs() {
  const j = await apiJson('GET', '/api/outputs/limits');
  outputCfg = j.outputs.find(o => o.id === pwTargetId()) || j.outputs.find(o => o.id === j.active); pwLoadError = j.load_error;
  if (!pjModels.length && j.models) {
    pjModels = j.models;
    $('pjModel').innerHTML = pjModels.map(m => `<option value="${m.model}">${m.label_fr}</option>`).join('');
  }
  showOutputs(true);
}
function pwLimitsFromControls() {
  const v = id => Math.min(100, Math.max(0, +$(id).value || 0)) / 100;
  return { max_power: v('pwPower'), max_color: [v('pwRed'), v('pwGreen'), v('pwBlue')] };
}
function pjFromControls() {
  const n = id => Math.max(0, +$(id).value || 0);
  const sf = $('pjScanFail').value;
  return {
    name: $('pjName').value, class: $('pjClass').value, notes: $('pjNotes').value,
    power_mw: ['pjPowR', 'pjPowG', 'pjPowB'].map(n), wavelength_nm: ['pjWlR', 'pjWlG', 'pjWlB'].map(id => Math.round(n(id))),
    aperture_mm: n('pjAperture'), divergence_mrad: n('pjDiv'), scan_angle_deg: n('pjScan'),
    hw_scan_fail: sf === 'yes' ? true : sf === 'no' ? false : null, divergence_lens: $('pjLens').checked,
    dac_model: $('pjModel').value, xy_bits: Math.round(n('pjXyBits')), color_bits: Math.round(n('pjColorBits')), max_pps: Math.round(n('pjMaxPps')),
  };
}
async function saveOutputs(body, confirm = false) {
  body = { ...body, id: pwTargetId() };
  const r = await sendLatest('outputs:' + Object.keys(body).sort().join(), 'POST', '/api/outputs/limits', confirm ? { ...body, confirm_loosen: true } : body, { quiet: true });
  if (!r) return; // replaced by a newer edit (T-484)
  if (r.ok) {
    outputCfg = (await r.json()).output; pwLoadError = null; pwPending = null;
    $('pwLoosen').hidden = true;
    showOutputs();
  } else if (r.status === 409) {
    const j = await r.json();
    if (j.armed) {
      pwPending = null; $('pwLoosen').hidden = true;
      await reloadOutputs();
      $('pwError').textContent = j.error + ' (' + j.loosen.join(' · ') + ')';
    } else {
      pwPending = body;
      $('pwLoosenText').textContent = 'Relever le plafond de puissance ? ' + j.loosen.join(' · ');
      $('pwLoosen').hidden = false;
    }
  } else {
    const msg = await r.text();
    await reloadOutputs();
    $('pwError').textContent = msg;
  }
}
Object.keys(pwFields).forEach(id => $(id).addEventListener('input', () => {
  $(id + 'V').textContent = $(id).value + ' %';
  clearTimeout(pwTimer); pwTimer = setTimeout(() => saveOutputs({ limits: pwLimitsFromControls() }), 60);
}));
['pjName', 'pjClass', 'pjNotes', 'pjPowR', 'pjPowG', 'pjPowB', 'pjWlR', 'pjWlG', 'pjWlB', 'pjAperture', 'pjDiv', 'pjScan', 'pjScanFail', 'pjLens', 'pjXyBits', 'pjColorBits', 'pjMaxPps']
  .forEach(id => $(id).addEventListener('change', () => saveOutputs({ projector: pjFromControls() })));
$('pjModel').addEventListener('change', () => {
  const m = pjModels.find(x => x.model === $('pjModel').value);
  if (m) { $('pjXyBits').value = m.xy_bits; $('pjColorBits').value = m.color_bits; $('pjMaxPps').value = m.max_pps; }
  saveOutputs({ projector: pjFromControls() });
});
$('pwLoosenOk').addEventListener('click', () => { if (pwPending) saveOutputs(pwPending, true); });
$('pwLoosenCancel').addEventListener('click', () => { pwPending = null; $('pwLoosen').hidden = true; reloadOutputs(); });

// Couleurs de la sortie (T-414): white balance, a curve per colour (gamma
// or 5 points) and the colour delay of the output. Darker applies at once;
// brighter gets a 409 listing what would brighten, and only « Confirmer
// l'assouplissement » sends `confirm_loosen`. Machine settings: no
// shortcut, no MIDI/OSC/DMX control.
const OC_CH = [['r', 'Rouge', '--danger'], ['g', 'Vert', '--ok'], ['b', 'Bleu', '--accent']];
const OC_NODES = [0, 25, 50, 75, 100];
let outColor = null, ocTimer = null, ocPending = null;
$('ocCurves').innerHTML = OC_CH.map(([k, name], i) => `
  <div class="oc-ch">
    <h4 class="${k}">Courbe ${name.toLowerCase()}</h4>
    <canvas id="ocGraph${i}" width="144" height="144" aria-label="Courbe ${name.toLowerCase()}"></canvas>
    <select id="ocType${i}" aria-label="Type de courbe ${name.toLowerCase()}"><option value="gamma">Gamma</option><option value="points">5 points</option></select>
    <div id="ocGammaRow${i}"><label>Gamma <span class="v" id="ocGammaV${i}"></span></label><input type="range" id="ocGamma${i}" data-default="1" min="0.2" max="5" step="0.05"></div>
    <div id="ocPtsRow${i}" class="oc-pts" hidden>${OC_NODES.map(n => `<span>${n} %</span>`).join('')}${OC_NODES.map((n, j) =>
      `<input type="number" id="ocPt${i}_${j}" min="0" max="100" step="1" title="Sortie pour ${n} % en entrée"${j === 0 ? ' disabled' : ''}>`).join('')}</div>
  </div>`).join('');
function ocEval(c, v) {
  if (v <= 0) return 0;
  if (c.type === 'gamma') return Math.pow(Math.min(v, 1), c.gamma);
  const x = Math.min(v, 1) * 4, i = Math.min(3, Math.floor(x)), f = x - i;
  return c.out[i] + (c.out[i + 1] - c.out[i]) * f;
}
function ocDraw(i, c) {
  const cv = $('ocGraph' + i), g = cv.getContext('2d'), W = cv.width, H = cv.height;
  const css = getComputedStyle(document.documentElement);
  g.clearRect(0, 0, W, H);
  g.strokeStyle = css.getPropertyValue('--line').trim() || '#262a3b'; g.lineWidth = 1;
  g.beginPath(); g.moveTo(0, H); g.lineTo(W, 0); g.stroke();
  g.strokeStyle = css.getPropertyValue(OC_CH[i][2]).trim() || '#fff'; g.lineWidth = 3; g.beginPath();
  for (let k = 0; k <= 48; k++) { const v = k / 48, y = H - ocEval(c, v) * H; k ? g.lineTo(v * W, y) : g.moveTo(0, y); }
  g.stroke();
}
function ocCurveFromControls(i) {
  if ($('ocType' + i).value === 'gamma') return { type: 'gamma', gamma: Math.min(5, Math.max(0.2, +$('ocGamma' + i).value || 1)) };
  const out = OC_NODES.map((_, j) => j === 0 ? 0 : Math.min(100, Math.max(0, +$(`ocPt${i}_${j}`).value || 0)) / 100);
  for (let j = 1; j < 5; j++) out[j] = Math.max(out[j], out[j - 1]);
  return { type: 'points', out };
}
function ocFromControls() {
  return {
    white_balance: ['ocWbR', 'ocWbG', 'ocWbB'].map(id => Math.min(100, Math.max(0, +$(id).value || 0)) / 100),
    curve: [0, 1, 2].map(ocCurveFromControls),
    color_delay_points: Math.round(+$('ocDelay').value || 0),
  };
}
function showOutColor(force = false) {
  if (!outColor) return;
  const c = outColor.color, editing = !force && $('ocBox').contains(document.activeElement);
  ['ocWbR', 'ocWbG', 'ocWbB'].forEach((id, i) => {
    if (!editing) $(id).value = Math.round(c.white_balance[i] * 100);
    $(id + 'V').textContent = $(id).value + ' %';
  });
  c.curve.forEach((cu, i) => {
    if (!editing) {
      $('ocType' + i).value = cu.type;
      if (cu.type === 'gamma') $('ocGamma' + i).value = cu.gamma;
      const out = cu.type === 'points' ? cu.out : OC_NODES.map(n => ocEval(cu, n / 100));
      OC_NODES.forEach((_, j) => { $(`ocPt${i}_${j}`).value = Math.round(out[j] * 100); });
    }
    $('ocGammaRow' + i).hidden = $('ocType' + i).value !== 'gamma';
    $('ocPtsRow' + i).hidden = $('ocType' + i).value !== 'points';
    $('ocGammaV' + i).textContent = (+$('ocGamma' + i).value).toFixed(2);
    ocDraw(i, ocCurveFromControls(i));
  });
  if (!editing) $('ocDelay').value = c.color_delay_points;
  $('ocDelayV').textContent = $('ocDelay').value;
  $('ocLibDelay').textContent = `Sur un vrai DAC, la bibliothèque laser-dac décale déjà la couleur de ${outColor.library_delay_points} points ; ce réglage s'y ajoute.`;
}
async function reloadOutColor() {
  try {
    const r = await api('GET', '/api/outputs/color?output=' + encodeURIComponent(pwTargetId()));
    if (r.ok) { outColor = await r.json(); showOutColor(true); }
  } catch (e) { console.warn('/api/outputs/color', e); }
}
async function saveOutColor(color, confirm = false) {
  const path = '/api/outputs/color?output=' + encodeURIComponent(pwTargetId());
  const r = await sendLatest(path, 'POST', path, confirm ? { color, confirm_loosen: true } : { color }, { quiet: true });
  if (!r) return; // replaced by a newer edit (T-484)
  $('ocError').textContent = '';
  if (r.ok) {
    outColor = await r.json(); ocPending = null; $('ocLoosen').hidden = true;
    showOutColor();
  } else if (r.status === 409) {
    const j = await r.json();
    if (j.armed) {
      // Armed: brighter is refused outright, like raising a power cap.
      ocPending = null; $('ocLoosen').hidden = true;
      await reloadOutColor();
      $('ocError').textContent = j.error + ' (' + j.loosen.join(' · ') + ')';
      return;
    }
    ocPending = color;
    $('ocLoosenText').textContent = "Ce réglage augmente la lumière émise. Confirmer l'assouplissement ? " + j.loosen.join(' · ');
    $('ocLoosen').hidden = false;
  } else {
    const msg = await r.text();
    await reloadOutColor();
    $('ocError').textContent = msg;
  }
}
function ocChanged() {
  showOutColor();
  clearTimeout(ocTimer); ocTimer = setTimeout(() => saveOutColor(ocFromControls()), 60);
}
['ocWbR', 'ocWbG', 'ocWbB', 'ocDelay'].forEach(id => $(id).addEventListener('input', ocChanged));
[0, 1, 2].forEach(i => {
  $('ocGamma' + i).addEventListener('input', ocChanged);
  $('ocType' + i).addEventListener('change', ocChanged);
  OC_NODES.forEach((_, j) => $(`ocPt${i}_${j}`).addEventListener('change', ocChanged));
});
$('ocLoosenOk').addEventListener('click', () => { if (ocPending) saveOutColor(ocPending, true); });
$('ocLoosenCancel').addEventListener('click', () => { ocPending = null; $('ocLoosen').hidden = true; reloadOutColor(); });
$('ocReset').addEventListener('click', () => { if (outColor) saveOutColor(outColor.defaults); });
$('ocTpRamp').addEventListener('click', () => postTestPattern({ pattern: 'white_ramp' }));
$('ocTpBlank').addEventListener('click', () => postTestPattern({ pattern: 'blanking' }));

// Balayage de la sortie (T-171): scan speed range and points per frame of
// the chosen output. The warnings (its rate outside the range, above the
// projector sheet's interface maximum) are shown, never applied silently.
// The main output's alert threshold drives the « Images/s » indicator.
var scanAlertFps = 35;
const OSCAN_FIELDS = { oscanPpsMin: 'pps_min', oscanPpsMax: 'pps_max', oscanMinPts: 'min_points', oscanMaxPts: 'max_points', oscanFps: 'min_fps' };
function showScanLimits(j) {
  for (const [id, k] of Object.entries(OSCAN_FIELDS)) if (document.activeElement !== $(id)) $(id).value = j.scan[k];
  if (j.output === 'main') scanAlertFps = j.alert_fps;
  $('oscanInfo').textContent = `La sortie tourne à ${j.pps} points/s : vitesse de balayage de ${Math.round(j.limits.min_scan_pct)} à ${Math.round(j.limits.max_scan_pct)} %. `
    + `Alerte sous ${+j.alert_fps.toFixed(1)} images/s.`;
  $('oscanWarn').textContent = j.warnings.join(' ');
  showFixedRate(j.fixed);
}

// Cadence fixe (T-463): an output setting, not a control (no shortcut, no
// MIDI). Turned on or changed laser disarmed; off always applies. The
// line under it shows the points of one frame and the rate measured.
const FR_CHOICES = ['24', '25', '30', '50', '60'];
var fixedRateView = null;
function frNumber(v, d) { return v.toLocaleString('fr-FR', { minimumFractionDigits: d, maximumFractionDigits: d }); }
function showFixedRate(fx) {
  fixedRateView = fx || null;
  if (!fx) return;
  const sel = $('ofrChoice');
  if (document.activeElement !== sel && document.activeElement !== $('ofrValue')) {
    const v = fx.fps == null ? '' : String(+fx.fps);
    sel.value = fx.fps == null ? '' : FR_CHOICES.includes(v) ? v : 'other';
    $('ofrValue').value = fx.fps == null ? '' : +fx.fps;
  }
  const other = sel.value === 'other';
  $('ofrValue').disabled = !other;
  $('ofrApply').disabled = !other;
  showFixedRateStatus(fx.status);
}
function showFixedRateStatus(st) {
  const fx = fixedRateView;
  if (!st && (!fx || fx.fps == null)) { $('ofrInfo').textContent = 'Désactivée : le nombre de points suit le contenu.'; $('ofrWarn').textContent = ''; return; }
  const pts = (st && st.points) || fx.points;
  $('ofrInfo').textContent = `${frNumber(pts, 0)} points par image`
    + (st && st.measured_fps ? ` · cadence mesurée ${frNumber(st.measured_fps, 2)} im/s (écart ${frNumber(st.deviation_pct, 1)} %)` : '');
  $('ofrWarn').textContent = st && st.report && st.report.over_step
    ? `Trop de points pour la cadence fixe : réduisez le budget de points à ${frNumber(st.report.output, 0)} ou baissez la cadence.` : '';
}
async function saveFixedRate(fps) {
  const r = await api('POST', '/api/outputs/fixed_rate?output=' + encodeURIComponent(pwTargetId()), { fps }, { quiet: true });
  $('ofrError').textContent = '';
  if (r.ok) { showScanLimits(await r.json()); return; }
  const msg = r.status === 409 ? (j => j.error + ' (' + j.loosen.join(' · ') + ')')(await r.json()) : await r.text();
  await reloadScanLimits();
  $('ofrError').textContent = msg;
}
$('ofrChoice').addEventListener('change', () => {
  const v = $('ofrChoice').value;
  const other = v === 'other';
  $('ofrValue').disabled = !other;
  $('ofrApply').disabled = !other;
  if (other) { $('ofrValue').focus(); return; }
  saveFixedRate(v === '' ? null : +v);
});
$('ofrApply').addEventListener('click', () => {
  const v = +$('ofrValue').value;
  if (!Number.isFinite(v) || v < 20 || v > 60) { $('ofrError').textContent = 'Cadence fixe : une valeur de 20 à 60 images/s.'; return; }
  saveFixedRate(v);
});
async function reloadScanLimits() {
  try {
    const r = await api('GET', '/api/outputs/scan_limits?output=' + encodeURIComponent(pwTargetId()), undefined, { quiet: true });
    if (r.ok) showScanLimits(await r.json());
  } catch (e) { console.warn('/api/outputs/scan_limits', e); }
}
async function saveScanLimits() {
  const scan = Object.fromEntries(Object.entries(OSCAN_FIELDS).map(([id, k]) => [k, +$(id).value]));
  const r = await api('POST', '/api/outputs/scan_limits?output=' + encodeURIComponent(pwTargetId()), { scan }, { quiet: true });
  $('oscanError').textContent = '';
  if (r.ok) { showScanLimits(await r.json()); return; }
  const msg = r.status === 409 ? (j => j.error + ' (' + j.loosen.join(' · ') + ')')(await r.json()) : await r.text();
  await reloadScanLimits();
  $('oscanError').textContent = msg;
}
$('oscanSave').addEventListener('click', saveScanLimits);
$('pwTarget').addEventListener('change', reloadScanLimits);
reloadScanLimits();

// Sorties (T-401, after T-303's read-only list): the studio scans only on
// « Rechercher » (about 1.5 s, on its own thread). « Connecter » opens the
// output on the server's own thread; every change disarms the laser and a
// lost output comes back disarmed. No shortcut and no MIDI/OSC/DMX control.
const DEV_KINDS = { helios: 'Helios USB', etherdream: 'Ether Dream', idn: 'IDN', lasercube: 'LaserCube', ponk: 'PONK', artnet: 'ShowNET SD', audio: 'Audio', test: 'Test' };
const LINK_STATES = { connected: 'Connectée', connecting: 'Connexion…', lost: 'Perdue' };
function outStateText(j) {
  const name = j.name || j.selected;
  if (j.state === 'connected') return 'Connectée : ' + name;
  if (j.state === 'connecting') return 'Connexion… ' + (name || '');
  if (j.state === 'lost') {
    const retry = j.retry_in_ms == null ? (j.auto_reconnect ? 'nouvelle tentative…' : 'reconnexion automatique désactivée')
      : 'nouvelle tentative dans ' + Math.max(1, Math.ceil(j.retry_in_ms / 1000)) + ' s';
    return 'Sortie perdue — ' + retry + (j.error ? ' (' + j.error + ')' : '');
  }
  return 'Aucune sortie : aperçu seulement';
}
function showDevices(j) {
  const list = $('devList');
  // Rebuilt only when it changes, so a click never lands on a replaced button.
  const key = JSON.stringify([j.devices, j.scanning, j.scanned]);
  if (list.dataset.key !== key) {
    list.dataset.key = key;
    renderDeviceList(list, j);
  }
  $('devStatus').textContent = j.scanning ? 'Recherche en cours…'
    : j.scan_error ? 'Erreur : ' + j.scan_error
    : !j.scanned ? 'DAC pas encore cherchés.'
    : !j.usb ? 'USB non scanné pendant la sortie sur un DAC.' : '';
  // T-496: a LaserCube is a whole projector; the studio's stop is never enough.
  $('lcHelp').hidden = !j.devices.some(d => d.kind === 'lasercube');
  $('devStatus').classList.toggle('sf-error', !!j.scan_error);
  $('devScan').disabled = !!j.scanning;
  $('outState').dataset.state = j.state;
  $('outState').textContent = outStateText(j);
  $('outDisconnect').disabled = !j.selected;
  if (document.activeElement !== $('outAuto')) $('outAuto').checked = j.auto_reconnect;
}
function renderDeviceList(list, j) {
  list.replaceChildren();
  for (const d of j.devices) {
    const li = document.createElement('li');
    li.dataset.id = d.id; li.dataset.state = d.state;
    const kind = document.createElement('span'); kind.className = 'dev-kind'; kind.textContent = DEV_KINDS[d.kind] || d.kind;
    const name = document.createElement('span'); name.className = 'dev-name'; name.textContent = d.name; name.title = d.name;
    const id = document.createElement('code'); id.textContent = d.id; id.title = '--device ' + d.id;
    const state = document.createElement('span'); state.className = 'dev-state'; state.textContent = LINK_STATES[d.state] || '';
    const connect = document.createElement('button'); connect.className = 'dev-connect'; connect.textContent = 'Connecter';
    connect.title = 'Connecter cette sortie (désarme le laser ; il reste désarmé une fois connecté)';
    connect.hidden = d.state === 'connected' || d.state === 'connecting';
    connect.onclick = () => selectOutput(d.id);
    li.append(kind, name, id, state, connect);
    list.append(li);
  }
  if (!j.devices.length) {
    const li = document.createElement('li'); li.className = 'empty';
    li.textContent = j.scanning ? 'Recherche…' : j.scanned ? 'Aucune sortie trouvée.' : 'Pas encore cherché : cliquez sur Rechercher.';
    list.append(li);
  }
}
async function reloadDevices() {
  try { showDevices(await apiJson('GET', '/api/outputs')); } catch { /* next try */ }
}
async function selectOutput(id) {
  const r = await api('POST', '/api/outputs/select', { id, source: 'ui' }, { quiet: true });
  if (r.ok) showDevices(await r.json());
  else { $('devStatus').textContent = await r.text(); $('devStatus').classList.add('sf-error'); }
}
$('devScan').addEventListener('click', async () => {
  let j = await apiJson('POST', '/api/outputs/scan');
  showDevices(j);
  for (let i = 0; j.scanning && i < 40; i++) {
    await new Promise(r => setTimeout(r, 250));
    j = await apiJson('GET', '/api/outputs');
    showDevices(j);
  }
});
$('outDisconnect').addEventListener('click', () => selectOutput(null));
$('outAuto').addEventListener('change', async () => {
  const r = await api('POST', '/api/outputs/config', { auto_reconnect: $('outAuto').checked });
  if (r.ok) showDevices(await r.json());
});
// ShowNET SD card over Art-Net (T-301): the box's address is typed, never
// searched for. Saved only while that output is disconnected; the box then
// appears in the list above. Its live report comes with every frame.
async function loadArtnet() {
  try { showArtnet(await apiJson('GET', '/api/outputs/artnet', undefined, { cache: 'no-store' })); } catch { /* next time */ }
}
function showArtnet(j) {
  const c = j.config;
  if (!$('anProfile').children.length) {
    for (const p of j.profiles) { const o = document.createElement('option'); o.value = p.id; o.textContent = p.label; $('anProfile').append(o); }
  }
  $('anTarget').value = c.target ? c.target.replace(/:6454$/, '') : '';
  $('anProfile').value = c.profile;
  $('anUniverse').value = c.universe;
  $('anAddress').value = c.dmx_address;
  $('anZoneSide').value = c.zone_side;
  $('anZoneLevel').value = c.zone_intensity;
  $('anFollowSize').checked = c.follow_size;
}
$('anSave').addEventListener('click', async () => {
  const body = {
    target: $('anTarget').value.trim() || null,
    profile: $('anProfile').value,
    universe: +$('anUniverse').value || 0,
    dmx_address: +$('anAddress').value || 1,
    zone_side: +$('anZoneSide').value || 0,
    zone_intensity: +$('anZoneLevel').value || 0,
    follow_size: $('anFollowSize').checked,
  };
  const r = await api('POST', '/api/outputs/artnet', body, { quiet: true });
  if (r.ok) {
    const j = await r.json();
    showArtnet(j);
    $('anMsg').textContent = j.id ? 'Enregistré : « Connecter » dans la liste ci-dessus.' : 'Sortie ShowNET carte SD désactivée.';
    $('anMsg').classList.remove('sf-error');
    reloadDevices();
  } else {
    $('anMsg').textContent = await r.text();
    $('anMsg').classList.add('sf-error');
  }
});
// Audio interface (T-498): an oscilloscope, or a DC-coupled interface.
// Every launch starts in oscilloscope mode; laser mode asks the operator
// to confirm the risk first. Saved only while that output is disconnected.
let audioOut = null;
async function loadAudioOut() {
  try { showAudioOut(await apiJson('GET', '/api/outputs/audio', undefined, { cache: 'no-store' })); } catch { /* next time */ }
}
function showAudioOut(j) {
  audioOut = j;
  const c = j.config;
  const names = j.devices.map(d => d.name);
  if (c.device && !names.includes(c.device)) names.push(c.device);
  $('aoDevice').replaceChildren(new Option('Aucune', ''), ...names.map(n => {
    const d = j.devices.find(x => x.name === n);
    return new Option(d ? `${n} (${d.channels} canaux)` : `${n} (absente)`, n);
  }));
  $('aoDevice').value = c.device || '';
  for (const k of ['x', 'y', 'r', 'g', 'b']) $('ao' + k.toUpperCase()).value = c.channels[k];
  $('aoMode').value = j.mode;
}
$('aoSave').addEventListener('click', async () => {
  const mode = $('aoMode').value;
  const body = { device: $('aoDevice').value || null, channels: {}, mode };
  for (const k of ['x', 'y', 'r', 'g', 'b']) body.channels[k] = +$('ao' + k.toUpperCase()).value || 0;
  if (mode === 'laser' && (!audioOut || audioOut.mode !== 'laser')) {
    if (!confirm(audioOut ? audioOut.laser_confirm : 'Passer en mode Laser ?')) { $('aoMode').value = audioOut ? audioOut.mode : 'scope'; return; }
    body.confirm_laser = true;
  } else if (mode === 'laser') {
    body.confirm_laser = true;
  }
  const r = await api('POST', '/api/outputs/audio', body, { quiet: true });
  if (r.ok) {
    const j = await r.json();
    showAudioOut(j);
    $('aoMsg').textContent = j.id ? `Enregistré (mode ${j.mode === 'laser' ? 'Laser' : 'Oscilloscope'}) : « Connecter » dans la liste ci-dessus.` : 'Sortie interface audio désactivée.';
    $('aoMsg').classList.remove('sf-error');
    reloadDevices();
  } else {
    $('aoMsg').textContent = await r.text();
    $('aoMsg').classList.add('sf-error');
  }
});
function showArtnetStatus(st) {
  const on = !!st && st.kind === 'artnet';
  const warning = on ? st.warning : null;
  $('sdPill').hidden = !warning;
  if (warning) $('sdPill').title = warning + ' — RÉGLAGES › Carte SD ShowNET pour lui donner un numéro.';
  const live = $('anLive');
  live.hidden = !on;
  if (!on) return;
  const lit = st.intensity > 0 && st.pattern > 0;
  live.dataset.lit = lit; live.dataset.warn = !!warning;
  live.textContent = (lit ? `Envoyé : figure ${String(st.pattern).padStart(3, '0')}, intensité ${st.intensity}/255`
    : 'Envoyé : intensité 0, figure 0 (noir)') + ` · univers ${st.universe}, adresse ${st.dmx_address}`
    + (warning ? ' — ' + warning : '') + (st.error ? ' — ' + st.error : '');
}
// ---------- Sortie DMX (T-453): haze machine and simple lights ----------
// The settings in RÉGLAGES (the node's address is typed, never searched
// for); the *Brouillard* box in LIVE › Direct; the values and the sending
// state come with every frame (`f.dmx_out`). Never the laser.
let dmxOut = null;
let doChans = [];
let dmxOutLoading = null;
async function loadDmxOut() {
  try { showDmxOut(await apiJson('GET', '/api/dmx-out', undefined, { cache: 'no-store' })); } catch { /* next time */ }
}
// The named channels, for the timeline's DMX events (loaded once if needed).
function dmxOutNames() { return dmxOut ? dmxOut.config.channels.map(c => c.name) : []; }
function dmxOutChannels() {
  if (!dmxOut && !dmxOutLoading) dmxOutLoading = loadDmxOut().finally(() => { dmxOutLoading = null; assignTlDmxKey(''); });
  return dmxOutLoading;
}
function doOptions(sel, list, cur) {
  sel.replaceChildren(...list.map(([v, t]) => { const o = document.createElement('option'); o.value = v; o.textContent = t; return o; }));
  sel.value = list.some(([v]) => v === cur) ? cur : '';
}
function showDmxOut(j) {
  dmxOut = j;
  const c = j.config;
  $('doEnabled').checked = c.enabled;
  $('doProtocol').value = c.protocol;
  $('doUniverse').value = c.universe;
  const ifs = j.interfaces.map(i => [i.ip, `${i.ip} (${i.name})`]);
  if (c.interface && !ifs.some(([ip]) => ip === c.interface)) ifs.push([c.interface, c.interface + ' (absente)']);
  doOptions($('doInterface'), [['', 'Choisir…'], ...ifs], c.interface || '');
  $('doTarget').value = c.target || '';
  doChans = c.channels.map(ch => ({ ...ch }));
  renderDoChannels();
  $('doPumpLevel').value = c.haze.pump_level;
  $('doMaxBurst').value = c.haze.max_burst_s;
  doHazeSelects(c.haze.pump || '', c.haze.fan || '');
  showDmxOutLive(j.summary);
}
function doHazeSelects(pump, fan) {
  const names = doChans.map(ch => ch.name.trim()).filter(Boolean).map(n => [n, n]);
  doOptions($('doPump'), [['', 'Aucun'], ...names], pump);
  doOptions($('doFan'), [['', 'Aucun'], ...names], fan);
}
function renderDoChannels() {
  const t = $('doChannels');
  const head = document.createElement('tr');
  for (const h of ['Nom', 'Adresse', 'Valeur de repos', '']) { const th = document.createElement('th'); th.textContent = h; head.append(th); }
  const rows = doChans.map((ch, i) => {
    const tr = document.createElement('tr'); tr.dataset.i = i;
    const cell = (k, type, label, attrs) => {
      const td = document.createElement('td'), inp = document.createElement('input');
      inp.type = type; inp.dataset.k = k; inp.value = ch[k]; inp.setAttribute('aria-label', label);
      for (const [a, v] of Object.entries(attrs)) inp.setAttribute(a, v);
      td.append(inp); return td;
    };
    const del = document.createElement('button');
    del.dataset.del = i; del.textContent = '✕'; del.title = 'Supprimer ce canal'; del.setAttribute('aria-label', `Supprimer le canal ${i + 1}`);
    const tdDel = document.createElement('td'); tdDel.append(del);
    tr.append(cell('name', 'text', `Nom du canal ${i + 1}`, { maxlength: 40, autocomplete: 'off' }), cell('address', 'number', `Adresse du canal ${i + 1}`, { min: 1, max: 512, step: 1 }),
      cell('rest', 'number', `Valeur de repos du canal ${i + 1}`, { min: 0, max: 255, step: 1 }), tdDel);
    return tr;
  });
  if (!rows.length) {
    const tr = document.createElement('tr'), td = document.createElement('td');
    td.colSpan = 4; td.className = 'hint'; td.textContent = 'Aucun canal : « + Canal » pour en nommer un (16 au plus).';
    tr.append(td); rows.push(tr);
  }
  t.replaceChildren(head, ...rows);
  $('doAddCh').disabled = doChans.length >= (dmxOut ? dmxOut.max_channels : 16);
}
$('doChannels').addEventListener('input', e => {
  const tr = e.target.closest('tr[data-i]'); if (!tr) return;
  const ch = doChans[+tr.dataset.i], k = e.target.dataset.k;
  ch[k] = k === 'name' ? e.target.value : e.target.value === '' ? '' : Math.round(+e.target.value);
  if (k === 'name') doHazeSelects($('doPump').value, $('doFan').value);
});
$('doChannels').addEventListener('click', e => {
  const b = e.target.closest('button[data-del]'); if (!b) return;
  doChans.splice(+b.dataset.del, 1);
  renderDoChannels(); doHazeSelects($('doPump').value, $('doFan').value);
});
$('doAddCh').addEventListener('click', () => {
  const used = new Set(doChans.map(c => +c.address));
  let address = 1; while (used.has(address) && address < 512) address++;
  doChans.push({ name: `Canal ${doChans.length + 1}`, address, rest: 0 });
  renderDoChannels(); doHazeSelects($('doPump').value, $('doFan').value);
});
function doBody() {
  const int = (v, d) => { const n = Math.round(+v); return v === '' || !Number.isFinite(n) ? d : n; };
  return {
    enabled: $('doEnabled').checked,
    protocol: $('doProtocol').value,
    universe: int($('doUniverse').value, 1),
    interface: $('doInterface').value || null,
    target: $('doTarget').value.trim() || null,
    channels: doChans.map(ch => ({ name: String(ch.name).trim(), address: int(ch.address, 0), rest: int(ch.rest, 0) })),
    haze: { pump: $('doPump').value || null, fan: $('doFan').value || null, pump_level: int($('doPumpLevel').value, 255), max_burst_s: +$('doMaxBurst').value || 10 },
  };
}
async function saveDmxOut() {
  const r = await api('POST', '/api/dmx-out', doBody(), { quiet: true });
  if (r.ok) {
    showDmxOut(await r.json());
    $('doMsg').textContent = dmxOut.config.enabled ? 'Enregistré : sortie DMX activée.' : 'Enregistré (sortie DMX désactivée).';
    $('doMsg').classList.remove('sf-error');
  } else {
    $('doMsg').textContent = await r.text();
    $('doMsg').classList.add('sf-error');
    if (dmxOut) $('doEnabled').checked = dmxOut.config.enabled;
  }
}
$('doSave').addEventListener('click', saveDmxOut);
$('doEnabled').addEventListener('change', saveDmxOut);
$('dmxOutPanel').addEventListener('toggle', () => { if ($('dmxOutPanel').open) loadDmxOut(); });
// CITP (T-454): names and pictures of the cue grid for a lighting
// console's media browser. Read-only for the console; the state comes
// with every frame (`f.citp`).
let citpCfg = null;
async function loadCitp() {
  try { showCitp(await apiJson('GET', '/api/citp', undefined, { cache: 'no-store' })); } catch { /* next time */ }
}
function showCitp(j) {
  citpCfg = j.config;
  $('citpEnabled').checked = j.config.enabled;
  const ifs = [['', '127.0.0.1 (ce Mac seulement)'], ...j.interfaces.filter(i => i.ip !== '127.0.0.1').map(i => [i.ip, `${i.ip} (${i.name})`])];
  const cur = j.config.interface === '127.0.0.1' ? '' : j.config.interface || '';
  if (cur && !ifs.some(([ip]) => ip === cur)) ifs.push([cur, cur + ' (absente)']);
  doOptions($('citpInterface'), ifs, cur);
  $('citpName').value = j.config.server_name;
  $('citpThumb').value = j.config.thumb_max_px;
  showCitpLive({ enabled: j.config.enabled, ...j.status });
}
function showCitpLive(c) {
  if (!c) return;
  const st = $('citpState');
  st.textContent = c.error ? '⚠ ' + c.error
    : c.listening ? `À l'écoute sur ${c.local || '?'}${c.announcing ? ' · annoncé sur le réseau' : ''}`
    : c.enabled ? 'Démarrage…' : 'Arrêté';
  st.className = 'ni-state' + (c.error ? ' warn' : c.listening ? ' ok' : '');
  const peers = c.peers || [];
  const key = peers.join('\n');
  if ($('citpPeers').dataset.key !== key) {
    $('citpPeers').dataset.key = key;
    $('citpPeers').replaceChildren(...(peers.length ? peers : ['Aucune']).map(p => { const li = document.createElement('li'); li.textContent = p; return li; }));
  }
  $('citpServed').textContent = c.served_thumbs ?? 0;
  $('citpIgnored').textContent = c.ignored_messages ?? 0;
}
async function saveCitp() {
  const n = Math.round(+$('citpThumb').value);
  const body = {
    enabled: $('citpEnabled').checked,
    interface: $('citpInterface').value || null,
    server_name: $('citpName').value,
    thumb_max_px: Number.isFinite(n) && $('citpThumb').value !== '' ? n : 128,
  };
  const r = await api('POST', '/api/citp', body, { quiet: true });
  if (r.ok) {
    showCitp(await r.json());
    $('citpMsg').textContent = citpCfg.enabled ? 'Enregistré : cues annoncés à la console.' : 'Enregistré (annonce désactivée).';
    $('citpMsg').classList.remove('sf-error');
  } else {
    $('citpMsg').textContent = await r.text();
    $('citpMsg').classList.add('sf-error');
    if (citpCfg) $('citpEnabled').checked = citpCfg.enabled;
  }
}
$('citpSave').addEventListener('click', saveCitp);
$('citpEnabled').addEventListener('change', saveCitp);
$('citpPanel').addEventListener('toggle', () => { if ($('citpPanel').open) loadCitp(); });
// Every frame: the witness in RÉGLAGES and the haze box in LIVE.
function showDmxOutLive(d) {
  if (!d) return;
  const live = $('doLive');
  live.dataset.on = !!d.sending;
  live.dataset.warn = !!(d.blocked || d.error || d.warning || d.estop || (d.enabled && d.held));
  live.textContent = (d.sending ? `Envoi : ${d.rate_hz} trames/s` : 'Arrêté')
    + (d.estop ? " — arrêt d'urgence : tous les canaux à 0" : d.enabled && d.held ? ' — ' + d.held : '') + (d.blocked ? ' — ' + d.blocked : '')
    + (d.error ? ' — ' + d.error : '') + (d.warning ? ' — ' + d.warning : '');
  $('hazeBox').hidden = !d.haze.available;
  $('hazeFanRow').hidden = !d.haze.has_fan;
  if (!hazeHeld) { $('hazeBtn').classList.toggle('active', !!d.haze.held); $('hazeBtn').setAttribute('aria-pressed', !!d.haze.held); }
  $('hazeState').textContent = d.estop ? "Arrêt d'urgence : brouillard coupé" : d.haze.pump_on ? 'Pompe en marche' : d.haze.held ? "Durée max d'un jet atteinte : relâchez" : !d.enabled ? 'Sortie DMX désactivée' : '';
  if (document.activeElement !== $('hazeFan')) $('hazeFan').value = Math.round(d.haze.fan * 100);
  $('hazeFanV').textContent = `${$('hazeFan').value} %`;
}
// The haze button (and Alt+B): held while the pointer or the key is down;
// let go on up, cancel, lost capture and when the window loses focus.
let hazeHeld = false;
function setHazeHeld(on) {
  if (on === hazeHeld) return;
  hazeHeld = on;
  $('hazeBtn').classList.toggle('active', on); $('hazeBtn').setAttribute('aria-pressed', on);
  api('POST', '/api/dmx-out/haze', { on }, { quiet: true }).catch(() => {});
}
$('hazeBtn').addEventListener('pointerdown', e => { if (e.button === 0) { $('hazeBtn').setPointerCapture?.(e.pointerId); setHazeHeld(true); } });
['pointerup', 'pointercancel', 'lostpointercapture'].forEach(ev => $('hazeBtn').addEventListener(ev, () => setHazeHeld(false)));
window.addEventListener('blur', () => setHazeHeld(false));
$('hazeFan').addEventListener('input', () => {
  $('hazeFanV').textContent = `${$('hazeFan').value} %`;
  api('POST', '/api/dmx-out/haze', { fan: +$('hazeFan').value / 100 }, { quiet: true }).catch(() => {});
});

// While RÉGLAGES is shown, the state line follows connections, losses and
// the retry countdown.
setInterval(() => { if (activeWs === 'settings') reloadDevices(); }, 500);
// The output in the top bar leads to this section.
$('output').addEventListener('click', () => {
  setWorkspace('settings');
  reloadDevices();
  $('outputsPanel').scrollIntoView({ block: 'start' });
});
// ---------- damaged data files (T-402) ----------
// One banner per unacknowledged file (up to 2), else one summary with
// « Détails ». A safety file's banner is red: arming waits for « J'ai compris ».
let dataWarnOpen = false;
function showDataWarnings(dw) {
  const list = ((dw && dw.warnings) || []).filter(w => !w.acknowledged);
  const box = $('dataWarn');
  box.hidden = !list.length;
  noticeCondition('data', list.length > 0, { level: list.some(w => w.safety) ? 'error' : 'warning', source: 'Réglages', toast: false,
    text: list.length === 1 ? list[0].message : `${list.length} fichiers de données illisibles, valeurs par défaut utilisées` });
  box.innerHTML = '';
  if (!list.length) return;
  const blocked = ' L\'armement est bloqué tant que vous n\'avez pas validé.';
  const line = (text, safety, ack) => {
    const d = document.createElement('div');
    d.className = 'dw' + (safety ? ' safety' : '');
    const span = document.createElement('span'); span.textContent = text + (safety ? blocked : '');
    const b = document.createElement('button'); b.textContent = 'J\'ai compris';
    b.onclick = () => ackDataWarning(ack);
    d.append(span, b);
    box.appendChild(d);
    return d;
  };
  if (list.length <= 2) {
    list.forEach(w => line(w.message, w.safety, { file: w.file }).dataset.file = w.file);
    return;
  }
  const d = line(`${list.length} fichiers illisibles, valeurs par défaut utilisées.`, list.some(w => w.safety), { all: true });
  const more = document.createElement('button'); more.textContent = dataWarnOpen ? 'Masquer' : 'Détails';
  more.onclick = () => { dataWarnOpen = !dataWarnOpen; showDataWarnings(dw); };
  d.insertBefore(more, d.lastChild);
  if (dataWarnOpen) {
    const ul = document.createElement('ul');
    list.forEach(w => { const li = document.createElement('li'); li.textContent = w.message; ul.appendChild(li); });
    d.appendChild(ul);
  }
}
async function ackDataWarning(body) {
  const r = await api('POST', '/api/data/warnings/ack', body);
  if (r.ok) { showDataWarnings(await r.json()); reloadSafety(); }
}
async function dataWarnLoop() {
  try { showDataWarnings(await apiJson('GET', '/api/data/warnings')); } catch { /* server gone */ }
  setTimeout(dataWarnLoop, 5000);
}

// ---------- mode de sortie (T-255) ----------
// « Au-dessus du public » is the default and comes back on every start.
// « Balayage public » opens a step-by-step wizard: every condition with a
// green tick or a red cross and a link to its setting, then the phrase.
let audience = null, audWizardOpen = false;
function showAudienceMode(a) {
  const was = audienceMode && audienceMode.scanning;
  assignAudienceMode(a || null);
  const on = !!(a && a.scanning);
  $('audPill').hidden = !on;
  $('stage').classList.toggle('aud-scan', on);
  $('audSeg').querySelectorAll('button').forEach(b => b.classList.toggle('active', (b.dataset.aud === 'scanning') === on));
  if (!!was !== on) { reloadAudience(); safetyTo3d(); if (lastPoints && show2d) draw(lastShown); }
}
async function reloadAudience() {
  try {
    const r = await api('GET', '/api/audience');
    if (r.ok) showAudience(await r.json());
  } catch { /* server gone */ }
}
function showAudience(a) {
  audience = a;
  showAudienceMode({ mode: a.mode, scanning: a.scanning, since: a.since });
  $('audInfo').textContent = a.scanning
    ? `Balayage public depuis ${clock(a.since)} : les zones Public reçoivent leur lumière atténuée. Le logiciel ne détecte pas une panne des galvos.`
    : 'Au-dessus du public : rien n\'est allumé dans les zones Public.'
      + (a.protected ? '' : ' ⚠ Aucune zone Public ni horizon complet : avec une vraie sortie, l\'armement est refusé.');
  $('audPhraseText').textContent = a.phrase;
  if (a.scanning) audWizardOpen = false;
  $('audWizard').hidden = !audWizardOpen;
  const ol = $('audSteps');
  ol.innerHTML = '';
  for (const c of a.conditions) {
    const li = document.createElement('li');
    li.className = c.ok ? 'ok' : 'bad';
    li.dataset.cond = c.id;
    const mark = document.createElement('span'); mark.className = 'mark'; mark.textContent = c.ok ? '✓' : '✗';
    mark.setAttribute('aria-label', c.ok ? 'rempli' : 'manquant');
    const txt = document.createElement('span'); txt.className = 'txt'; txt.textContent = c.label_fr;
    const detail = document.createElement('span'); detail.className = 'detail'; detail.textContent = c.detail_fr;
    txt.append(detail);
    const go = document.createElement('button'); go.textContent = 'Régler'; go.onclick = () => audGoTo(c.link);
    li.append(mark, txt, go);
    ol.append(li);
  }
  const ready = a.conditions.every(c => c.ok);
  $('audUnlock').disabled = !ready || a.armed;
  $('audUnlock').title = a.armed ? 'Désarmez le laser d\'abord' : ready ? '' : 'Conditions non remplies';
}
/** Opens the setting a condition links to. */
function audGoTo(id) {
  const el = $(id);
  if (!el) return;
  setWorkspace('settings');
  const d = el.tagName === 'DETAILS' ? el : el.closest('details');
  if (d) d.open = true;
  el.scrollIntoView({ block: 'center', behavior: 'smooth' });
  if (el.focus && el.tagName !== 'DETAILS') el.focus({ preventScroll: true });
}
$('audSeg').addEventListener('click', async e => {
  const b = e.target.closest('button');
  if (!b) return;
  $('audError').textContent = '';
  if (b.dataset.aud === 'above') {
    audWizardOpen = false;
    const r = await api('POST', '/api/audience', { mode: 'above', source: 'ui' });
    if (r.ok) showAudience(await r.json());
  } else if (!(audienceMode && audienceMode.scanning)) {
    audWizardOpen = true; $('audPhrase').value = '';
    await reloadAudience();
  }
});
$('audCancel').onclick = () => { audWizardOpen = false; $('audWizard').hidden = true; $('audPhrase').value = ''; $('audError').textContent = ''; };
$('audUnlock').onclick = async () => {
  const r = await api('POST', '/api/audience', { mode: 'scanning', phrase: $('audPhrase').value, source: 'ui' }, { quiet: true });
  $('audPhrase').value = '';
  if (r.ok) { $('audError').textContent = ''; audWizardOpen = false; showAudience(await r.json()); return; }
  if (r.status === 409) {
    const j = await r.json();
    showAudience(j);
    $('audError').textContent = j.error + (j.missing ? ' : ' + j.missing.join(' · ') : '');
  } else $('audError').textContent = await r.text();
};
// While the wizard is open its ticks follow the settings changed elsewhere.
setInterval(() => { if (audWizardOpen && activeWs === 'settings' && !document.hidden) reloadAudience(); }, 1000);

export { dataWarnLoop, dmxOutChannels, dmxOutNames, frNumber, loadArtnet, loadAudioOut, loadDmxOut, outputCfg,
  reloadAudience, reloadDevices, reloadOutColor, reloadOutputs, scanAlertFps, setHazeHeld,
  showArtnetStatus, showAudienceMode, showCitpLive, showDataWarnings, showDmxOutLive,
  showFixedRateStatus };
