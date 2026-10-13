// Laser Studio — state.js (T-435).
// Sections (as in the former index.html script):
//   pushed state and binary frames (T-411)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { GALVO, assignLastFrameAt, draw, galvoShown, showKeepAwake, showOutputKind, showPresence } from './presence.js';
import { $, NC, api, assignArmed, assignPps, lookBusy, noticeCondition, notify, pps, pullNotices,
  sendLatest, settingsRev, showHistory, takeProjectModified } from './core.js';
import { armAnsweredAt, assignArm, assignChecklistState, showArmed, showPowerWatch, showShownet } from './laser.js';
import { assignLastPoints, assignLastShown, show2d, show3d, showPrepared, view3d } from './view3d.js';
import { showShowMode } from './workspaces.js';
import { showAudioGuard } from './music.js';
import { highlightPlaylist, showCueList, showTestPattern, showZoneTrace } from './cuelist.js';
import { showOutdoor, showPerfLevel } from './safety.js';
import { assignDwellStatus, showDwell, showStrobe } from './targets.js';
import { liveAnsweredAt, liveSentAt, scopeMods, showDirectScope, showLive } from './direct.js';
import { showAutopilot, showEvolving, showTempo } from './tempo.js';
import { assignGridPage, assignServerPage, grid, gridPage, loadGrid, presets, refreshLook,
  renderCues, serverPage, showCueDeck } from './cues.js';
import { showRunCount } from './presets.js';
import { showMacros, showScripts } from './macros.js';
import { assignLastTimeline, showAgenda, showShowList, showSong, showTcOut, showTimecode,
  showTimeline } from './shows.js';
import { tlOnState } from './timeline-editor.js';
import { showExecutors, showLayers } from './layers.js';
import { showDesk } from './desk.js';
import { showFxGrid, showLfos, showRoutes, showTriggers } from './modulators.js';
import { frNumber, scanAlertFps, showArtnetStatus, showAudienceMode, showCitpLive, showDmxOutLive,
  showFixedRateStatus } from './outputs.js';
import { checklist, reloadChecklist, showProfilePill } from './venue.js';
import { showPhoneAlign, showPhoneAlignNote, showStandbyBar } from './apc40.js';
import { figFrameInfo } from './figures.js';

// ---------- pushed state and binary frames (T-411) ----------
// The state is pushed by the studio (GET /api/events, on each change) and
// the preview reads binary frames (GET /api/frame.bin, which waits for the
// next one). One stream per browser: the page holding the Web Lock opens
// it and passes each state to the other windows (BroadcastChannel), so
// several windows never use up the connections the heartbeat and the stop
// need. If the stream is down, or more than 1 s behind the version the
// frames announce, the page polls /api/frame as before until it is back.
// The stream never counts as the operator's presence: the heartbeat does.
const LINK = {
  leader: false, es: null, up: false, last: null, // this page's own stream (leader)
  leaderUp: false,                                // the leader's, as it last said
  version: 0, state: null, stateAt: 0,            // the last pushed state
  serverVersion: 0, waitFor: 0, waitSince: 0,     // the version frames announce
  seq: undefined, push: null, armSeqMin: 0,         // push: null until the first frame
};
const linkChan = 'BroadcastChannel' in window ? new BroadcastChannel('laser-studio-state') : null;
function takePushed(v, json) {
  if (v === LINK.version) return;
  if (v < LINK.version) LINK.waitFor = 0; // a restarted studio counts from 1 again
  LINK.version = v; LINK.state = JSON.parse(json); LINK.stateAt = performance.now();
  if (LINK.push) applyState(LINK.state, LINK.stateAt, true);
}
function openStream() {
  const es = new EventSource('/api/events');
  LINK.es = es;
  es.addEventListener('state', e => {
    LINK.up = true;
    LINK.last = { v: +e.lastEventId, s: e.data };
    linkChan?.postMessage({ ...LINK.last, up: true });
    takePushed(LINK.last.v, e.data);
  });
  es.onerror = () => {
    // The browser reconnects by itself; until a state comes, poll.
    LINK.up = false;
    linkChan?.postMessage({ up: false });
    if (es.readyState === EventSource.CLOSED) { LINK.es = null; setTimeout(openStream, 3000); }
  };
}
if (linkChan) {
  linkChan.onmessage = ({ data: d }) => {
    if (d.want) { if (LINK.leader && LINK.up && LINK.last) linkChan.postMessage({ ...LINK.last, up: true }); return; }
    if ('up' in d) LINK.leaderUp = d.up;
    if (d.s) takePushed(d.v, d.s);
  };
}
if (linkChan && navigator.locks) {
  // Held until the page closes; then another window takes the stream over.
  navigator.locks.request('laser-studio-events', () => { LINK.leader = true; openStream(); return new Promise(() => {}); });
  linkChan.postMessage({ want: true });
} else {
  LINK.leader = true; openStream();
}
// Whether the pushed state is current: a stream is up and has caught up
// with what the frames say within 1 s.
function linkHealthy(now) {
  if (!LINK.state || !(LINK.leader ? LINK.up : LINK.leaderUp)) return false;
  if (LINK.waitFor && LINK.version >= LINK.waitFor) LINK.waitFor = 0;
  if (!LINK.waitFor && LINK.serverVersion > LINK.version) { LINK.waitFor = LINK.serverVersion; LINK.waitSince = now; }
  return !LINK.waitFor || now - LINK.waitSince <= 1000;
}
function updateLink() {
  const push = linkHealthy(performance.now());
  if (push === LINK.push) return;
  LINK.push = push;
  $('engineLed').title = 'Chien de garde du moteur — Liaison : ' + (push ? 'flux' : 'interrogation (secours)');
  if (push) applyState(LINK.state, LINK.stateAt, true);
}
// A state skipped by a guard (an edit in flight) is shown on the next pass.
setInterval(() => { if (LINK.push && !document.hidden) applyState(LINK.state, LINK.stateAt, true); }, 500);
// The beat counted on from the last state: at rest no state comes.
function tempoNow() {
  const t = LINK.state && LINK.state.tempo;
  if (!t) return null;
  const beat = t.beat + (performance.now() - LINK.stateAt) / 1000 * t.bpm / 60, whole = Math.floor(beat), bpb = t.beats_per_bar || 4;
  return { ...t, beat, bar: Math.floor(whole / bpb), beat_in_bar: ((whole % bpb) + bpb) % bpb, phase: beat - whole };
}
// GET /api/frame.bin: "LSF1", seq u32, count u16, flags u16, then x, y
// (i16, ±32767) and r, g, b (u8) per point, little-endian.
function decodeFrame(buf) {
  const d = new DataView(buf);
  if (buf.byteLength < 12 || d.getUint32(0, false) !== 0x4C534631) return null;
  const n = d.getUint16(8, true), points = new Array(n);
  if (buf.byteLength !== 12 + 7 * n) return null;
  for (let i = 0, o = 12; i < n; i++, o += 7) {
    points[i] = [d.getInt16(o, true) / 32767, d.getInt16(o + 2, true) / 32767, d.getUint8(o + 4) / 255, d.getUint8(o + 5) / 255, d.getUint8(o + 6) / 255];
  }
  return { seq: d.getUint32(4, true), flags: d.getUint16(10, true), points };
}
function showPoints(points) {
  assignLastPoints(points);
  // One frame feeds whichever view is shown; the 3D view only reads it.
  // With the galvo simulation (T-280) both draw the simulated trace; the
  // 3D view also gets the frame as sent, for its audience check.
  const shown = assignLastShown(galvoShown(points));
  if (view3d && show3d) {
    view3d.setSim({ gain: GALVO.gain, divergenceMrad: GALVO.divergence_mrad });
    view3d.setFrame(shown, shown === points ? null : points);
  }
  if (show2d) draw(shown);
  $('galvoLong').hidden = !GALVO.long;
  const n = points.length, fps = n ? pps / n : 0;
  $('stPoints').textContent = n;
  // A fixed rate (T-463) shows as « 25 im/s fixe »: the frame size is the rate's.
  const fixed = lastFixedRate && lastFixedRate.target_fps;
  $('stFps').textContent = fixed ? frNumber(+fixed, Number.isInteger(+fixed) ? 0 : 2) + ' im/s fixe' : n ? fps.toFixed(1) : '–';
  $('stFps').classList.toggle('bad', !fixed && n && fps < scanAlertFps);
}
var lastFixedRate = null;
async function pushFrame() {
  const fetchedAt = performance.now();
  const r = await api('GET', '/api/frame.bin' + (LINK.seq === undefined ? '' : '?after=' + LINK.seq), undefined, { cache: 'no-store', quiet: true });
  if (!r.ok) throw new Error('/api/frame.bin ' + r.status);
  LINK.serverVersion = +(r.headers.get('X-State-Version') || 0);
  const fr = decodeFrame(await r.arrayBuffer());
  if (!fr) throw new Error('/api/frame.bin: image illisible');
  // Only a new frame counts: the same one again means the engine is stuck.
  if (fr.seq !== LINK.seq) assignLastFrameAt(performance.now());
  LINK.seq = fr.seq;
  // The armed state comes with every frame, as the engine saw it. A frame
  // published before our last arm request was handled may be older.
  if (fetchedAt > armAnsweredAt && (fr.seq > LINK.armSeqMin || fetchedAt - armAnsweredAt > 1000)) assignArmed(!!(fr.flags & 1));
  showArmed();
  showPoints(fr.points);
  showTempo(tempoNow());
}
async function pollFrame() {
  const fetchedAt = performance.now();
  const r = await api('GET', '/api/frame');
  LINK.serverVersion = +(r.headers.get('X-State-Version') || 0);
  const f = await r.json();
  assignLastFrameAt(performance.now());
  if (fetchedAt > armAnsweredAt) assignArmed(f.armed);
  showPoints(f.points);
  applyState(f, fetchedAt, false);
}
async function previewLoop() {
  const started = performance.now();
  try {
    if (LINK.push) await pushFrame(); else await pollFrame();
  } catch (e) { $('output').textContent = '⚠ Studio injoignable'; }
  updateLink();
  // About 30 images a second; a polled one waits as before.
  setTimeout(previewLoop, LINK.push ? Math.max(0, 33 - (performance.now() - started)) : 33);
}
// Everything the page shows besides the points: a pushed state (`pushed`,
// received at `at`) or a polled /api/frame (fetched at `at`).
// Palettes festival (T-130): the pills, the quantisation and the
// perceived balance. A pick lands on the server's boundary (next bar by
// default); the pill blinks until then.
let festival = null, festSentAt = 0;
function rgbHex(c) { return '#' + c.map(v => v.toString(16).padStart(2, '0')).join(''); }
function showFestival(f) {
  if (!f) return;
  festival = f;
  const pals = $('festPals');
  if (pals.children.length !== f.palettes.length) {
    pals.replaceChildren(...f.palettes.map((p, i) => {
      const b = document.createElement('button');
      b.className = 'fest-pal'; b.dataset.pal = i;
      b.style.setProperty('--c1', rgbHex(p.c1)); b.style.setProperty('--c2', rgbHex(p.c2));
      b.title = p.name + ' (couleur 1, couleur 2) : s’applique à tout ce qui joue';
      const dot = document.createElement('span'); dot.className = 'dot';
      b.append(dot, document.createTextNode(p.name));
      b.onclick = () => pickPalette(i);
      return b;
    }));
  }
  const waiting = f.pending ? f.pending.palette : undefined;
  pals.querySelectorAll('.fest-pal').forEach(b => {
    const i = +b.dataset.pal;
    b.classList.toggle('on', f.active === i);
    b.classList.toggle('pending', waiting === i);
  });
  $('festOwn').classList.toggle('on', f.active == null);
  $('festOwn').classList.toggle('pending', !!f.pending && waiting == null);
  $('festWait').textContent = f.pending
    ? (f.quantize === 'phrase' ? 'attend la phrase…' : 'attend la mesure…') : '';
  // Our own edit in flight: don't draw an older state over the controls.
  if (performance.now() - festSentAt < 400) return;
  document.querySelectorAll('#festQuant button').forEach(b => b.classList.toggle('active', b.dataset.fq === f.quantize));
  $('festBal').checked = f.balance.enabled;
  $('festGains').hidden = !f.balance.enabled;
  ['R', 'G', 'B'].forEach((k, i) => { if (document.activeElement !== $('festGain' + k)) $('festGain' + k).value = f.balance.gains[i]; });
}
async function pickPalette(palette) {
  const r = await api('POST', '/api/festival/palette', { palette });
  if (r.ok) showFestival(await r.json());
}
async function sendFestival(patch) {
  if (!festival) return;
  festSentAt = performance.now();
  const body = { quantize: festival.quantize, balance: { ...festival.balance }, ...patch };
  const r = await sendLatest('festival', 'POST', '/api/festival', body);
  if (r && r.ok) { festSentAt = 0; showFestival(await r.json()); }
}
$('festOwn').onclick = () => pickPalette(null);
document.querySelectorAll('#festQuant button').forEach(b => b.onclick = () => sendFestival({ quantize: b.dataset.fq }));
$('festBal').onchange = () => sendFestival({ balance: { ...festival.balance, enabled: $('festBal').checked } });
['R', 'G', 'B'].forEach((k, i) => $('festGain' + k).oninput = () => {
  const gains = [...festival.balance.gains]; gains[i] = +$('festGain' + k).value;
  sendFestival({ balance: { ...festival.balance, gains } });
});
$('festPreset').onclick = () => sendFestival({ balance: { enabled: true, gains: [1, 0.6, 1] } });

function applyState(f, at, pushed) {
  if (!f) return;
  assignDwellStatus(f.dwell);
  assignArm(f.arm); assignPps(f.pps); assignChecklistState(f.checklist);
  // T-432/T-433: the studio's notices and its « modified » flag.
  if (f.notices_last !== undefined && f.notices_last !== NC.serverLast) pullNotices(f.notices_last);
  takeProjectModified(f.project_modified, at);
  showHistory(f.history);
  noticeConditions(f);
  showShownet(f.shownet);
  showPowerWatch(f.power_watch);
  showAudienceMode(f.audience);
  if (f.outdoor) showOutdoor(f.outdoor);
  showProfilePill(f.safety_profile);
  // Ticked or restarted from another page, or the output became (or
  // stopped being) a ShowNET: refresh the list.
  if (checklist && f.checklist && f.checklist.complete !== checklist.complete) reloadChecklist();
  else if (checklist && f.shownet && (f.shownet.active !== (checklist.output === 'shownet') || f.shownet.hw_stop_ok !== checklist.hw_stop_ok)) reloadChecklist();
  showArmed(); showPresence(f); showKeepAwake(f.keep_awake);
  if (f.perf_level !== undefined) showPerfLevel(f.perf_level);
  // The main output's fixed rate (T-463): the top bar, and the settings line when it shows the main output.
  if (f.fixed_rate !== undefined) {
    lastFixedRate = f.fixed_rate;
    if (($('pwTarget').value || 'main') === 'main') showFixedRateStatus(f.fixed_rate);
  }
  $('output').textContent = f.output_state === 'lost' ? 'Sortie perdue'
    : f.output_state === 'connecting' ? 'Connexion de la sortie…'
    : f.output_error ? '⚠ ' + f.output_error
    : f.output ? 'Sortie : ' + f.output : 'Aperçu seulement';
  showOutputKind(f);
  showArtnetStatus(f.output_status);
  showDmxOutLive(f.dmx_out);
  showCitpLive(f.citp);
  highlightPlaylist(f.playlist);
  showTempo(pushed ? tempoNow() : f.tempo);
  // A live change of ours in flight, or just answered: an older state must
  // not be drawn over the controls (it would e.g. untick « Synchro tempo »
  // right after it was ticked). A pushed one skipped now is shown on the
  // next pass.
  if (pushed ? liveAnsweredAt >= liveSentAt && performance.now() - liveAnsweredAt > 100
    : at > liveAnsweredAt && liveAnsweredAt >= liveSentAt) {
    showDirectScope(f);
    const m = scopeMods(f);
    if (m) showLive(m);
  }
  showLfos(f.lfos);
  showFxGrid(f.fx);
  showFestival(f.festival);
  showRoutes(f.audio_routes);
  // How long ago each trigger fired, counted on since a pushed state.
  const trig = pushed && f.audio_triggers ? { ...f.audio_triggers, rules: (f.audio_triggers.rules || []).map(r =>
    r.ago_s == null ? r : { ...r, ago_s: r.ago_s + (performance.now() - at) / 1000 }) } : f.audio_triggers;
  showTriggers(trig);
  showAutopilot(f.autopilot);
  showStrobe(f.strobe);
  showAudioGuard(f.audio_guard);
  showDwell(f.dwell);
  showTestPattern(f.test_pattern);
  showZoneTrace(f.zone_trace, f.armed);
  if (f.align !== undefined) showPhoneAlign(f.align);
  if (f.align_note !== undefined) showPhoneAlignNote(f.align_note);
  showShowMode(f.lock);
  // Follow page changes made elsewhere (MIDI controller, API) - only
  // when the server's value changes, so a tab click that the server
  // hasn't processed yet isn't undone by an older frame.
  // The grid changed elsewhere (another page, a project opened, figures).
  if (f.grid_rev !== undefined && f.grid_rev !== grid.rev && presets.length) { grid.rev = f.grid_rev; loadGrid(); }
  if (f.cue_page !== serverPage) {
    assignServerPage(f.cue_page);
    if (grid.pages[serverPage] && serverPage !== gridPage) { assignGridPage(serverPage); renderCues(); }
  }
  showCueDeck(f.cues);
  showPrepared(f.prepared);
  showCueList(f.cuelist);
  // A cell « en attente » (T-159) blinks with the beat.
  if (f.tempo && f.tempo.bpm > 0) $('cues').style.setProperty('--beat', (60 / f.tempo.bpm).toFixed(3) + 's');
  showRunCount(f);
  showMacros(f.macros);
  showScripts(f.scripts);
  // Follow a look changed elsewhere (playlist, cue from MIDI/API, look
  // control); our own requests update settingsRev themselves.
  if (f.settings_rev !== settingsRev && !lookBusy) refreshLook();
  showLayers(f.layers);
  showExecutors(f.executors);
  showDesk(f);
  figFrameInfo(f);
  showEvolving(f.evolving);
  showTimeline(f.timeline);
  assignLastTimeline(f.timeline);
  showTimecode(f.timecode);
  showTcOut(f.timeline && f.timeline.timecode_out);
  showShowList(f.showlist);
  showAgenda(f.agenda);
  if (f.standby) showStandbyBar(f.standby);
  showSong(f.timeline, f.timeline_audio);
  tlOnState(f.timeline, f.tempo, at);
}

// What the banners and the status line show, also recorded in the
// notification centre (T-432). The banners keep their place: no toast for
// what already has one (stop, MIDI, DMX, damaged files, engine link).
function noticeConditions(f) {
  noticeCondition('estop', !!(f.arm && f.arm.estop), { level: 'error', source: 'Sécurité', text: "Arrêt d'urgence enclenché : réinitialiser pour réarmer", toast: false });
  const lost = f.output_state === 'lost';
  noticeCondition('output', lost || !!f.output_error, { level: 'error', source: 'Sortie',
    text: lost ? 'Sortie perdue : laser désarmé' + (f.output_error ? ' (' + f.output_error + ')' : '') : 'Erreur de sortie : ' + f.output_error });
  const wifi = f.shownet && f.shownet.wifi;
  noticeCondition('wifi', !!wifi, { level: 'warning', source: 'Sortie', text: 'La sortie ShowNET passe par le Wi-Fi : branchez un câble réseau' });
  const scan = !!(f.audience && f.audience.scanning);
  if (NC.audienceScan !== undefined && NC.audienceScan !== scan) {
    notify({ level: scan ? 'warning' : 'info', source: 'Sécurité', toast: false,
      text: scan ? 'Balayage public déverrouillé : les zones Public reçoivent de la lumière atténuée' : 'Retour au-dessus du public' });
  }
  NC.audienceScan = scan;
}

export { LINK, previewLoop, showPoints, tempoNow };
