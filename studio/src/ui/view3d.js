// Laser Studio — view3d.js (T-435).
// Sections (as in the former index.html script):
//   3D beam view (T-275, T-276)
//   Vidéo de la vue 3D (T-459)
//   « Préparer » (T-274)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './state.js';
import { $, VUE, VUE_RENDU, api, hex, rgb, sendLatest, setVal } from './core.js';
import { draw } from './presence.js';
import { remember, remembered } from './workspaces.js';
import { outdoorState, safety } from './safety.js';
import { audienceMode } from './targets.js';
import { deck, paintCues, refreshLook } from './cues.js';
import { pollFrames3d, venue, venueTo3d } from './venue.js';

// ---------- 3D beam view (T-275, T-276) ----------
// Preview only: beam3d.js gets the same points as the 2D canvas and has no
// way to reach the server. It is loaded on the first click on "3D", runs
// its render loop only while shown, and remembers its render settings in
// this browser.
let view3d = null, show3d = false, lastPoints = [], lastShown = [], beam3d = null;
function assignLastPoints(v) { return (lastPoints = v); }
function assignLastShown(v) { return (lastShown = v); }
const R3_KEY = 'laserStudio.beam3d.render';
function loadRender3d() {
  try { return JSON.parse(localStorage.getItem(R3_KEY) || '{}') || {}; } catch { return {}; }
}
function saveRender3d() {
  remember(R3_KEY, JSON.stringify(view3d.render));
}
const QUALITY_FR = { low: 'Basse', medium: 'Moyenne', high: 'Haute' };
function showRender3d() {
  const r = view3d.render;
  $('r3exposure').value = Math.round(r.exposure * 100); $('r3exposureV').textContent = Math.round(r.exposure * 100) + ' %';
  $('r3haze').value = Math.round(r.haze * 100); $('r3hazeV').textContent = Math.round(r.haze * 100) + ' %';
  $('r3roomLight').value = Math.round(r.roomLight * 100); $('r3roomLightV').textContent = Math.round(r.roomLight * 100) + ' %';
  for (const k of ['sheets', 'spots', 'bloom', 'hazeNoise', ...SF3D]) $('r3' + k).checked = r[k];
  for (const k of SF3D.slice(1)) $('r3' + k).disabled = !r.safety;
  $('r3quality').value = r.quality;
  showAud3d();
}
// Safety overlays (T-279): master switch, then each layer. Display only:
// the counter is worked out here from the frames the page already has.
const SF3D = ['safety', 'sfAudience', 'sfHorizon', 'sfZones', 'sfOutdoor'];
function safetyTo3d() {
  if (view3d) view3d.setSafety(safety, !!(audienceMode && audienceMode.scanning));
  outdoorTo3d();
}
/** The outdoor cut (T-477) per projector of the 3D view, in the venue's order. */
function outdoorTo3d() {
  if (!view3d) return;
  const o = outdoorState, on = !!(o && (o.enabled || o.sky_off) && o.cut);
  const ids = venue ? venue.projectors.map(p => p.id) : ['main'];
  view3d.setOutdoor(on ? ids.map((id, i) => (i === 0 ? o.cut : (o.cuts || {})[id]) || null) : null);
}
function showAud3d() {
  const on = show3d && !!view3d && view3d.render.safety;
  const n = on ? view3d.inAudience : 0;
  $('aud3d').hidden = $('stAudWrap').hidden = !on;
  $('aud3dN').textContent = $('stAud').textContent = n;
  $('aud3d').classList.toggle('hot', n > 0); $('stAudWrap').classList.toggle('hot', n > 0);
}
// The settings drawer (T-503): closed by default so the view keeps its
// height, opened by « Réglages 3D » in the bar, the choice remembered in
// this browser. It floats over the bottom of the view and scrolls inside
// itself; the badges and the stats underneath never move.
const R3_OPEN_KEY = 'laserStudio.beam3d.settingsOpen';
let render3dOpen = remembered(R3_OPEN_KEY) === '1';
// T-516: the 3D canvas is hidden by the stylesheet in a view up to 420 px
// wide in 2D+3D with the prepared pane (the 2D output alone is kept).
let collapsed3d = false;
function showRender3dDrawer() {
  const open = show3d && render3dOpen && !collapsed3d;
  $('render3d').hidden = !open;
  $('render3dBtn').hidden = !show3d || collapsed3d;
  $('render3dBtn').setAttribute('aria-expanded', String(open));
}
// While the 3D canvas is collapsed (see collapsed3d), the bar says so
// (#no3dNote) and the 3D-only controls go: « Recentrer », the drag hint,
// the video (a recording in progress stops, as when leaving the 3D
// view), the settings drawer and its button. Everything comes back as
// soon as the canvas is drawn again (pane gone, 3D mode, wider view).
// Called after a mode or pane change (setView shows the controls again
// for a 3D mode, so this always re-applies the state), and from the
// view's ResizeObserver (on the next frame, so the bar's change never
// lands in the observer's own cycle).
function sync3dCollapsed() {
  const next = show3d && !$('beams').offsetParent, changed = next !== collapsed3d;
  collapsed3d = next;
  $('no3dNote').hidden = !next;
  for (const id of ['recenter3d', 'hint3d']) $(id).hidden = !show3d || next;
  if (next) $('vid3d').hidden = true; else if (show3d) $('vid3d').hidden = false;
  if (next && changed) { vidStop('vue'); closeRecMenu(); }
  showRender3dDrawer();
}
// T-512: the drawer starts under the badges' row, however many lines it
// wraps to (--badges-h on the view, at least 48 px), the prepared pane
// moves down under it (--badges-row) and sizes its picture from its
// header's real height (--phead: one line, or the name on its own line
// in a narrow pane), and the drawer says when more settings sit below
// its edge (.more-below fades its last line).
function drawerMoreBelow() {
  const d = $('render3d');
  d.classList.toggle('more-below', d.scrollTop + d.clientHeight < d.scrollHeight - 1);
}
function fitDrawerUnderBadges() {
  const row = $('viewBadges'), view = row.parentElement, pane = $('prepPane');
  const vis = [...row.children].filter(b => b.offsetParent);
  const low = vis.length ? Math.ceil(Math.max(...vis.map(b => b.getBoundingClientRect().bottom)) - view.getBoundingClientRect().top + 6) : 0;
  view.style.setProperty('--badges-h', Math.max(48, low) + 'px');
  view.style.setProperty('--badges-row', low + 'px'); // .view.prep #prepPane margin-top
  const head = Math.ceil(pane.querySelector('.prep-head').getBoundingClientRect().height);
  view.style.setProperty('--phead', head + 'px');
  if (!pane.hidden) {
    // The pane's slider column shows whole or not at all: its full height
    // (measured shown; no paint in between) against the height left under
    // the row and the header (18 px of pane chrome). Hiding it narrows the
    // pane, which can only wrap the header more: the choice is stable.
    const side = pane.querySelector('.prep-side');
    side.style.display = 'flex'; const need = side.scrollHeight; side.style.display = '';
    view.classList.toggle('no-side', view.clientHeight - low - head - 18 < need);
  }
  drawerMoreBelow();
  if ((show3d && !$('beams').offsetParent) !== collapsed3d) requestAnimationFrame(sync3dCollapsed);
}
$('render3d').addEventListener('scroll', drawerMoreBelow, { passive: true });
if (typeof ResizeObserver === 'function') {
  const ro = new ResizeObserver(fitDrawerUnderBadges);
  ro.observe($('viewBadges')); ro.observe($('render3d')); ro.observe($('viewBadges').parentElement);
  ro.observe($('prepPane').querySelector('.prep-head'));
}
function setRender3dOpen(on) {
  render3dOpen = !!on;
  remember(R3_OPEN_KEY, render3dOpen ? '1' : '0');
  showRender3dDrawer();
}
$('render3dBtn').onclick = () => setRender3dOpen(!render3dOpen);
$('render3dClose').onclick = () => { setRender3dOpen(false); $('render3dBtn').focus(); };
function bindRender3d() {
  const set = patch => { view3d.setRender(patch); saveRender3d(); showRender3d(); };
  for (const k of ['exposure', 'haze', 'roomLight']) $('r3' + k).oninput = e => set({ [k]: e.target.value / 100 });
  for (const k of ['sheets', 'spots', 'bloom', 'hazeNoise', ...SF3D]) $('r3' + k).onchange = e => set({ [k]: e.target.checked });
  $('r3quality').onchange = e => set({ quality: e.target.value });
  $('recenter3d').onclick = () => view3d.recenter();
  // Which level Auto picked, and the render cost.
  setInterval(() => {
    if (!show3d || !view3d) return;
    const q = QUALITY_FR[beam3d.QUALITIES[view3d.qualityLevel()]];
    $('r3info').textContent = `${view3d.render.quality === 'auto' ? 'Auto : ' + q : q} · ${view3d.stats.drawMs.toFixed(1)} ms`;
  }, 500);
  setInterval(() => { if (show3d && view3d) showAud3d(); }, 200);
}
// Viewpoints (T-278): computed from the venue by beam3d.js; the choice
// (and the *Libre* position) is saved in venue.json through
// /api/venue/camera, a view like a camera move: allowed in show mode,
// never undone, never touching the output. The orbit mode is this
// browser's. Display only: the 3D view is a design aid, not a safety check.
const ORBIT_KEY = 'laserStudio.beam3d.orbit';
let cam3dRestored = false, cam3dTimer = 0;
function assignCam3dRestored(v) { return (cam3dRestored = v); }
function bindCam3d() {
  view3d.setOrbitMode(remembered(ORBIT_KEY) || 'room', { retarget: false });
  view3d.onCameraChange = c => {
    showCam3d();
    if (venue) venue.camera = c;
    clearTimeout(cam3dTimer);
    cam3dTimer = setTimeout(() => api('POST', '/api/venue/camera', c, { quiet: true }).catch(() => { /* next move saves it */ }), 300);
  };
  document.querySelectorAll('#cam3dSeg button').forEach(b => b.onclick = () => setCam3d(b.dataset.cam));
  $('cam3dOrbit').onchange = e => { view3d.setOrbitMode(e.target.value); remember(ORBIT_KEY, e.target.value); showCam3d(); };
  showCam3d();
}
function setCam3d(id) {
  if (!view3d || !show3d) return false;
  view3d.applyPreset(id);
  showCam3d();
}
function showCam3d() {
  if (!view3d) return;
  const st = view3d.cameraState();
  document.querySelectorAll('#cam3dSeg button').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.cam === st.preset)));
  $('cam3dOrbit').value = st.orbit;
}
// The camera for the e2e tests and the console: read only.
window.__viz = {
  camera: () => view3d ? view3d.cameraState() : null,
  presets: () => view3d ? view3d.presets() : null,
};
// Stage tabs (T-270): 2D, 3D, or both side by side. The choice is
// remembered in this browser.
const STAGE_KEY = 'laserStudio.stageTab';
let show2d = true;
async function setView(mode) {
  if (!['2d', '3d', 'both'].includes(mode)) mode = '2d';
  // Windows (T-281): 3D shows the 3D view; Sortie the one its address asks
  // for (&rendu=3d or both), 2D by default.
  if (VUE === '3d') mode = '3d';
  if (VUE === 'sortie') mode = VUE_RENDU;
  const want3d = mode !== '2d';
  if (want3d && !view3d && $('beamsMsg').hidden) {
    try {
      beam3d ??= await import('/beam3d.js');
      if (!beam3d.BeamView.supported()) throw new Error('no WebGL2');
      view3d = new beam3d.BeamView($('beams'), { render: { ...beam3d.DEFAULT_RENDER, ...loadRender3d() } });
      bindRender3d(); bindCam3d(); showRender3d();
    } catch (e) {
      console.warn('3D view unavailable:', e);
      $('beamsMsg').hidden = false;
    }
  }
  show3d = want3d && !!view3d;
  show2d = mode !== '3d';
  document.querySelectorAll('#viewSeg button').forEach(b => b.classList.toggle('active', b.dataset.view === mode));
  $('preview').parentElement.classList.toggle('both', mode === 'both');
  $('preview').hidden = !show2d;
  $('beams').hidden = !show3d;
  $('beamsMsg').hidden = !want3d || !!view3d;
  for (const id of ['recenter3d', 'hint3d']) $(id).hidden = !show3d;
  showRender3dDrawer();
  // Vidéo (T-459): shown with the 3D view (greyed if it cannot record);
  // leaving the 3D view ends a recording and offers its file.
  $('vid3d').hidden = !want3d;
  if (!show3d) { vidStop('vue'); closeRecMenu(); }
  vidShow();
  if (show3d) { venueTo3d(); safetyTo3d(); view3d.setFrame(lastShown, lastShown === lastPoints ? null : lastPoints); view3d.start(); pollFrames3d(); }
  else view3d?.stop();
  showAud3d();
  if (show2d) draw(lastShown);
  remember(STAGE_KEY, mode);
  sync3dCollapsed();
}
document.querySelectorAll('#viewSeg button').forEach(b => b.onclick = () => setView(b.dataset.view));

// ---------- Vidéo de la vue 3D (T-459) ----------
// Everything happens in this page: after each 3D image (view3d.onDrawn)
// the picture is copied into a 2D composition canvas that adds the
// « Simulation » watermark, and that canvas is what MediaRecorder encodes.
// Nothing is sent to the studio (no route) nor anywhere else: the file is
// offered to the browser's download. Display only: the engine, the output
// and the arming never see it, and it works with the laser disarmed.
const REC_MAX_S = 300, REC_FPS = 30, REC_WATERMARK = 'Simulation — Laser Studio';
// Memory bound: the encoded chunks stay in the page until the download.
const REC_MAX_BYTES = 400 * 1024 * 1024;
const REC_KEY = 'laserStudio.record3d';
const REC_SECONDS = [5, 10, 30, 60, 0]; // 0: until « Arrêter » (REC_MAX_S at most)
const REC_SIZES = { vue: null, '720p': [1280, 720], '1080p': [1920, 1080] };
const REC_FORMATS = [['video/webm;codecs=vp9', 'webm'], ['video/webm;codecs=vp8', 'webm'], ['video/webm', 'webm'], ['video/mp4', 'mp4']];
const REC = { rec: null, stream: null, cv: null, ctx: null, chunks: [], bytes: 0, fmt: null, name: '', t0: 0, limit: 0, elapsed: 0,
  timer: null, tick: null, stamp: '', stampS: -1, why: '', last: null };
function vidPrefs() {
  let p = {};
  try { p = JSON.parse(localStorage.getItem(REC_KEY) || '{}') || {}; } catch { p = {}; }
  return {
    seconds: REC_SECONDS.includes(p.seconds) ? p.seconds : 10,
    size: Object.hasOwn(REC_SIZES, p.size) ? p.size : 'vue',
    // The show's song is played by the studio on its own audio output,
    // never by this page: there is no track to add, the box stays greyed.
    audio: false,
  };
}
function vidSavePrefs() {
  const p = { seconds: +$('vidSeconds').value, size: $('vidSize').value, audio: $('vidAudio').checked };
  try { localStorage.setItem(REC_KEY, JSON.stringify(p)); } catch { /* private window: not remembered */ }
}
// The first format this browser can write, or null.
function vidFormat() {
  if (typeof MediaRecorder !== 'function' || typeof MediaRecorder.isTypeSupported !== 'function') return null;
  if (typeof HTMLCanvasElement === 'undefined' || typeof HTMLCanvasElement.prototype.captureStream !== 'function') return null;
  for (const [mime, ext] of REC_FORMATS) { try { if (MediaRecorder.isTypeSupported(mime)) return { mime, ext }; } catch { /* next */ } }
  return null;
}
const vidReady = () => !!view3d && show3d && !!vidFormat();
const vidPad = n => String(n).padStart(2, '0');
function vidClock(ms) { const s = Math.max(0, Math.floor(ms / 1000)); return `${vidPad(Math.floor(s / 60))}:${vidPad(s % 60)}`; }
function vidMb(bytes) { return (bytes / 1048576).toLocaleString('fr-FR', { maximumFractionDigits: 1, minimumFractionDigits: 1 }) + ' Mo'; }
function vidShow() {
  const btn = $('vidBtn'), on = !!REC.rec, ok = on || vidReady();
  btn.disabled = !ok;
  btn.classList.toggle('recording', on);
  btn.setAttribute('aria-pressed', String(on));
  btn.title = !ok ? 'Enregistrement vidéo indisponible sur ce navigateur'
    : on ? 'Arrêter et télécharger la vidéo (Alt+R)'
    : "Enregistre la vue 3D. La vidéo porte la mention « Simulation » : ce n'est pas une prise de vue du laser. (Alt+R)";
  if (on) {
    const left = REC.limit * 1000 - (REC.t0 ? performance.now() - REC.t0 : 0);
    btn.textContent = `■ Arrêter — ${vidClock(left + 999)}`;
  } else btn.textContent = '● Enregistrer';
  $('vidOptBtn').disabled = on || !ok;
}
// The date in the watermark, rebuilt once a second.
function vidStamp() {
  const d = new Date(), s = Math.floor(d.getTime() / 1000);
  if (s !== REC.stampS) {
    REC.stampS = s;
    REC.stamp = `${vidPad(d.getDate())}/${vidPad(d.getMonth() + 1)}/${d.getFullYear()} ${vidPad(d.getHours())}:${vidPad(d.getMinutes())}:${vidPad(d.getSeconds())}`;
  }
  return REC.stamp;
}
// Bottom right, on a dark plate so it reads on any picture. Always drawn:
// there is no option to leave it out.
function vidWatermark(c, W, H) {
  const fs = Math.max(14, Math.round(H / 30)), pad = Math.round(fs * 0.6), small = Math.round(fs * 0.8);
  const date = vidStamp();
  c.font = `600 ${fs}px system-ui, sans-serif`;
  const w1 = c.measureText(REC_WATERMARK).width;
  c.font = `${small}px system-ui, sans-serif`;
  const w2 = c.measureText(date).width;
  const bw = Math.max(w1, w2) + pad * 2, bh = fs + small + pad * 2.4;
  c.fillStyle = 'rgba(0, 0, 0, 0.65)';
  c.fillRect(W - bw - pad, H - bh - pad, bw, bh);
  c.fillStyle = '#ffffff'; c.textAlign = 'right'; c.textBaseline = 'bottom';
  c.font = `600 ${fs}px system-ui, sans-serif`;
  c.fillText(REC_WATERMARK, W - pad * 2, H - pad * 2.2 - small);
  c.font = `${small}px system-ui, sans-serif`;
  c.fillText(date, W - pad * 2, H - pad * 1.6);
}
// One video image: the 3D picture fitted whole (black bars), then the watermark.
function vidCompose(src) {
  const c = REC.ctx;
  if (!c) return;
  const W = REC.cv.width, H = REC.cv.height;
  c.fillStyle = '#000'; c.fillRect(0, 0, W, H);
  if (src && src.width && src.height) {
    const k = Math.min(W / src.width, H / src.height), w = src.width * k, h = src.height * k;
    c.drawImage(src, (W - w) / 2, (H - h) / 2, w, h);
  }
  vidWatermark(c, W, H);
}
const vidEven = n => Math.max(16, Math.round(n / 2) * 2);
function vidFileStamp(d) {
  return `${d.getFullYear()}${vidPad(d.getMonth() + 1)}${vidPad(d.getDate())}-${vidPad(d.getHours())}${vidPad(d.getMinutes())}${vidPad(d.getSeconds())}`;
}
function vidStart() {
  if (REC.rec || !vidReady()) return;
  const fmt = vidFormat(), pref = vidPrefs(), size = REC_SIZES[pref.size], bc = $('beams');
  // « Taille de la vue »: its proportions, 360 px high or wide at least, 4K at most.
  const k = Math.min(Math.max(1, 360 / Math.max(1, Math.min(bc.width, bc.height))), 3840 / Math.max(1, bc.width), 2160 / Math.max(1, bc.height));
  // A view squeezed to a strip (small window, panels open): 1280×720, the picture fitted whole.
  const odd = !size && (bc.width / bc.height > 4 || bc.height / bc.width > 4);
  const [W, H] = size || (odd ? REC_SIZES['720p'] : [vidEven(bc.width * k), vidEven(bc.height * k)]);
  const cv = document.createElement('canvas');
  cv.width = W; cv.height = H;
  let rec;
  try {
    REC.cv = cv; REC.ctx = cv.getContext('2d', { alpha: false });
    vidCompose(null);
    REC.stream = cv.captureStream(REC_FPS);
    // ~0.1 bit per pixel and image: 1080p ≈ 6 Mbit/s, 5 min ≈ 230 Mo.
    rec = new MediaRecorder(REC.stream, { mimeType: fmt.mime, videoBitsPerSecond: Math.round(Math.min(8e6, Math.max(1e6, W * H * REC_FPS * 0.1))) });
  } catch (e) {
    console.warn('vidéo 3D :', e);
    vidCleanup();
    $('vidStatus').textContent = 'Enregistrement vidéo impossible sur ce navigateur';
    return;
  }
  REC.rec = rec; REC.fmt = fmt; REC.chunks = []; REC.bytes = 0; REC.t0 = 0; REC.elapsed = 0; REC.why = '';
  REC.limit = pref.seconds || REC_MAX_S;
  REC.name = `laser-studio-simulation-${vidFileStamp(new Date())}.${fmt.ext}`;
  if (size) { view3d.setRenderSize(size); bc.classList.add('rec-fixed'); }
  view3d.onDrawn = vidCompose;
  rec.ondataavailable = e => {
    if (!e.data || !e.data.size) return;
    REC.chunks.push(e.data); REC.bytes += e.data.size;
    if (REC.bytes > REC_MAX_BYTES && REC.rec === rec) vidStop('taille');
  };
  rec.onstart = () => {
    REC.t0 = performance.now();
    REC.timer = setTimeout(() => vidStop('fin'), REC.limit * 1000);
  };
  rec.onstop = () => vidFinish(rec);
  rec.onerror = e => { console.warn('vidéo 3D :', e.error || e); vidStop('erreur'); };
  rec.start(1000);
  REC.tick = setInterval(vidShow, 250);
  $('vidStatus').textContent = '';
  closeRecMenu();
  vidShow();
}
// Ends the recording (its file is then offered): `why` says what stopped it.
function vidStop(why = 'arrêt') {
  const rec = REC.rec;
  if (!rec || REC.why) return;
  REC.why = why;
  clearTimeout(REC.timer); REC.timer = null;
  clearInterval(REC.tick); REC.tick = null;
  REC.elapsed = REC.t0 ? (performance.now() - REC.t0) / 1000 : 0;
  if (view3d) { view3d.onDrawn = null; view3d.setRenderSize(null); }
  $('beams').classList.remove('rec-fixed');
  try { if (rec.state !== 'inactive') rec.stop(); else vidFinish(rec); } catch (e) { console.warn('vidéo 3D :', e); vidFinish(rec); }
}
function vidCleanup() {
  try { REC.stream?.getTracks().forEach(t => t.stop()); } catch { /* already stopped */ }
  REC.stream = null; REC.rec = null; REC.ctx = null;
}
function vidFinish(rec) {
  if (REC.rec !== rec) return;
  const chunks = REC.chunks, bytes = REC.bytes, fmt = REC.fmt, name = REC.name, why = REC.why;
  REC.chunks = []; REC.bytes = 0; REC.why = '';
  vidCleanup();
  vidShow();
  if (!bytes) { $('vidStatus').textContent = 'Vidéo vide : rien à télécharger'; return; }
  const blob = new Blob(chunks, { type: fmt.mime.split(';')[0] });
  REC.last = { name, bytes: blob.size, seconds: REC.elapsed, mime: blob.type, why };
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob); a.download = name;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 60000);
  const said = { vue: ' (vue 3D quittée)', espace: ' (espace de travail changé)', page: ' (page masquée)', taille: ' (taille maximale atteinte)', erreur: ' (erreur du navigateur)' }[why] || '';
  $('vidStatus').textContent = `Vidéo prête (${vidMb(blob.size)})${said}`;
}
function vidToggle() { if (REC.rec) vidStop('arrêt'); else vidStart(); }
// The options menu, beside the button.
function closeRecMenu() { $('vidMenu').hidden = true; $('vidOptBtn').setAttribute('aria-expanded', 'false'); }
function openRecMenu() {
  const p = vidPrefs();
  $('vidSeconds').value = String(p.seconds); $('vidSize').value = p.size; $('vidAudio').checked = false;
  const menu = $('vidMenu'), r = $('vidOptBtn').getBoundingClientRect();
  menu.hidden = false;
  menu.style.top = (r.bottom + 4) + 'px';
  menu.style.left = Math.max(8, Math.min(r.left, innerWidth - menu.offsetWidth - 8)) + 'px';
  $('vidOptBtn').setAttribute('aria-expanded', 'true');
  $('vidSeconds').focus();
}
$('vidBtn').onclick = vidToggle;
$('vidOptBtn').onclick = e => { e.stopPropagation(); if ($('vidMenu').hidden) openRecMenu(); else closeRecMenu(); };
for (const id of ['vidSeconds', 'vidSize', 'vidAudio']) $(id).addEventListener('change', vidSavePrefs);
document.addEventListener('click', e => { if (!$('vidMenu').hidden && !e.target.closest('#vidMenu, #vidOptBtn')) closeRecMenu(); });
// A hidden page draws no more images: the recording ends there.
document.addEventListener('visibilitychange', () => { if (document.hidden) vidStop('page'); });
// For the e2e tests and the console: read only.
window.__rec = {
  canvas: () => REC.cv, recording: () => !!REC.rec, last: () => REC.last,
  format: () => { const f = vidFormat(); return f && f.mime; },
};
vidShow();

// ---------- « Préparer » (T-274) ----------
// A cue loaded blind into its own view, « Préparé », drawn from
// GET /api/preview-frame: the studio renders it on a separate animator
// that never feeds an output. Adjusted here, then « Envoyer » plays it
// like a click on its cell (transition and quantise included). The mode
// is this page's; what is prepared is the studio's (`prepared` in the state).
let prepMode = false, prepared = null, prepRev = -1, prepTimer = null, prepBusy = false;
const prepCv = $('prepared');
function showPrepPane() {
  // Not in the Sortie and 3D windows (T-281): the picture stays the output's.
  const c = prepared && prepared.cue, on = (prepMode || !!c) && VUE !== 'sortie' && VUE !== '3d';
  $('prepPane').hidden = !on; $('outTag').hidden = !on;
  $('preview').parentElement.classList.toggle('prep', on);
  $('prepBtn').classList.toggle('active', prepMode);
  $('prepBtn').setAttribute('aria-pressed', String(prepMode));
  document.body.classList.toggle('prep-mode', prepMode);
  $('prepName').textContent = c ? c.name : 'Cliquez une case pour la préparer';
  $('prepName').title = c ? c.name : ''; // the full name when a narrow pane cuts it
  for (const id of ['prepSend', 'prepClear', 'prepScale', 'prepBright', 'prepRot', 'prepColor']) $(id).disabled = !c;
  if (c) {
    setVal('prepScale', Math.round(c.scale * 100)); setVal('prepBright', Math.round(c.brightness * 100));
    setVal('prepRot', Math.round(c.rotation_speed)); setVal('prepColor', hex(c.color));
  }
  if (deck) paintCues();
  if (on && !prepTimer) { prepTimer = setInterval(pollPrepared, 50); pollPrepared(); }
  if (!on && prepTimer) { clearInterval(prepTimer); prepTimer = null; }
  if (!c) draw([], prepCv);
  sync3dCollapsed();
}
function showPrepared(p) {
  if (!p || p.rev === prepRev) return;
  prepRev = p.rev; prepared = p;
  showPrepPane();
}
async function pollPrepared() {
  if (prepBusy || document.hidden) return;
  prepBusy = true;
  try {
    const r = await api('GET', '/api/preview-frame', undefined, { cache: 'no-store', quiet: true });
    if (r.ok) {
      const j = await r.json();
      showPrepared(j.prepared);
      if (prepared && prepared.cue) draw(j.points, prepCv);
    }
  } catch { /* the link notice says it */ } finally { prepBusy = false; }
}
// `latest`: a slider being dragged, latest wins per field (T-484).
async function prepPost(path, body, latest = false) {
  const r = latest ? await sendLatest('prep:' + Object.keys(body).join(), 'POST', path, body) : await api('POST', path, body ?? {});
  if (!r) return true; // replaced by a newer value
  if (!r.ok) return false;
  showPrepared(await r.json());
  return true;
}
function setPrepMode(on) { prepMode = on; showPrepPane(); }
async function prepareCue(id) { if (await prepPost('/api/prepared', { id })) pollPrepared(); }
async function sendPrepared() {
  if (prepared && prepared.cue && await prepPost('/api/prepared/send')) refreshLook();
}
$('prepBtn').onclick = () => setPrepMode(!prepMode);
$('prepSend').onclick = sendPrepared;
$('prepClear').onclick = () => prepPost('/api/prepared/clear');
$('prepScale').oninput = e => prepPost('/api/prepared/adjust', { scale: e.target.value / 100 }, true);
$('prepBright').oninput = e => prepPost('/api/prepared/adjust', { brightness: e.target.value / 100 }, true);
$('prepRot').oninput = e => prepPost('/api/prepared/adjust', { rotation_speed: +e.target.value }, true);
$('prepColor').oninput = e => prepPost('/api/prepared/adjust', { color: rgb(e.target.value) }, true);

export { R3_KEY, REC, STAGE_KEY, assignCam3dRestored, assignLastPoints, assignLastShown, beam3d,
  cam3dRestored, lastPoints, lastShown, loadRender3d, outdoorTo3d, prepMode, prepareCue, prepared,
  safetyTo3d, saveRender3d, sendPrepared, setCam3d, setView, show2d, show3d, showCam3d, showPrepared,
  setRender3dOpen, showRender3d, vidReady, vidStop, vidToggle, view3d };
