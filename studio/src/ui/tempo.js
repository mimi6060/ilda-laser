// Laser Studio — tempo.js (T-435).
// Sections (as in the former index.html script):
//   tempo
//   Vitesses : groupes A à D et cadence (T-419)
//   horloge MIDI (T-207)
//   Ableton Link (T-154)
//   compensation de latence (T-246)
//   Pilote automatique (T-148)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { scopedId } from './direct.js';
import { $, api, apiJson, controlLatest, post, sendLatest, setVal } from './core.js';
import { tempoNow } from './state.js';
import { grid, presetById } from './cues.js';
import { esc } from './midi.js';
import { assignPgTempo } from './program.js';

// ---------- tempo ----------
// The newest evolving cue on show: where it is in its length, one tick per bar.
function showEvolving(list) {
  const e = list && list[list.length - 1];
  $('evoBar').hidden = !e;
  if (!e) return;
  const pos = e.waiting ? 0 : e.pos, len = e.length;
  $('evoPos').textContent = `${pos.toFixed(1)} / ${+len.toFixed(2)} temps`;
  $('evoFill').style.width = Math.min(100, 100 * pos / len) + '%';
  $('evoTicks').style.backgroundSize = (100 * (e.beats_per_bar || 4) / len) + '% 100%';
  // A cue program (T-157) without evolving content has no keys.
  $('evoState').textContent = e.waiting ? 'attend le temps' : e.ended ? (e.keys ? 'fin (dernière clé)' : 'fin (dernière valeur)')
    : (e.keys ? `clé ${e.key + 1}/${e.keys}` : 'programme') + (e.loop ? ` · boucle ${e.pass + 1}` : '');
}

// ---------- Vitesses : groupes A à D et cadence (T-419) ----------
// Each group: ×¼ … ×4 of the master tempo, or a free BPM; the server
// moves a new multiple to the next bar when « Appliquer au début de la
// mesure » is on (the button waits, dashed). Everything goes through the
// registry controls (`speed.<n>.*`, `master.rate`), as MIDI learn does.
const SPEED_NAMES = ['Par défaut', 'A', 'B', 'C', 'D'], SPEED_LABELS = ['×¼', '×½', '×1', '×2', '×4'], SPEED_MULTS = [0.25, 0.5, 1, 2, 4];
function buildSpeedGroups() {
  $('speedGroups').innerHTML = [1, 2, 3, 4].map(n => `<div class="spg" data-g="${n}">
      <span class="spg-n" id="spgName${n}" aria-hidden="true">${SPEED_NAMES[n]}</span>
      <div class="seg" role="group" aria-label="Groupe ${SPEED_NAMES[n]} : multiple du tempo" data-control="speed.${n}.mult" title="speed.${n}.mult">${SPEED_LABELS.map((l, i) =>
        `<button data-mult="${i}" aria-label="Groupe ${SPEED_NAMES[n]} ${l}" aria-pressed="false">${l}</button>`).join('')}</div>
      <div class="spg-free"><label class="check"><input type="checkbox" data-free data-control="speed.${n}.free" title="speed.${n}.free"> BPM libre</label>
        <input type="number" data-bpm min="40" max="250" step="0.1" value="120" aria-label="BPM libre du groupe ${SPEED_NAMES[n]}" data-control="speed.${n}.bpm" title="speed.${n}.bpm"></div>
    </div>`).join('');
  $('speedGroups').querySelectorAll('.spg').forEach(d => {
    const n = +d.dataset.g;
    d.querySelectorAll('[data-mult]').forEach(b => b.addEventListener('click', () => post('/api/control', { id: `speed.${n}.mult`, value: +b.dataset.mult })));
    d.querySelector('[data-free]').addEventListener('change', e => post('/api/control', { id: `speed.${n}.free`, value: e.target.checked }));
    d.querySelector('[data-bpm]').addEventListener('change', e => {
      const v = +e.target.value;
      if (Number.isFinite(v) && v > 0) post('/api/control', { id: `speed.${n}.bpm`, value: v });
    });
  });
}
buildSpeedGroups();
$('speedOnBar').addEventListener('change', e => post('/api/speed-groups', { change_on_bar: e.target.checked }));
$('speedRate').addEventListener('input', () => {
  $('speedRateV').textContent = $('speedRate').value + ' %';
  controlLatest({ id: 'master.rate', value: $('speedRate').value / 100 });
});
let speedKey = '';
function showSpeedGroups(t) {
  if (!t.groups) return;
  const key = JSON.stringify([t.groups, t.rate, t.change_on_bar, t.bpm]);
  if (key === speedKey) return;
  speedKey = key;
  const off = [];
  t.groups.forEach((g, i) => {
    const d = $('speedGroups').children[i];
    if (!d) return;
    const wait = g.pending && g.pending.mode.type === 'multiple' ? SPEED_MULTS.indexOf(g.pending.mode.mult) : -1;
    d.querySelectorAll('[data-mult]').forEach(b => {
      const on = !g.free && SPEED_MULTS[+b.dataset.mult] === g.mult;
      b.classList.toggle('active', on);
      b.setAttribute('aria-pressed', String(on));
      b.classList.toggle('waiting', +b.dataset.mult === wait);
      b.title = `speed.${g.group}.mult` + (+b.dataset.mult === wait ? ' : au prochain premier temps' : '');
    });
    d.querySelector('[data-free]').checked = g.free;
    const bpm = d.querySelector('[data-bpm]');
    if (document.activeElement !== bpm) bpm.value = g.bpm.toFixed(1);
    bpm.disabled = !g.free;
    if (g.free || g.mult !== 1) off.push(`${g.name} ${g.label}`);
  });
  $('speedOnBar').checked = !!t.change_on_bar;
  const rate = Math.round((t.rate || 1) * 100);
  setVal('speedRate', rate);
  if (document.activeElement !== $('speedRate')) $('speedRateV').textContent = rate + ' %';
  if (rate !== 100) off.push(`Cadence ${rate} %`);
  $('speedHint').hidden = !off.length;
  $('speedHint').textContent = off.join(' · ');
}
function showTempo(t) {
  assignPgTempo(t); // the cue editor's ruler (T-164)
  if (!t) return;
  showSpeedGroups(t);
  if (document.activeElement !== $('bpm')) $('bpm').value = t.bpm.toFixed(1);
  // Light the current beat for the first half of it, so the dots visibly pulse.
  document.querySelectorAll('#beats span, #gBeats span').forEach(d => {
    const i = [...d.parentNode.children].indexOf(d);
    d.classList.toggle('on', i === t.beat_in_bar && t.phase < 0.5);
  });
  $('gBpm').textContent = t.bpm.toFixed(1) + ' BPM';
  // Tempo auto (T-234): source, follower state and detection confidence.
  const auto = t.source === 'audio';
  $('tempoAuto').classList.toggle('active', auto);
  $('tempoGuide').hidden = !auto;
  const FOLLOW = { waiting: 'en attente', locked: 'verrouillé', coasting: 'maintien', unlocked: 'déverrouillé' };
  $('tempoSrc').hidden = !auto;
  $('tempoSrc').textContent = auto ? `${FOLLOW[t.follow] || t.follow} ${Math.round(100 * (t.confidence || 0))} %` + (t.guide_bpm ? ` · guide ${t.guide_bpm.toFixed(0)}` : '') : '';
  // Décalage de sortie (T-246): a controller may move it too.
  const off = t.output_offset_ms || 0;
  $('latBtn').classList.toggle('active', off !== 0);
  // The text stays short (the bar is one row at 1280 px); the value is in the title.
  $('latBtn').title = 'Latence : décalage de sortie ' + (off ? fmtMs(off) : 'aucun') + ', latence estimée, calage à l\'oreille';
  if (LAT && LAT.data && LAT.data.config.output_offset_ms !== off && !LAT.send) { LAT.data.config.output_offset_ms = off; showLatency(); }
  // Temps 1 suggéré (T-235): a dotted outline on the dot where the music's
  // "1" falls, the text and *Accepter* (a resync on it).
  const db = t.downbeat, sugg = !!(db && t.downbeat_suggested);
  document.querySelectorAll('#beats span').forEach((d, i) => d.classList.toggle('suggest', sugg && i === db.beat_in_bar_offset));
  $('downbeatHint').hidden = $('downbeatAccept').hidden = !sugg;
  if (sugg) $('downbeatHint').textContent = `Temps 1 suggéré ${Math.round(100 * db.confidence)} %`;
  const phrase = t.phrase_bar != null;
  $('phraseHint').hidden = !phrase;
  $('phraseHint').textContent = phrase ? `phrase ${t.phrase_bar + 1}/${t.phrase_bars}` : '';
  // Horloge MIDI (T-207): the followed clock; the studio refuses Tap,
  // Nudge and BPM meanwhile (and says so).
  const mc = t.midi_clock, follow = !!(mc && mc.follow);
  $('midiClockHint').hidden = !follow;
  if (follow) $('midiClockHint').textContent = midiClockText(mc);
  // Ableton Link (T-154): the session and its peers.
  const lk = t.link, linked = !!(lk && lk.enabled);
  $('linkHint').hidden = !linked;
  if (linked) $('linkHint').textContent = linkText(lk);
}
$('tempoAuto').addEventListener('click', () => post('/api/control', { id: 'tempo.auto', value: $('tempoAuto').classList.contains('active') ? 0 : 1 }));
document.querySelectorAll('[data-ctl]').forEach(b => b.addEventListener('click', () => { const id = scopedId(b.dataset.ctl); if (id) post('/api/control', { id }); }));
$('bpm').addEventListener('change', () => post('/api/control', { id: 'tempo.bpm', value: +$('bpm').value }));

// ---------- horloge MIDI (T-207) ----------
// Follow a deck's MIDI clock (tempo only, never arming), send ours to a
// port the operator picks (off by default, never back to the followed port).
let mclkTimer = null;
function frBpm(bpm) { return bpm.toFixed(1).replace('.', ',') + ' BPM'; }
function midiClockText(mc) {
  const bpm = mc.bpm != null ? frBpm(mc.bpm) : '';
  return { waiting: 'Horloge MIDI : en attente', following: 'Horloge MIDI : ' + (bpm || '…'), stopped: 'Horloge MIDI : arrêtée' + (bpm ? ` (${bpm})` : ''), lost: 'Horloge MIDI perdue : tempo manuel' }[mc.state] || 'Horloge MIDI';
}
function fillPorts(sel, ports, first, current, label) {
  if (document.activeElement === sel) return;
  const list = [...ports];
  if (current && !list.includes(current)) list.push(current);
  const html = `<option value="">${esc(first)}</option>` + list.map(p => `<option value="${esc(p)}">${esc(label(p) + (ports.includes(p) ? '' : ' (déconnecté)'))}</option>`).join('');
  if (sel.dataset.html !== html) { sel.innerHTML = html; sel.dataset.html = html; }
  sel.value = current || '';
}
function showMidiClock(v) {
  const c = v.config, i = v.input, o = v.output;
  fillPorts($('mclkSource'), v.inputs, 'Manuel', c.follow ? c.in_port : '', p => `Horloge MIDI (${p})`);
  fillPorts($('mclkOut'), v.outputs, 'Choisir un port…', c.out_port, p => p);
  $('mclkSend').checked = c.send;
  $('mclkInState').textContent = !v.midi_enabled ? 'MIDI désactivé : pas d\'horloge.'
    : c.follow ? `${midiClockText({ state: i.state, bpm: i.bpm })} · ${i.ticks} tops reçus` : 'Tempo manuel (ou Tap, ou Tempo auto).';
  $('mclkOutState').textContent = !c.send ? 'Horloge non envoyée.'
    : o.open ? `Envoyée sur ${o.open} · ${o.ticks} tops` : (o.error || 'Ouverture du port…');
}
async function loadMidiClock() {
  clearTimeout(mclkTimer);
  try { showMidiClock(await apiJson('GET', '/api/midi/clock', undefined, { quiet: true })); } catch { /* shown on the next refresh */ }
  if ($('midiClockPanel').open) mclkTimer = setTimeout(loadMidiClock, 1000);
}
async function postMidiClock(patch) {
  const r = await api('POST', '/api/midi/clock', patch, { quiet: true });
  $('mclkError').textContent = r.ok ? '' : (await r.text()).trim();
  if (r.ok) showMidiClock(await r.json()); else loadMidiClock();
}
$('midiClockPanel').addEventListener('toggle', () => { if ($('midiClockPanel').open) loadMidiClock(); });
$('mclkSource').addEventListener('change', () => {
  const p = $('mclkSource').value;
  postMidiClock(p ? { follow: true, in_port: p } : { follow: false });
});
$('mclkSend').addEventListener('change', () => {
  postMidiClock($('mclkSend').checked ? { send: true, out_port: $('mclkOut').value || null } : { send: false });
});
$('mclkOut').addEventListener('change', () => {
  const p = $('mclkOut').value;
  postMidiClock(p ? { out_port: p } : { send: false, out_port: null });
});

// ---------- Ableton Link (T-154) ----------
// Tempo and phase from a Link session, only when the operator turns it on
// (never at start-up); greyed in a build without the `ableton-link` feature.
let linkTimer = null;
function linkText(lk) {
  const peers = `Pairs : ${lk.peers}`;
  if (lk.state === 'out_of_range') return `Ableton Link : tempo de la session hors plage (${lk.session_bpm != null ? frBpm(lk.session_bpm) : '?'}) · ${peers}`;
  return `Ableton Link${lk.session_bpm != null ? ' : ' + frBpm(lk.session_bpm) : ''} · ${peers}`;
}
function showLink(v) {
  $('linkOn').disabled = !v.available;
  $('linkQuantum').disabled = !v.available;
  $('linkOn').checked = v.enabled;
  $('linkOnState').textContent = v.enabled ? 'Activé' : 'Désactivé';
  if (document.activeElement !== $('linkQuantum')) $('linkQuantum').value = String(v.quantum);
  $('linkState').textContent = !v.available ? v.why : v.enabled ? linkText(v) : 'Tempo du studio (manuel, Tap, Tempo auto ou horloge MIDI).';
}
async function loadLink() {
  clearTimeout(linkTimer);
  try { showLink(await apiJson('GET', '/api/link', undefined, { quiet: true })); } catch { /* shown on the next refresh */ }
  if ($('linkPanel').open) linkTimer = setTimeout(loadLink, 1000);
}
async function postLink(patch) {
  const r = await api('POST', '/api/link', patch, { quiet: true });
  $('linkError').textContent = r.ok ? '' : (await r.text()).trim();
  if (r.ok) showLink(await r.json()); else loadLink();
}
$('linkPanel').addEventListener('toggle', () => { if ($('linkPanel').open) loadLink(); });
$('linkOn').addEventListener('change', () => postLink({ enabled: $('linkOn').checked }));
$('linkQuantum').addEventListener('change', () => postLink({ quantum: +$('linkQuantum').value }));

// ---------- compensation de latence (T-246) ----------
// *Décalage de sortie*: the effects read the beat ahead by this much, so the
// light, late by the DAC's buffer, lands on the beat. It never holds a
// frame back: Échap, the blackout and a disarm act on the next frame.
// `var` and a function declaration: `showTempo` (above) may read them first.
var LAT = { data: null, send: null, ctx: null, timer: null, last: null };
function fmtMs(ms) { return (ms > 0 ? '+' : '') + Math.round(ms) + ' ms'; }
function showLatency() {
  const d = LAT.data;
  if (!d) return;
  const c = d.config;
  if (document.activeElement !== $('latOffset')) $('latOffset').value = Math.round(c.output_offset_ms);
  $('latOffsetV').textContent = fmtMs(c.output_offset_ms);
  $('latEst').textContent = d.output.ms == null ? 'Latence estimée : sans objet' : `Latence estimée : ${Math.round(d.output.ms)} ms`;
  $('latEst').title = d.output.detail;
  $('latAnalysis').textContent = `Retard d'analyse déjà compensé : ${c.analysis_delay_ms.toFixed(1)} ms`;
}
async function loadLatency() {
  try { LAT.data = await apiJson('GET', '/api/latency', undefined, { quiet: true }); showLatency(); } catch { /* shown on the next open */ }
}
async function sendLatency() {
  LAT.send = null;
  try {
    const r = await sendLatest('latency', 'POST', '/api/latency', { output_offset_ms: +$('latOffset').value });
    if (r && r.ok) { LAT.data = await r.json(); showLatency(); }
  } catch { /* unreachable: said by api() */ }
}
function latOffsetMoved() {
  $('latOffsetV').textContent = fmtMs(+$('latOffset').value);
  clearTimeout(LAT.send);
  LAT.send = setTimeout(sendLatency, 60);
}
$('latOffset').addEventListener('input', latOffsetMoved);
$('latOffset').addEventListener('change', latOffsetMoved);
function latClose() {
  $('latMenu').hidden = true;
  $('latBtn').setAttribute('aria-expanded', 'false');
  latEarStop();
}
$('latBtn').addEventListener('click', e => {
  e.stopPropagation();
  if (!$('latMenu').hidden) return latClose();
  $('latMenu').hidden = false;
  $('latBtn').setAttribute('aria-expanded', 'true');
  loadLatency();
});
document.addEventListener('click', e => { if (!$('latMenu').hidden && !e.target.closest('.lat-wrap')) latClose(); });

// ---------- Pilote automatique (T-148) ----------
// A cue of the page (or of the pages ticked) every N bars, in order or at
// random. It only launches cues: never arms; an e-stop or a disarm
// switches it off and it never comes back by itself. The switch, the
// rhythm and the order are controls (vlj.*, allowed in show mode); the
// pages and the public content are an edit (undone, locked in show mode).
let VLJ = null;
function showAutopilot(a) {
  if (!a) return;
  VLJ = a;
  const name = id => (presetById.get(id) || { name: id }).name;
  const state = !a.enabled ? 'coupé' : a.waiting ? 'en attente' : `toutes les ${a.every_bars} mesure${a.every_bars > 1 ? 's' : ''}`;
  document.querySelectorAll('.vlj-open').forEach(btn => {
    btn.classList.toggle('active', !!a.enabled && !a.waiting);
    btn.classList.toggle('waiting', !!a.enabled && !!a.waiting);
    btn.title = `Pilote automatique : ${state}` + (a.enabled && a.last_pick ? ` · dernier cue « ${name(a.last_pick)} »` : '');
  });
  $('vljToggle').setAttribute('aria-pressed', String(!!a.enabled));
  $('vljToggle').classList.toggle('btn--on', !!a.enabled);
  $('vljToggle').textContent = a.enabled ? 'Couper' : 'Activer';
  const every = [1, 2, 4, 8].indexOf(a.every_bars);
  if (document.activeElement !== $('vljEvery') && every >= 0) $('vljEvery').value = String(every);
  if (document.activeElement !== $('vljOrder')) $('vljOrder').value = a.random ? '1' : '0';
  $('vljAudience').checked = !!a.allow_audience;
  $('vljPages').querySelectorAll('input').forEach(c => { c.checked = (a.pages || []).includes(+c.value); });
  $('vljStatus').textContent = !a.enabled ? 'Coupé.'
    : a.waiting ? a.waiting_label
    : (a.next_bar != null ? `Prochain changement à la mesure ${a.next_bar + 1}` : 'Prochain changement au début de la mesure')
      + (a.last_pick ? ` · en cours : « ${name(a.last_pick)} »` : '');
}
function vljRenderPages() {
  const box = $('vljPages');
  box.replaceChildren(...(grid.pages || []).map((p, i) => {
    const row = document.createElement('div');
    row.className = 'row';
    const id = `vljPage${i + 1}`;
    const label = document.createElement('label');
    label.htmlFor = id;
    label.textContent = `${i + 1} · ${p.name || 'Page ' + (i + 1)}`;
    label.title = label.textContent;
    const check = document.createElement('input');
    check.type = 'checkbox';
    check.id = id;
    check.value = String(i + 1);
    check.checked = !!(VLJ && (VLJ.pages || []).includes(i + 1));
    check.addEventListener('change', vljSaveContent);
    row.append(label, check);
    return row;
  }));
}
async function vljSaveContent() {
  const pages = [...$('vljPages').querySelectorAll('input:checked')].map(c => +c.value);
  try {
    const a = await apiJson('POST', '/api/autopilot', { pages, allow_audience: $('vljAudience').checked });
    showAutopilot(a);
  } catch { if (VLJ) showAutopilot(VLJ); /* refused: said by api(), the boxes put back */ }
}
function vljClose() {
  $('vljMenu').hidden = true;
  document.querySelectorAll('.vlj-open').forEach(b => b.setAttribute('aria-expanded', 'false'));
}
// One menu, under the button that opened it (kept inside the window).
document.querySelectorAll('.vlj-open').forEach(btn => btn.addEventListener('click', e => {
  e.stopPropagation();
  const m = $('vljMenu');
  if (!m.hidden && btn.getAttribute('aria-expanded') === 'true') return vljClose();
  vljClose();
  vljRenderPages();
  m.hidden = false;
  btn.setAttribute('aria-expanded', 'true');
  const r = btn.getBoundingClientRect();
  m.style.left = Math.max(8, Math.min(r.right - m.offsetWidth, innerWidth - m.offsetWidth - 8)) + 'px';
  m.style.top = Math.max(8, Math.min(r.bottom + 6, innerHeight - m.offsetHeight - 8)) + 'px';
}));
document.addEventListener('click', e => { if (!$('vljMenu').hidden && !e.target.closest('#vljMenu, .vlj-open')) vljClose(); });
$('vljToggle').addEventListener('click', () => post('/api/control', { id: 'vlj.enabled', value: VLJ && VLJ.enabled ? 0 : 1 }));
$('vljEvery').addEventListener('change', () => post('/api/control', { id: 'vlj.every', value: +$('vljEvery').value }));
$('vljOrder').addEventListener('change', () => post('/api/control', { id: 'vlj.order', value: +$('vljOrder').value }));
$('vljAudience').addEventListener('change', vljSaveContent);
// *Caler à l'oreille*: a click on every beat of the clock, on the Mac, and
// a dot on every beat the effects play (the clock's, `offset` ahead).
function latClick(at, accent) {
  const ctx = LAT.ctx, osc = ctx.createOscillator(), g = ctx.createGain();
  osc.frequency.value = accent ? 1760 : 1320;
  g.gain.setValueAtTime(0.0001, at);
  g.gain.exponentialRampToValueAtTime(0.5, at + 0.002);
  g.gain.exponentialRampToValueAtTime(0.0001, at + 0.05);
  osc.connect(g).connect(ctx.destination);
  osc.start(at);
  osc.stop(at + 0.06);
}
function latEarTick() {
  const t = tempoNow();
  if (!t) return;
  const beatS = 60 / Math.max(20, t.bpm), next = Math.ceil(t.beat), wait = (next - t.beat) * beatS;
  if (LAT.ctx && wait < 0.1 && LAT.last !== next) {
    LAT.last = next;
    latClick(LAT.ctx.currentTime + wait, next % (t.beats_per_bar || 4) === 0);
  }
  const offset = (LAT.data ? LAT.data.config.output_offset_ms : t.output_offset_ms || 0) / 1000;
  const eb = t.beat + offset / beatS;
  $('latFlash').classList.toggle('on', eb - Math.floor(eb) < 0.15);
}
function latEarStart() {
  try {
    LAT.ctx = LAT.ctx || new AudioContext();
    LAT.ctx.resume();
  } catch { LAT.ctx = null; }
  LAT.last = null;
  clearInterval(LAT.timer);
  LAT.timer = setInterval(latEarTick, 20);
  $('latEar').setAttribute('aria-pressed', 'true');
  $('latEar').textContent = 'Arrêter le calage';
  $('latEarHint').hidden = false;
}
function latEarStop() {
  clearInterval(LAT.timer);
  LAT.timer = null;
  $('latFlash').classList.remove('on');
  $('latEar').setAttribute('aria-pressed', 'false');
  $('latEar').textContent = "Caler à l'oreille";
  $('latEarHint').hidden = true;
}
$('latEar').addEventListener('click', () => (LAT.timer ? latEarStop() : latEarStart()));

export { SPEED_NAMES, showAutopilot, showEvolving, showTempo };
