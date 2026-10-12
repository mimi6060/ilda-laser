// Laser Studio — music.js (T-435).
// Sections (as in the former index.html script):
//   music
//   audio safety (T-245)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './workspaces.js';
import { $, apiJson, post } from './core.js';
import { scenes } from './cuelist.js';
import { esc } from './midi.js';
import { openHelp } from './help.js';

// ---------- music ----------
// Two sources feed the engine (T-230): *Navigateur* = this page's micro,
// analysed here and POSTed to /api/audio (the code below, unchanged in
// substance); *Native* = the studio captures a Mac input itself. The
// panel (T-243) shows what the engine hears, whatever the source, from
// GET /api/audio/state (10/s, 6.7/s with the spectrum) and, for Native,
// GET /api/audio/spectrum (20/s), only while the panel is on screen:
// ≤ 30 requests/s, nothing when it is hidden.
let audio = null, beat = 0, bassAvg = 0, lastBeat = 0, sending = false;

async function listInputs() {
  const devs = (await navigator.mediaDevices.enumerateDevices()).filter(d => d.kind === 'audioinput');
  const sel = $('micSelect'), cur = sel.value;
  sel.innerHTML = '<option value="">Par défaut</option>' + devs.map(d => `<option value="${d.deviceId}">${esc(d.label || 'Entrée ' + d.deviceId.slice(0, 6))}</option>`).join('');
  sel.value = cur;
}

async function startMic() {
  stopMic();
  const deviceId = $('micSelect').value;
  const stream = await navigator.mediaDevices.getUserMedia({ audio: {
    deviceId: deviceId ? { exact: deviceId } : undefined,
    echoCancellation: false, noiseSuppression: false, autoGainControl: false } });
  const ctx = new AudioContext();
  const an = ctx.createAnalyser();
  an.fftSize = 2048; an.smoothingTimeConstant = 0.3;
  ctx.createMediaStreamSource(stream).connect(an);
  audio = { ctx, an, stream, time: new Float32Array(an.fftSize), freq: new Uint8Array(an.frequencyBinCount), fdb: new Float32Array(an.frequencyBinCount) };
  musError = '';
  showMusic();
  await listInputs();
  analyse();
}

function stopMic() {
  if (!audio) return;
  audio.stream.getTracks().forEach(t => t.stop());
  audio.ctx.close();
  audio = null;
  showMusic();
}

function analyse() {
  if (!audio) return;
  const { an, time, freq, ctx } = audio, gain = +$('gain').value;
  an.getFloatTimeDomainData(time);
  let sum = 0; for (const v of time) sum += v * v;
  const level = Math.min(1, Math.sqrt(sum / time.length) * gain * 3);
  an.getByteFrequencyData(freq);
  const binHz = ctx.sampleRate / an.fftSize, top = Math.max(2, Math.round(150 / binHz));
  let b = 0; for (let i = 1; i < top; i++) b += freq[i];
  const bass = Math.min(1, b / (top - 1) / 255 * gain * 0.6);

  // Beat: bass clearly above its recent average, at most ~5 per second.
  const now = performance.now();
  bassAvg = bassAvg * 0.95 + bass * 0.05;
  if (bass > bassAvg * 1.35 && bass > 0.12 && now - lastBeat > 200) {
    beat++; lastBeat = now;
    $('beatDot').classList.add('hit'); setTimeout(() => $('beatDot').classList.remove('hit'), 90);
  }

  if (!sending) {
    sending = true;
    post('/api/audio', { level, bass, beat }).finally(() => sending = false);
  }
  setTimeout(analyse, 25);
}

// The 64-band spectrum of this page's analyser (Navigateur source), on the
// server's scale: log bands 20 Hz–20 kHz, the loudest bin of each, −90..0 dB.
function browserSpectrum() {
  const { an, fdb, ctx } = audio;
  an.getFloatFrequencyData(fdb);
  const binHz = ctx.sampleRate / an.fftSize, out = new Array(64);
  for (let i = 0; i < 64; i++) {
    const lo = 20 * 1000 ** (i / 64), hi = 20 * 1000 ** ((i + 1) / 64);
    const a = Math.min(fdb.length - 1, Math.max(1, Math.floor(lo / binHz))), z = Math.min(fdb.length - 1, Math.max(a, Math.ceil(hi / binHz) - 1));
    let m = -Infinity; for (let k = a; k <= z; k++) m = Math.max(m, fdb[k]);
    out[i] = Math.max(0, Math.min(1, (m + 90) / 90));
  }
  return out;
}

const TEMPO_FR = { no_input: "Pas d'entrée", checking: 'Vérification', locked: 'Verrouillé', coasting: 'Maintien', guided: 'Guidé' };
const SECTION_FR = { silence: 'Silence', normal: 'Normal', break: 'Break', buildup: 'Montée', drop: 'Drop' };
const BANDS = ['sub', 'bass', 'low_mid', 'mid', 'high'];
// The last /api/audio/state, the source picked in the panel (Native can be
// picked without listening yet: « Écouter » starts it), the server's source
// last seen (a change made elsewhere is followed), the last counters (a
// light flashes when its counter moves) and since when a running native
// capture has been exactly silent.
let mus = null, musPick = null, musServerSrc = null, musCounters = null, musError = '', musConfig = null, musSentAt = 0;
// T-241: « Son du Mac » is the studio's own capture too (source `system`),
// chosen in the Native device list; `musSys` says whether this build and
// this macOS can run it (from /api/audio/devices).
const MUS_SYSTEM = '@system';
const musNat = src => src === 'native' || src === 'system';
let musSys = null;
const musSens = delta => Math.round(100 * Math.log10(0.3 / delta));      // delta 0.3 … 0.03 ↔ 0 … 100 %
const musDelta = sens => +(0.3 * 10 ** (-sens / 100)).toFixed(4);
const fmtDb = v => (v <= -119.5 ? '−∞' : (v < 0 ? '−' : '') + Math.abs(v).toFixed(1)) + ' dBFS';

function musVisible() {
  return document.visibilityState === 'visible' && $('musPanel').offsetParent !== null;
}

async function setMusSource(src, extra = {}) {
  musServerSrc = src; // ours: not a change made elsewhere
  musSentAt = performance.now(); // answers asked before this are stale
  const ok = await post('/api/audio/config', { source: src, ...extra });
  await loadMusConfig();
  return ok;
}

function pickSource(src) {
  musPick = src;
  musError = '';
  // Only one analysis at a time: the page stops listening unless it is the source.
  if (src !== 'browser') stopMic();
  // Navigateur and Aucune apply at once; Native waits for « Écouter »
  // (that is when macOS may ask for the micro).
  if (src !== 'native') setMusSource(src);
  else loadDevices();
  showMusic();
}

async function listenClick() {
  const src = musPick || 'browser';
  if (src === 'browser') {
    if (audio) stopMic();
    else startMic().catch(e => { musError = 'Micro refusé par le navigateur : ' + e.name; showMusic(); });
  } else if (src === 'native') {
    const device = $('musDevice').value || null;
    // Arrêter: back to the default source, nothing listening.
    if (musNat(musServerSrc)) await setMusSource('browser');
    else if (device === MUS_SYSTEM) await setMusSource('system');
    else await setMusSource('native', { device });
  }
  showMusic();
}

async function loadMusConfig() {
  const asked = performance.now();
  try { musConfig = await apiJson('GET', '/api/audio/config'); } catch { return; }
  if (asked >= musSentAt) followSource(musConfig.source);
  const an = musConfig.analysis || {};
  const focus = document.activeElement;
  $('musAutoGain').checked = !!an.auto_gain;
  if (focus !== $('musGain')) $('musGain').value = an.manual_gain_db ?? 0;
  if (focus !== $('musSilence')) $('musSilence').value = an.silence_db ?? -60;
  if (focus !== $('musSens')) $('musSens').value = musSens(an.onsets?.delta ?? 0.1);
  showMusSafety();
  showMusic();
}

// ---------- audio safety (T-245) ----------
function musSilenceAction() {
  const a = musConfig?.safety?.silence_action ?? 'keep';
  return typeof a === 'string' ? { act: a, scene: '' } : { act: 'calm_look', scene: a.calm_look ?? '' };
}
function showMusSafety() {
  const sf = musConfig?.safety ?? {}, { act, scene } = musSilenceAction();
  if (document.activeElement !== $('musFlash')) $('musFlash').value = sf.max_flash_hz ?? 10;
  $('musFlashV').textContent = (+$('musFlash').value).toLocaleString('fr-FR') + ' / s';
  document.querySelectorAll('#musSilAct button').forEach(b => {
    b.classList.toggle('active', b.dataset.act === act);
    b.setAttribute('aria-checked', String(b.dataset.act === act));
  });
  const sel = $('musCalm');
  const names = scenes.map(sc => sc.name);
  if (scene && !names.includes(scene)) names.push(scene);
  const want = ['', ...names].join('\n');
  if (sel.dataset.names !== want) {
    sel.dataset.names = want;
    sel.innerHTML = '<option value="">Choisir une scène…</option>';
    for (const n of names) { const o = document.createElement('option'); o.value = n; o.textContent = n; sel.appendChild(o); }
  }
  sel.value = scene;
  sel.hidden = act !== 'calm_look';
}
const musSafetyPatch = patch => post('/api/audio/config', { safety: patch }).then(loadMusConfig);
function showAudioGuard(g) {
  if (!g) return;
  const action = { keep: 'look gardé sans modulation', calm_look: 'look calme', blackout: 'sortie éteinte en fondu' }[g.action] ?? g.action;
  $('musGuard').dataset.silent = String(g.silent);
  $('musGuard').textContent = g.silent ? `Silence : ${action}.`
    : g.flash_limited ? `Flashs audio limités à ${g.max_flash_hz} / s.`
    : g.live ? 'Audio actif.' : 'Pas d\'audio : liens audio au neutre.';
}

async function loadDevices() {
  let r; try { r = await apiJson('GET', '/api/audio/devices'); } catch { return; }
  musSys = r.system_audio || null;
  const sel = $('musDevice');
  const cur = musConfig?.source === 'system' ? MUS_SYSTEM : (musConfig?.device ?? (sel.value === MUS_SYSTEM ? '' : sel.value));
  const names = r.devices.map(d => d.name);
  if (cur && cur !== MUS_SYSTEM && !names.includes(cur)) names.push(cur); // the saved one, even unplugged
  const sysOk = !!musSys?.available;
  sel.innerHTML = '<option value="">Entrée par défaut du Mac</option>' + names.map(n => {
    const d = r.devices.find(x => x.name === n);
    return `<option value="${esc(n)}">${esc(n)}${d?.is_default ? ' (par défaut)' : d ? '' : ' (absente)'}</option>`;
  }).join('') + `<option value="${MUS_SYSTEM}"${sysOk ? '' : ' disabled'}>Son du Mac (ce que le Mac joue)${sysOk ? '' : ' : indisponible'}</option>`;
  sel.value = cur || '';
  showMusic();
}

function showMusic() {
  const pick = musPick || 'browser', a = mus;
  document.querySelectorAll('#musSrc button').forEach(b => {
    b.classList.toggle('active', b.dataset.src === pick);
    b.setAttribute('aria-checked', String(b.dataset.src === pick));
  });
  // The permission note, until the native capture is on.
  $('musNativeInfo').hidden = pick !== 'native' || musNat(musServerSrc);
  $('musDevRow').hidden = pick === 'none';
  $('micSelect').hidden = pick !== 'browser';
  $('musDevice').hidden = pick !== 'native';
  // T-241: the way to the Mac's own sound, and why « Son du Mac » is greyed.
  $('musSysRow').hidden = pick !== 'native';
  const sysPicked = $('musDevice').value === MUS_SYSTEM;
  $('musSysNote').hidden = pick !== 'native' || !(sysPicked || (musSys && !musSys.available));
  $('musSysNote').textContent = sysPicked
    ? 'Son du Mac : le studio écoute ce que le Mac joue. macOS demande l\'autorisation d\'enregistrer l\'audio du système au premier « Écouter ».'
    : musSys && !musSys.available ? `Son du Mac indisponible : ${musSys.reason}.` : '';
  $('musNativeSettings').hidden = pick === 'browser';
  $('musBrowserSettings').hidden = pick !== 'browser';
  const listening = pick === 'browser' ? !!audio : pick === 'native' && musNat(musServerSrc);
  $('micBtn').hidden = pick === 'none';
  $('micBtn').textContent = listening ? 'Arrêter' : 'Écouter';
  $('micBtn').classList.toggle('primary', !listening);
  $('micBtn').title = pick === 'native' ? (listening ? 'Arrêter la capture native (retour à la source Navigateur, micro coupé)' : 'Le studio écoute l\'entrée choisie ; macOS peut demander l\'autorisation d\'accès au micro')
    : 'Le navigateur écoute le micro de cette page';

  // Status line.
  let text = '', cls = '';
  if (musError) { text = musError; cls = 'err'; }
  else if (pick === 'none') text = 'Aucune source : le laser ne réagit plus à la musique.';
  else if (pick === 'browser') text = audio ? 'Le micro de cette page est analysé par le navigateur.' : 'Micro de la page coupé. « Écouter » pour que le laser suive la musique.';
  else if (!musNat(musServerSrc)) text = 'Choisissez l\'entrée puis « Écouter ».';
  else if (a && a.state === 'disabled') { text = 'Capture native indisponible : studio lancé avec --no-audio.'; cls = 'err'; }
  else if (a && a.state === 'running') {
    const st = a.stats || {};
    text = `Capture de « ${a.capturing || a.device || 'entrée par défaut'} »` + (st.sample_rate ? ` · ${(st.sample_rate / 1000).toFixed(1)} kHz` : ''); cls = 'ok';
  } else if (a) { text = a.message || 'Ouverture de l\'entrée…'; cls = ['permission_denied', 'no_device', 'device_lost', 'error'].includes(a.state) ? 'err' : ''; }
  $('musStatus').textContent = text;
  $('musStatus').className = 'hint ' + cls;

  // Input health (T-242), judged by the studio on the samples: the badge
  // next to the meter, and the help for a refusal or a missing input.
  const nativeOn = a && musNat(musServerSrc) && musNat(a.source);
  const h = nativeOn ? a.health : null;
  showHealth($('musHealth'), h);
  const denied = nativeOn && a.state === 'permission_denied';
  const bad = !!h && h.tone === 'bad';
  $('musSilentHint').hidden = !bad;
  if (bad) {
    $('musSilentHint').textContent = (denied ? 'Accès au micro refusé par macOS. ' : '') + h.help;
    $('musSilentHint').dataset.reason = h.state;
  }

  // Settings.
  $('musGain').disabled = $('musAutoGain').checked;
  $('musGainV').textContent = $('musAutoGain').checked ? 'auto' : (+$('musGain').value > 0 ? '+' : '') + $('musGain').value + ' dB';
  $('musSilenceV').textContent = $('musSilence').value + ' dBFS';
  $('musSensV').textContent = $('musSens').value + ' %';
  $('gainV').textContent = '×' + (+$('gain').value).toFixed(1);
}

// The input health badge (T-242): `h` from /api/state.audio.health or
// /api/audio/diagnostics (null: nothing to judge).
function showHealth(el, h, empty) {
  el.hidden = !h && !empty;
  el.dataset.tone = h ? h.tone : '';
  el.dataset.state = h ? h.state : '';
  el.textContent = h ? h.label : (empty || '');
  el.title = h ? h.help : '';
}

function flashLight(el) {
  el.classList.add('hit');
  clearTimeout(el._t); el._t = setTimeout(() => el.classList.remove('hit'), 140);
}

// The server's source changed elsewhere (API, another page, first load): follow it.
function followSource(src) {
  if (src === musServerSrc && musPick) return;
  musServerSrc = src;
  musPick = src === 'system' ? 'native' : src;
  if (src === 'system') $('musDevice').value = MUS_SYSTEM;
  if (src !== 'browser') stopMic();
}

function showAudioState(a, asked) {
  mus = a;
  if (asked >= musSentAt) followSource(a.source);
  const f = a.features || {};
  // Level: the native meter when it is fresh, else the features' dBFS.
  const db = a.level_db ?? f.level_db ?? -120, peak = a.peak_db ?? db;
  const pos = v => Math.max(0, Math.min(100, (v + 60) / 60 * 100));
  $('musDb').textContent = fmtDb(db);
  $('musDbFill').style.width = pos(db) + '%';
  $('musDbFill').classList.toggle('hot', peak >= -0.5);
  $('musDbPeak').style.left = `calc(${pos(peak)}% - 2px)`;
  for (const k of BANDS) document.querySelector(`#musBands [data-band="${k}"] .bar > div`).style.height = Math.round(100 * (a.bands?.[k] ?? 0)) + '%';
  // Lights: a counter that moved since the last poll.
  const c = a.counters || {};
  if (musCounters) {
    for (const [k, id] of [['kick', 'lKick'], ['snare', 'lSnare'], ['hat', 'lHat']]) if ((c[k] ?? 0) > (musCounters[k] ?? 0)) flashLight($(id));
    if ((c.beat ?? 0) > (musCounters.beat ?? 0) && !audio) flashLight($('beatDot'));
  }
  musCounters = c;
  const bpm = a.detected_bpm || 0, conf = a.detected_confidence || 0;
  $('musBpm').textContent = bpm > 0 ? bpm.toFixed(1) : '–';
  $('musConf').style.width = Math.round(100 * conf) + '%';
  $('musConfV').textContent = bpm > 0 ? Math.round(100 * conf) + ' %' : '–';
  $('musTempoState').textContent = a.tempo ? TEMPO_FR[a.tempo.state] || a.tempo.state : 'estimation native seulement';
  const sec = a.section || 'silence';
  const since = a.sections && a.sections.since_s > 0 ? ` · ${Math.round(a.sections.since_s)} s` : '';
  $('musSection').textContent = (SECTION_FR[sec] || sec) + since;
  $('musSection').dataset.section = sec;
  $('musBuildup').style.width = Math.round(100 * (a.buildup || 0)) + '%';
  $('musBuildupV').textContent = Math.round(100 * (a.buildup || 0)) + ' %';
  $('musDrops').textContent = c.drop ?? 0;
  showMusic();
}

function drawSpectrum(values) {
  const cv = $('musSpectrum'), w = cv.clientWidth | 0, h = cv.clientHeight | 0;
  if (!w || !h) return;
  if (cv.width !== w * devicePixelRatio) { cv.width = w * devicePixelRatio; cv.height = h * devicePixelRatio; }
  const g = cv.getContext('2d');
  g.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
  g.clearRect(0, 0, w, h);
  if (!values) {
    g.fillStyle = '#8b929c'; g.font = '11px system-ui'; g.textAlign = 'center';
    g.fillText(musPick === 'browser' ? 'Spectre : « Écouter » le micro de la page' : 'Spectre : pas de capture native en cours', w / 2, h / 2 + 4);
    cv.dataset.bands = '0';
    return;
  }
  const bw = w / values.length;
  const grad = g.createLinearGradient(0, h, 0, 0);
  grad.addColorStop(0, '#3ddc84'); grad.addColorStop(0.75, '#ffb020'); grad.addColorStop(1, '#ff4d4f');
  g.fillStyle = grad;
  values.forEach((v, i) => { const bh = Math.max(1, v * h); g.fillRect(i * bw + 0.5, h - bh, Math.max(1, bw - 1), bh); });
  cv.dataset.bands = String(values.length);
  cv.dataset.peak = String(values.indexOf(Math.max(...values)));
}

// Polling: state 10/s (6.7/s with the spectrum), spectrum 20/s (Native
// only), devices every 5 s,
// only while the panel is visible; the preview's /api/frame loop is not touched.
let musTick = 0, musBusy = false, musWasVisible = false;
async function musicLoop() {
  const visible = musVisible();
  if (visible && !musWasVisible) { musCounters = null; loadMusConfig(); loadDevices(); }
  musWasVisible = visible;
  if (visible && !musBusy) {
    musBusy = true;
    try {
      const jobs = [];
      const analysisOpen = $('musAnalysis').open;
      const spectrum = analysisOpen && mus && musNat(mus.source) && musNat(musServerSrc);
      const asked = performance.now();
      if (musTick % (spectrum ? 3 : 2) === 0) jobs.push(apiJson('GET', '/api/audio/state').then(a => showAudioState(a, asked)));
      if (spectrum) {
        jobs.push(apiJson('GET', '/api/audio/spectrum').then(sp => drawSpectrum(sp.values)));
      } else if (analysisOpen) drawSpectrum(audio && (musPick || 'browser') === 'browser' ? browserSpectrum() : null);
      if (musTick % 100 === 0 && musTick && musPick === 'native') jobs.push(loadDevices());
      await Promise.all(jobs);
    } catch { /* studio unreachable: the preview says so */ }
    musBusy = false;
    musTick++;
  }
  setTimeout(musicLoop, visible ? 50 : 250);
}

document.querySelectorAll('#musSrc button').forEach(b => b.addEventListener('click', () => pickSource(b.dataset.src)));
$('micBtn').addEventListener('click', listenClick);
$('micSelect').addEventListener('change', () => { if (audio) startMic().catch(e => { musError = 'Micro refusé par le navigateur : ' + e.name; showMusic(); }); });
$('musDevice').addEventListener('change', () => {
  const v = $('musDevice').value;
  if (musNat(musServerSrc)) { if (v === MUS_SYSTEM) setMusSource('system'); else setMusSource('native', { device: v || null }); }
  showMusic();
});
$('musSysHelp').addEventListener('click', () => openHelp('son-du-mac'));
$('gain').addEventListener('input', showMusic);
const musAnalysisPatch = patch => post('/api/audio/config', { analysis: patch }).then(loadMusConfig);
$('musAutoGain').addEventListener('change', () => { showMusic(); musAnalysisPatch({ auto_gain: $('musAutoGain').checked }); });
$('musGain').addEventListener('input', showMusic);
$('musGain').addEventListener('change', () => musAnalysisPatch({ manual_gain_db: +$('musGain').value }));
$('musSilence').addEventListener('input', showMusic);
$('musSilence').addEventListener('change', () => musAnalysisPatch({ silence_db: +$('musSilence').value }));
$('musSens').addEventListener('input', showMusic);
$('musSens').addEventListener('change', () => musAnalysisPatch({ onsets: { delta: musDelta(+$('musSens').value) } }));
$('musFlash').addEventListener('input', showMusSafety);
$('musFlash').addEventListener('change', () => musSafetyPatch({ max_flash_hz: +$('musFlash').value }));
document.querySelectorAll('#musSilAct button').forEach(b => b.addEventListener('click', () => {
  if (b.dataset.act !== 'calm_look') return musSafetyPatch({ silence_action: b.dataset.act });
  // A calm look needs a scene: the current one, else the first saved one.
  const scene = musSilenceAction().scene || scenes[0]?.name;
  if (scene) return musSafetyPatch({ silence_action: { calm_look: scene } });
  $('musCalm').hidden = false;
  $('musGuard').textContent = 'Sauvez d\'abord une scène (onglet Scènes) pour l\'utiliser comme look calme.';
}));
$('musCalm').addEventListener('change', () => {
  const v = $('musCalm').value;
  musSafetyPatch({ silence_action: v ? { calm_look: v } : 'keep' });
});
loadMusConfig();
musicLoop();

export { fmtDb, musNat, showAudioGuard, showHealth, showMusSafety };
