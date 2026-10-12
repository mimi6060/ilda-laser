// Laser Studio — presence.js (T-435).
// Sections (as in the former index.html script):
//   operator presence (T-252)
//   galvo simulation (T-280)
//   preview
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './laser.js';
import { $, api, apiJson, assignPageLeaving, noticeCondition, pps, sendLatest } from './core.js';
import { showPoints } from './state.js';
import { lastPoints } from './view3d.js';
import { drawPsnOverlay, drawSafetyOverlay, drawTargetsOverlay } from './targets.js';
import { outputCfg } from './outputs.js';
import { reloadVenue } from './venue.js';

// ---------- operator presence (T-252) ----------
// Every page beats POST /api/heartbeat every 500 ms. With no beat from any
// page for ui_timeout_ms (2 s), the server disarms (« Interface perdue »)
// and ends held flashes. The beat comes from a Worker, whose timers are
// not throttled like a background tab's, but only while this page answers
// it: a frozen page stops vouching for itself.
const clientId = globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`;
let presence = { hold_to_run: false, hold_key: 'ShiftRight' }, holdDown = false, pickingHoldKey = false;
function assignPickingHoldKey(v) { return (pickingHoldKey = v); }
const downCodes = new Set(); // KeyboardEvent.code of the keys down now
const KEY_NAMES = { ShiftRight: 'Maj droite', ShiftLeft: 'Maj gauche', ControlRight: 'Ctrl droite', ControlLeft: 'Ctrl gauche',
  AltRight: 'Alt droite', AltLeft: 'Alt gauche', MetaRight: 'Cmd droite', MetaLeft: 'Cmd gauche' };
const keyName = code => KEY_NAMES[code] ?? code.replace(/^(Key|Digit)/, '');
// Shift as a modifier (Shift+Escape = plain disarm, Shift+cue = flash),
// not counting a Shift key that is the hold key.
function shiftMod(e) {
  if (!e.shiftKey) return false;
  if (!presence.hold_to_run || !presence.hold_key.startsWith('Shift')) return true;
  return [...downCodes].some(c => c.startsWith('Shift') && c !== presence.hold_key);
}
const beatBody = gone => JSON.stringify(gone ? { client_id: clientId, gone: true }
  : { client_id: clientId, visible: document.visibilityState === 'visible', hold: holdDown });
function beatNow() { api('POST', '/api/heartbeat', beatBody(), { keepalive: true }).catch(() => {}); }
const HB_WORKER = `let s = null, at = 0;
onmessage = e => { s = e.data; at = Date.now(); };
setInterval(() => {
  postMessage(0);
  if (s && Date.now() - at < 1500) fetch(s.url, { method: 'POST', body: s.body }).catch(() => {});
}, 500);`;
try {
  const hb = new Worker(URL.createObjectURL(new Blob([HB_WORKER], { type: 'text/javascript' })));
  hb.onmessage = () => hb.postMessage({ url: location.origin + '/api/heartbeat', body: beatBody() });
} catch (e) {
  console.warn('heartbeat worker unavailable, beating from the page:', e);
  setInterval(beatNow, 500);
}
beatNow();
document.addEventListener('visibilitychange', () => { if (document.visibilityState !== 'visible') setHold(false); beatNow(); });
// Closing the page: gone at once, without waiting for the timeout.
window.addEventListener('pagehide', () => navigator.sendBeacon('/api/heartbeat', beatBody(true)));
window.addEventListener('pageshow', e => { if (e.persisted) { assignPageLeaving(false); beatNow(); } });

function setHold(down) {
  if (holdDown === down) return;
  holdDown = down;
  beatNow(); // don't wait for the next beat: release = black now
}
function showPresence(f) {
  const p = f.presence;
  if (!p) return;
  presence.hold_to_run = p.hold_to_run; presence.hold_key = p.hold_key;
  if (!p.hold_to_run) holdDown = false;
  const led = $('presenceLed');
  led.classList.toggle('ok', p.ui_alive);
  led.textContent = libelle(p.ui_alive ? 'barre.presence.present' : 'barre.presence.absent');
  const stalled = !f.engine_ok || (!f.armed && f.arm?.last_disarm?.reason === 'engine_stall');
  $('engineLed').classList.toggle('ok', !stalled);
  $('engineLed').classList.toggle('bad', stalled);
  $('engineLed').textContent = libelle(stalled ? 'barre.moteur.bloque' : 'barre.moteur');
  const banner = $('holdBanner');
  banner.hidden = !p.hold_to_run;
  banner.classList.toggle('emitting', p.hold_ok);
  banner.textContent = p.hold_ok ? libelle('securite.maintien.actif') : libelle('securite.maintien.demande', { touche: keyName(p.hold_key) });
  if (!pickingHoldKey) $('holdKeyLabel').textContent = keyName(p.hold_key);
}
// RÉGLAGES › Général: keep the Mac awake during a show (T-412).
function showKeepAwake(k) {
  if (!k) return;
  if (document.activeElement !== $('kaEnabled')) $('kaEnabled').checked = k.enabled;
  const st = $('kaStatus');
  st.classList.toggle('sf-warn', !!k.error);
  st.textContent = k.error ? 'Blocage de la veille impossible' : k.held ? 'Veille bloquée' : 'Veille autorisée';
  st.title = k.error || '';
}
$('kaEnabled').onchange = async e => {
  const r = await api('POST', '/api/keep_awake', { enabled: e.target.checked }, { quiet: true });
  if (r.ok) showKeepAwake(await r.json()); else alert(await r.text());
};
async function loadPresence() {
  const { settings } = await apiJson('GET', '/api/presence');
  showPresenceSettings(settings);
}
function showPresenceSettings(st) {
  presence = { ...presence, ...st };
  if (document.activeElement !== $('uiTimeout')) $('uiTimeout').value = st.ui_timeout_ms;
  $('holdToRun').checked = st.hold_to_run;
  $('holdKeyLabel').textContent = keyName(st.hold_key);
}
async function savePresence(patch) {
  const { settings } = await apiJson('GET', '/api/presence');
  const r = await api('POST', '/api/presence', { ...settings, ...patch });
  if (r.ok) showPresenceSettings(await r.json());
}
function pickHoldKey(code) {
  pickingHoldKey = false;
  $('holdKeyPick').textContent = 'Changer…';
  savePresence({ hold_key: code }); // Space and Escape are refused (back to Maj droite)
}
$('uiTimeout').addEventListener('change', e => savePresence({ ui_timeout_ms: Math.round(+e.target.value) || 2000 }));
$('holdToRun').addEventListener('change', e => savePresence({ hold_to_run: e.target.checked }));
$('holdKeyPick').addEventListener('click', e => {
  pickingHoldKey = true;
  e.target.textContent = 'Appuyez sur une touche…';
  $('holdKeyLabel').textContent = '…';
});

// ---------- galvo simulation (T-280) ----------
// RÉGLAGES › Simulation de l'aperçu. DISPLAY ONLY: the previews (2D, 3D)
// draw the frame as scanners would trace it, the server keeps nothing but
// the settings (venue.json › render). The frame sent to the output,
// /api/frame and every export are the same with the simulation on or off
// (safety_invariants.rs checks it). An approximation: a real projector
// can round corners more, or flicker sooner, than shown here.
//
// The scanner is a second-order low-pass filter (damping 0.7, so about
// 4.6 % overshoot) whose 10–90 % rise time is *Réponse (ms)*, fed one
// point per 1/pps second, held between points as a DAC does. Exact
// discretisation: any point rate, any response, no sub-steps.
const GALVO_ZETA = 0.7, GALVO_RISE = 2.1376; // ωn · (10–90 % rise time) at ζ = 0.7
const GALVO_FLICKER_S = 1 / 25;              // a frame longer than this flickers
const GALVO = { enabled: false, response_ms: 0.3, divergence_mrad: 1.0, gain: 1, long: false };
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
/**
 * The positions a scanner reaches, one per point: [x, y, r, g, b] with the
 * colours of the point sent. `cyclic`: the frame repeats (the filter's
 * steady state, from two passes); otherwise it starts at rest on `from`.
 */
function galvoFilter(points, pps, responseMs, { cyclic = true, from = null } = {}) {
  const n = points.length;
  if (!n || !(pps > 0) || !(responseMs > 0)) return points;
  const dt = 1 / pps, wn = GALVO_RISE / (responseMs / 1000);
  const s = GALVO_ZETA * wn, wd = wn * Math.sqrt(1 - GALVO_ZETA * GALVO_ZETA);
  const ex = Math.exp(-s * dt), c = Math.cos(wd * dt), sn = Math.sin(wd * dt) / wd;
  // (error, speed) after dt, the input held: error = position - point.
  const a11 = ex * (c + s * sn), a12 = ex * sn, a21 = -ex * wn * wn * sn, a22 = ex * (c - s * sn);
  const start = from || points[n - 1];
  let x = start[0], y = start[1], vx = 0, vy = 0;
  const out = new Array(n);
  for (let pass = cyclic ? 0 : 1; pass < 2; pass++) {
    for (let i = 0; i < n; i++) {
      const p = points[i], ex0 = x - p[0], ey0 = y - p[1];
      x = p[0] + a11 * ex0 + a12 * vx; vx = a21 * ex0 + a22 * vx;
      y = p[1] + a11 * ey0 + a12 * vy; vy = a21 * ey0 + a22 * vy;
      if (pass) out[i] = [Math.max(-1.5, Math.min(1.5, x)), Math.max(-1.5, Math.min(1.5, y)), p[2], p[3], p[4]];
    }
  }
  return out;
}
/** The time to trace a frame of `n` points, seconds. */
const galvoFrameS = (n, pps) => (pps > 0 ? n / pps : 0);
/** The point rate the previews simulate: the output's, 30 kpps if unknown. */
const galvoPps = () => (pps > 0 ? pps : 30000);
/**
 * What the previews draw for a frame: its galvo simulation when on, else
 * the frame itself. Also sets the flicker gain (between 0.7 and 1, slow:
 * 2 Hz, so the preview never strobes; none with reduced motion) and the
 * « Image trop longue » flag.
 */
function galvoShown(points) {
  const on = GALVO.enabled;
  GALVO.long = on && galvoFrameS(points.length, galvoPps()) > GALVO_FLICKER_S;
  GALVO.gain = GALVO.long && !reducedMotion.matches ? 1 - 0.3 * (0.5 + 0.5 * Math.sin(performance.now() / 1000 * 2 * Math.PI * 2)) : 1;
  return on ? galvoFilter(points, galvoPps(), GALVO.response_ms) : points;
}
function showGalvo() {
  $('galvoOn').checked = GALVO.enabled;
  if (document.activeElement !== $('galvoResp')) $('galvoResp').value = GALVO.response_ms;
  $('galvoRespV').textContent = GALVO.response_ms.toFixed(2).replace('.', ',') + ' ms';
  if (document.activeElement !== $('galvoDiv')) $('galvoDiv').value = GALVO.divergence_mrad;
  $('galvoDivV').textContent = GALVO.divergence_mrad.toFixed(1).replace('.', ',') + ' mrad';
  $('galvoBadge').hidden = !GALVO.enabled;
  $('galvoLong').hidden = !GALVO.long;
  const sheet = outputCfg && outputCfg.projector && outputCfg.projector.divergence_mrad;
  $('galvoSheet').hidden = !(sheet > 0);
  $('galvoSheet').textContent = sheet > 0 ? `Reprendre la fiche projecteur (${String(sheet).replace('.', ',')} mrad)` : '';
}
/** venue.json › render, from GET /api/venue or a post. */
function takeGalvo(r) {
  if (!r) return;
  if (r.galvo) { GALVO.enabled = !!r.galvo.enabled; GALVO.response_ms = +r.galvo.response_ms || 0.3; }
  if (Number.isFinite(+r.divergence_mrad)) GALVO.divergence_mrad = +r.divergence_mrad;
  showGalvo();
  if (lastPoints) showPoints(lastPoints);
}
// A slider being dragged: latest wins (T-484); the answer is the clamped value.
async function sendGalvo(patch) {
  Object.assign(GALVO, 'enabled' in patch ? { enabled: patch.enabled } : {},
    'response_ms' in patch ? { response_ms: patch.response_ms } : {}, 'divergence_mrad' in patch ? { divergence_mrad: patch.divergence_mrad } : {});
  showGalvo();
  if (lastPoints) showPoints(lastPoints);
  const r = await sendLatest('galvo:' + Object.keys(patch).join(), 'POST', '/api/venue/sim', patch, { quiet: true });
  if (!r) return; // replaced by a newer value
  if (r.ok) takeGalvo((await r.json()).render);
  else { $('galvoError').textContent = await r.text(); reloadVenue(); }
}
$('galvoOn').onchange = e => sendGalvo({ enabled: e.target.checked });
$('galvoResp').oninput = e => { const v = +e.target.value; if (Number.isFinite(v)) sendGalvo({ response_ms: v }); };
$('galvoDiv').oninput = e => { const v = +e.target.value; if (Number.isFinite(v)) sendGalvo({ divergence_mrad: v }); };
$('galvoSheet').onclick = () => { const d = outputCfg && outputCfg.projector && outputCfg.projector.divergence_mrad; if (d > 0) sendGalvo({ divergence_mrad: d }); };
// For the e2e tests and the console: the pure filter, read only.
window.__galvo = { filter: galvoFilter, frameS: galvoFrameS, settings: () => ({ ...GALVO }) };

// ---------- preview ----------
const cv = $('preview'), g = cv.getContext('2d');
function draw(points, c = cv) {
  // Another canvas (« Préparé », T-274) draws without the output's overlays.
  const out = c === cv, g = out ? cv.getContext('2d') : c.getContext('2d');
  const W = c.width, H = c.height;
  g.fillStyle = '#000'; g.fillRect(0, 0, W, H);
  g.strokeStyle = '#ffffff10'; g.lineWidth = 1;
  g.beginPath(); g.moveTo(W / 2, 0); g.lineTo(W / 2, H); g.moveTo(0, H / 2); g.lineTo(W, H / 2); g.stroke();
  const px = p => [(p[0] + 1) / 2 * W, (1 - (p[1] + 1) / 2) * H];
  if (out) drawSafetyOverlay(W, H, px);
  if (out) drawTargetsOverlay(px);
  if (out) drawPsnOverlay(px);
  // Boost dim colors so a low-brightness look is still visible on screen.
  // T-280: an overlong frame flickers a little when the galvos are simulated.
  const gain = out ? GALVO.gain : 1;
  const col = (p, a) => `rgba(${[2, 3, 4].map(i => Math.round(255 * Math.sqrt(p[i]))).join(',')},${a * gain})`;
  for (const [width, alpha] of [[9, 0.18], [2.5, 1]]) {
    g.lineWidth = width; g.lineCap = 'round';
    for (let i = 1; i < points.length; i++) {
      const a = points[i - 1], b = points[i];
      if (a[2] + a[3] + a[4] === 0 || b[2] + b[3] + b[4] === 0) continue;
      const [x1, y1] = px(a), [x2, y2] = px(b);
      g.strokeStyle = col(b, alpha);
      g.beginPath(); g.moveTo(x1, y1); g.lineTo(x2 + 0.01, y2); g.stroke();
    }
  }
}

// Which output the frames go to (T-300), as a coloured pill in the header.
const OUTPUT_TITLES = {
  ponk: 'Sortie PONK : les tracés partent vers MadMapper/MadLaser, qui pilote le ShowNET. Laser désarmé = trames vides envoyées en continu.',
  dac: 'Sortie directe vers un DAC laser (Helios USB / IDN / Ether Dream).',
  artnet: 'Sortie ShowNET carte SD (Art-Net) : le boîtier joue lui-même les fichiers de sa carte ; le studio envoie le numéro du cue et l\'intensité. Désarmé = intensité 0 et figure 0 en continu. L\'arrêt matériel du projecteur reste la vraie sécurité.',
  test: 'Sortie de test (fichier journal, aucun laser).',
  preview: 'Aperçu seulement : rien ne part vers un laser.',
};
function showOutputKind(f) {
  const el = $('output'), state = f.output_state || 'available';
  const lost = state === 'lost';
  const kind = lost || f.output_error ? 'error' : state === 'connecting' ? 'preview' : f.output_kind || 'preview';
  el.dataset.state = state;
  const title = (lost ? 'Sortie perdue : laser désarmé, reconnexion en cours si elle est activée. ' + (f.output_error || '')
    : f.output_error ? 'Erreur de sortie' : OUTPUT_TITLES[kind] || '') + ' — clic : RÉGLAGES › Sorties et projecteurs';
  if (el.dataset.kind === kind && el.title === title) return;
  el.dataset.kind = kind;
  el.title = title;
}

// Engine link (T-271): no new frame for more than 1 s = an orange warning in the top bar.
let lastFrameAt = performance.now();
function assignLastFrameAt(v) { return (lastFrameAt = v); }
setInterval(() => {
  $('linkLost').hidden = performance.now() - lastFrameAt <= 1000;
  noticeCondition('engine', !$('linkLost').hidden, { level: 'warning', source: 'Studio', text: libelle('barre.liaison_perdue'), toast: false });
}, 250);

export { GALVO, assignLastFrameAt, assignPickingHoldKey, clientId, downCodes, draw, g, galvoFilter,
  galvoPps, galvoShown, keyName, loadPresence, pickHoldKey, pickingHoldKey, presence, setHold,
  shiftMod, showGalvo, showKeepAwake, showOutputKind, showPresence, takeGalvo };
