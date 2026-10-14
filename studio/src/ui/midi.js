// Laser Studio — midi.js (T-435).
// Sections (as in the former index.html script):
//   MIDI controllers
//   MIDI learn (T-203)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './venue.js';
import { $, api, apiJson, noticeCondition, post, setShortText } from './core.js';
import { LOCKED_FR, flash, showMode } from './workspaces.js';
import { controlDefaults, controlKinds, sliderLabel, toNeutral } from './direct.js';
import { emptyState } from './command.js';

// ---------- MIDI controllers ----------
const esc = t => String(t).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
let midiProfiles = [], midiKey = '', midiTimer = null, mapsKey = '';
// Refresh now (after a learn/delete request) instead of at the next tick.
function midiNow() { clearTimeout(midiTimer); midiLoop(); }
async function midiLoop() {
  clearTimeout(midiTimer);
  try {
    const m = await apiJson('GET', '/api/midi');
    // Again when a device uses a profile we don't know yet (a learn made a « -perso » copy).
    if (!midiProfiles.length || m.devices.some(d => !midiProfiles.some(p => p.slug === d.profile))) {
      midiProfiles = await apiJson('GET', '/api/midi/profiles');
      midiKey = '';
    }
    const live = m.devices.filter(d => d.connected);
    const st = $('midiStatus');
    const full = !m.enabled ? 'MIDI : désactivé' : live.length
      ? 'MIDI : ' + live.map(d => d.model === 'Unknown' ? d.name : d.model_label).join(', ') + ' connecté' + (live.length > 1 ? 's' : '')
      : 'MIDI : aucun contrôleur';
    // A narrow bar shows the short state (T-517); the title keeps the whole line.
    setShortText(st, full, !m.enabled ? libelle('barre.midi.court.desactive')
      : live.length ? libelle('barre.midi.court.connecte') : libelle('barre.midi.court.aucun'));
    st.title = full + ' — ' + libelle('barre.midi.info');
    st.classList.toggle('on', live.length > 0);
    const lost = m.devices.filter(d => d.lost);
    $('midiLost').textContent = 'Contrôleur MIDI déconnecté' + (lost.length ? ' : ' + lost.map(d => d.name).join(', ') : '');
    $('midiLost').classList.toggle('on', lost.length > 0);
    noticeCondition('midi', lost.length > 0, { level: 'warning', source: 'MIDI', text: $('midiLost').textContent, toast: false });
    if (m.safety) { $('midiAllowArm').checked = m.safety.allow_arm; $('midiBlackoutLost').checked = m.safety.blackout_on_disconnect; }
    // Only rebuild the list when something changed, so an open menu isn't reset.
    const key = JSON.stringify([m.enabled, m.devices.map(d => [d.name, d.connected, d.enabled, d.model, d.profile, d.profile_error, d.pad_colors, d.pulse_by_studio]), m.errors]);
    if (key !== midiKey) { midiKey = key; renderMidi(m); } else showPads(m);
    showLearn(m);
    const mk = JSON.stringify(midiMaps);
    if (mk !== mapsKey) { mapsKey = mk; renderMappings(); }
    showActivity(m);
  } catch (e) { /* studio restarting */ }
  // Every 250 ms while learning, so the banner follows the controller;
  // 400 ms with the controller panel open (activity lights, monitor).
  midiTimer = setTimeout(midiLoop, learnBusy || learnMode ? 250 : $('midiPanel').open ? 400 : 2000);
}
// T-206: what the APC40 mkII's pads show (colour, pulse / blink), as the
// studio drives them; rebuilt only when it changes.
let padsKey = '';
function showPads(m) {
  const key = JSON.stringify(m.devices.map(d => d.pads || null));
  if (key === padsKey) return;
  padsKey = key;
  const led = (l, cls) => `<i class="${cls}${l.mode ? ' ' + l.mode : ''}" data-v="${l.v}"${l.v ? ` style="background:${l.rgb}"` : ''} title="Couleur ${l.v}${l.mode === 'pulse' ? ', pulse' : l.mode === 'blink' ? ', clignote' : ''}"></i>`;
  m.devices.forEach((d, i) => {
    const box = document.querySelector(`[data-mpads="${i}"]`);
    if (!box || !d.pads) return;
    box.innerHTML = d.pads.pads.map((row, r) => row.map(l => led(l, 'pad')).join('') + led(d.pads.scenes[r], 'sc')).join('');
  });
}
// T-211: a light per device that flashes on each message, and the monitor.
const MIDI_KIND_FR = { note_on: 'Note On', note_off: 'Note Off', cc: 'CC', pitch_bend: 'Pitch bend', program_change: 'Programme', sys_ex: 'SysEx' };
function midiMsgLabel(msg) {
  const ch = msg.channel != null ? ` · can. ${msg.channel + 1}` : '';
  switch (msg.kind) {
    case 'note_on': return `Note On ${msg.note} · vél. ${msg.velocity}${ch}`;
    case 'note_off': return `Note Off ${msg.note}${ch}`;
    case 'cc': return `CC ${msg.number} = ${msg.value}${ch}`;
    case 'pitch_bend': return `Pitch bend ${msg.value}${ch}`;
    case 'program_change': return `Programme ${msg.program}${ch}`;
    case 'sys_ex': return `SysEx (${(msg.bytes || []).length} octets)`;
    default: return MIDI_KIND_FR[msg.kind] || msg.kind;
  }
}
// Messages seen per device: a new one lights its dot for half a second,
// however long the poll took.
const midiSeen = {};
function showActivity(m) {
  m.devices.forEach((d, i) => {
    const prev = midiSeen[d.name];
    midiSeen[d.name] = d.messages;
    if (prev != null ? d.messages === prev : !(d.activity_ms < 1500)) return; // nothing new
    const dot = document.querySelector(`[data-mact="${i}"]`);
    if (!dot) return;
    dot.classList.add('on');
    clearTimeout(dot.offTimer);
    dot.offTimer = setTimeout(() => dot.classList.remove('on'), 500);
  });
  if (!$('midiPanel').open) return;
  const recent = (m.recent || []).slice(-12).reverse();
  const html = recent.length
    ? recent.map(e => `<div class="${e.age_ms < 600 ? 'new' : ''}">${esc(e.port)} · ${esc(midiMsgLabel(e.msg))}</div>`).join('')
    : '<div class="hint">Aucun message reçu.</div>';
  if ($('midiMonitor').innerHTML !== html) $('midiMonitor').innerHTML = html;
}
// By port (« Exporter le profil ») or slug (« Exporter la disposition », T-210).
async function exportProfile(port, wanted) {
  const r = await api('POST', '/api/midi/profile/export', wanted ? { slug: wanted } : { port }, { quiet: true });
  if (!r.ok) { showBanner(await r.text(), 5000); return; }
  const { slug, profile } = await r.json();
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([JSON.stringify(profile, null, 2)], { type: 'application/json' }));
  a.download = `${slug}.json`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}
let importPort = null;
$('midiImportFile').onchange = async e => {
  const file = e.target.files[0];
  e.target.value = '';
  if (!file) return;
  let profile;
  try { profile = JSON.parse(await file.text()); } catch (err) { showBanner(`Profil illisible : ${file.name} n'est pas un fichier JSON`, 5000); return; }
  const slug = file.name.replace(/\.json$/i, '');
  const r = await api('POST', '/api/midi/profile/import', { profile, slug, port: importPort }, { quiet: true });
  if (!r.ok) { showBanner(await r.text(), 6000); return; }
  const out = await r.json();
  showBanner(`Profil importé : ${out.slug}` + (importPort ? ` (utilisé par ${importPort})` : ''), 4000);
  midiProfiles = [];
  midiKey = '';
  midiNow();
};
function renderMidi(m) {
  // Grouped: générique, modèles de départ (T-211), APC, profils perso.
  const opt = p => `<option value="${esc(p.slug)}">${esc(p.name || p.slug)}${p.template ? ' (modèle)' : p.builtin ? ' (intégré)' : ' (perso)'}</option>`;
  const group = (label, list) => list.length ? `<optgroup label="${label}">${list.map(opt).join('')}</optgroup>` : '';
  const opts = group('Générique', midiProfiles.filter(p => p.builtin && p.slug === m.default_profile))
    + group('Modèles de départ', midiProfiles.filter(p => p.template))
    + group('Akai APC40', midiProfiles.filter(p => p.builtin && !p.template && p.slug !== m.default_profile))
    + group('Profils perso', midiProfiles.filter(p => !p.builtin));
  $('midiDevices').innerHTML = m.devices.map((d, i) => `
    <div class="mdev ${d.connected ? '' : 'off'}" data-mport="${esc(d.name)}">
      <div class="n"><span class="mact" data-mact="${i}" title="Activité : s'allume à chaque message reçu"></span>${esc(d.name)} <span class="hint">${d.connected ? 'connecté' : 'déconnecté'}</span></div>
      <div class="hint">Modèle détecté : ${esc(d.model_label)}</div>
      <label class="check"><input type="checkbox" data-mdev="${i}" ${d.enabled ? 'checked' : ''}> Activé</label>
      <label class="check"><input type="checkbox" data-mled="${i}" ${d.leds ? 'checked' : ''}> Retour LED</label>
      ${d.model === 'Apc40Mk2' ? `
      <label for="mpadc${i}">Couleurs des pads (mkII)</label>
      <select id="mpadc${i}" data-mpadc="${i}">
        <option value="cue">Couleur de la cue</option>
        <option value="cell">Couleur de la case (comme à l'écran)</option>
        <option value="classic">Vert / jaune (APC40 d'origine)</option>
      </select>
      <label class="check" title="Si les pads de votre APC ne pulsent pas avec l'horloge MIDI du studio"><input type="checkbox" data-mpulse="${i}" ${d.pulse_by_studio ? 'checked' : ''}> Pulsation calculée par le studio</label>
      <div class="apc-prev" data-mpads="${i}" role="img" aria-label="Aperçu des pads de l'APC40 mkII"></div>` : ''}
      <label>Profil</label>
      <div class="row u-mt0"><select data-mprof="${i}">${opts}</select>
        <button class="u-flex-0 u-nowrap" data-mreset="${i}">Réinitialiser le profil</button></div>
      <div class="row"><button data-mexport="${i}">Exporter le profil</button><button data-mimport="${i}">Importer un profil</button></div>
      ${d.profile_error ? `<div class="merr">${esc(d.profile_error)}</div>` : ''}
    </div>`).join('') || '<div class="hint">Aucun appareil MIDI.</div>';
  m.devices.forEach((d, i) => {
    const sel = document.querySelector(`[data-mprof="${i}"]`); if (sel) sel.value = d.profile;
    const pc = document.querySelector(`[data-mpadc="${i}"]`); if (pc) pc.value = d.pad_colors || 'cue';
  });
  padsKey = '';
  showPads(m);
  $('midiHint').innerHTML = (m.enabled ? '' : 'Lancé avec --no-midi : aucun port ouvert. ') + m.errors.map(e => `<div class="merr">${esc(e)}</div>`).join('');
  const portOf = el => m.devices[+Object.values(el.dataset)[0]].name;
  document.querySelectorAll('[data-mdev]').forEach(el => el.onchange = () => post('/api/midi/device', { port: portOf(el), enabled: el.checked }));
  document.querySelectorAll('[data-mled]').forEach(el => el.onchange = () => post('/api/midi/device', { port: portOf(el), leds: el.checked }));
  document.querySelectorAll('[data-mpadc]').forEach(el => el.onchange = async () => { await post('/api/midi/device', { port: portOf(el), pad_colors: el.value }); midiNow(); });
  document.querySelectorAll('[data-mpulse]').forEach(el => el.onchange = async () => { await post('/api/midi/device', { port: portOf(el), pulse_by_studio: el.checked }); midiNow(); });
  document.querySelectorAll('[data-mprof]').forEach(el => el.onchange = () => post('/api/midi/profile', { port: portOf(el), profile: el.value }));
  document.querySelectorAll('[data-mreset]').forEach(el => el.onclick = () => post('/api/midi/profile', { port: portOf(el), profile: null }));
  document.querySelectorAll('[data-mexport]').forEach(el => el.onclick = () => exportProfile(portOf(el)));
  document.querySelectorAll('[data-mimport]').forEach(el => el.onclick = () => { importPort = portOf(el); $('midiImportFile').click(); });
}

// ---------- MIDI learn (T-203) ----------
// Right click on any [data-control] (or « Apprendre MIDI » mode, then a
// click) → the server waits 15 s for the next controller message and maps
// it in the port's profile. Échap cancels (and still stops the laser).
const MODE_FR = { trigger: 'Déclencher', toggle: 'Bascule', momentary: 'Maintenu', absolute: 'Absolu', relative: 'Relatif', grid: 'Grille' };
let controlLabels = {}, midiMaps = [], learnMode = false, learnBusy = false, seenLearn = -1, bannerTimer = null, menuControl = null;
function assignMenuControl(v) { return (menuControl = v); }
// The registry; a slider reset asks for it again if it isn't there yet.
const loadControls = () => apiJson('GET', '/api/controls').then(list => {
  for (const c of (list.controls || list)) {
    controlLabels[c.id] = c.label_fr;
    controlKinds[c.id] = c.kind;
    if (c.kind?.type === 'continuous') controlDefaults[c.id] = c.kind.default;
  }
}).catch(() => {});
let controlsLoaded = loadControls();
function assignControlsLoaded(v) { return (controlsLoaded = v); }
const labelOf = id => controlLabels[id] || id;
const mapLabel = mp => mp.message + (mp.channel != null ? ` · can. ${mp.channel + 1}` : '') + (mp.shift ? ' + Shift' : '');

async function startLearn(target, shift) {
  closeMidiMenu();
  const r = await api('POST', '/api/midi/learn', { target, shift }, { quiet: true });
  if (!r.ok) { showBanner(await r.text(), 6000); return; }
  learnBusy = true;
  midiNow();
}
async function forgetMidi(target) {
  closeMidiMenu();
  const r = await api('POST', '/api/midi/mapping/forget', { target });
  const n = r.ok ? (await r.json()).removed : 0;
  showBanner(n ? `Affectation MIDI oubliée : ${labelOf(target)}` : `Aucune affectation MIDI pour ${labelOf(target)}`, 3000);
  midiNow();
}
function stopLearning() {
  if (!learnBusy && !learnMode) return;
  setLearnMode(false);
  learnBusy = false;
  post('/api/midi/learn/cancel').then(midiNow);
}
function setLearnMode(on) {
  learnMode = on;
  document.body.classList.toggle('learning', on);
  $('midiLearnMode').textContent = on ? 'Terminer' : 'Apprendre MIDI';
  $('midiLearnMode').classList.toggle('active', on);
  if (!on && !learnBusy) $('learnBanner').hidden = true;
  else if (on && !learnBusy) showBanner('Cliquez un contrôle à affecter, puis touchez le contrôleur (Échap pour annuler)');
}
function showBanner(text, ms, buttons = []) {
  $('learnText').textContent = text;
  for (const b of ['learnYes', 'learnNo', 'learnCancel']) $(b).hidden = !buttons.includes(b);
  $('learnBanner').hidden = false;
  clearTimeout(bannerTimer);
  if (ms) bannerTimer = setTimeout(() => { if (!learnBusy) { learnMode ? setLearnMode(true) : ($('learnBanner').hidden = true); } }, ms);
}
function showLearn(m) {
  midiMaps = m.mappings || [];
  // What happened before the page loaded isn't news.
  if (seenLearn < 0) seenLearn = Math.max(0, m.learned?.seq || 0, m.learn_notice?.seq || 0);
  const learned = m.learned && m.learned.seq > seenLearn ? m.learned : null;
  const notice = m.learn_notice && m.learn_notice.seq > seenLearn ? m.learn_notice : null;
  seenLearn = Math.max(seenLearn, m.learned?.seq || 0, m.learn_notice?.seq || 0);
  if (m.learn) {
    learnBusy = true;
    showBanner(`« ${m.learn.target_label} »${m.learn.shift ? ' (avec Shift)' : ''} : touchez un bouton, un potard ou un fader de votre contrôleur… (Échap pour annuler)`, 0, ['learnCancel']);
  } else if (m.learn_conflict) {
    learnBusy = true;
    const c = m.learn_conflict;
    showBanner(`${c.message} pilote déjà « ${c.existing} ». Remplacer l'ancienne affectation (${c.existing}) par « ${c.target_label} » ?`, 0, ['learnYes', 'learnNo']);
  } else {
    const was = learnBusy;
    learnBusy = false;
    if (learned) showBanner(`${mapLabel(learned)} → ${learned.target_label} (${MODE_FR[learned.mode] || learned.mode})` + (learned.replaced ? ` — remplace ${learned.replaced}` : ''), 4000);
    else if (notice) showBanner(notice.text, 5000);
    else if (was) learnMode ? setLearnMode(true) : ($('learnBanner').hidden = true);
  }
  paintPills();
}
// Mapped controls show their message(s) while learning.
function paintPills() {
  const by = {};
  for (const mp of midiMaps) (by[mp.target] ||= []).push(mapLabel(mp));
  document.querySelectorAll('[data-midi]').forEach(el => { if (!by[el.dataset.control]) el.removeAttribute('data-midi'); });
  document.querySelectorAll('[data-midi-pill]').forEach(el => el.removeAttribute('data-midi-pill'));
  document.querySelectorAll('[data-control]').forEach(el => {
    const maps = by[el.dataset.control];
    if (!maps) return;
    el.dataset.midi = maps.join(', ');
    // Inputs can't show ::after: the pill goes on their label.
    const host = el.tagName === 'INPUT' ? sliderLabel(el) : el;
    if (host) host.dataset.midiPill = maps.join(', ');
  });
}
function renderMappings() {
  // T-211: a CC on a fader-like control can be absolute or relative (three
  // encoder encodings); a note or CC can light its LED.
  const ENC = [['absolute', '', 'Absolu'], ['relative', 'twos_complement', 'Relatif (complément à 2)'], ['relative', 'sign_bit', 'Relatif (bit de signe)'], ['relative', 'offset64', 'Relatif (offset 64)']];
  const modeCell = (mp, i) => mp.kind === 'cc' && (mp.mode === 'absolute' || mp.mode === 'relative')
    ? `<select data-menc="${i}" data-port="${esc(mp.port)}" title="Potard / fader (absolu) ou encodeur sans fin (relatif)">${ENC.map(([m, e, l]) =>
        `<option value="${m}:${e}" ${mp.mode === m && (m === 'absolute' || mp.encoding === e) ? 'selected' : ''}>${l}</option>`).join('')}</select>`
    : `<span class="hint hint--inline">${esc(MODE_FR[mp.mode] || mp.mode)}</span>`;
  const ledText = led => led ? [led.off, led.on].concat(led.blink != null ? [led.blink] : []).join(' ') : '';
  // The APC40 drivers light their own LEDs: no generic LED field there.
  const generic = mp => (midiProfiles.find(p => p.slug === mp.profile)?.driver ?? 'generic') === 'generic';
  const ledCell = (mp, i) => (mp.kind === 'note' || mp.kind === 'cc') && generic(mp)
    ? `<input data-mledv="${i}" data-port="${esc(mp.port)}" value="${ledText(mp.led)}" placeholder="LED" size="7" title="Retour LED : éteint allumé (clignotant), ex. 0 127 ; vide = aucun">`
    : '<span></span>';
  const rows = midiMaps.map((mp, i) => `
    <span>${esc(mapLabel(mp))}</span><span>${esc(mp.target_label)}</span>${modeCell(mp, i)}${ledCell(mp, i)}
    <button data-mdel="${i}" title="${esc(mp.port)} · profil ${esc(mp.profile)}">Supprimer</button>`).join('');
  $('midiMappings').innerHTML = rows
    ? `<div class="mmap"><span class="h">Message</span><span class="h">Cible</span><span class="h">Mode</span><span class="h">LED</span><span></span>${rows}</div>`
    : '';
  // T-437: what a mapping is, and the learn mode in one click.
  if (!rows) $('midiMappings').appendChild(emptyState('Aucune affectation. Reliez un bouton ou un curseur de votre contrôleur à un réglage : « Apprendre MIDI », puis cliquez le réglage et touchez le contrôleur (ou clic droit sur un contrôle).',
    [['Apprendre MIDI', () => { if (!learnMode) setLearnMode(true); }]]));
  const update = async (mp, patch) => {
    const r = await api('POST', '/api/midi/mapping/update', { port: mp.port, index: mp.index, ...patch }, { quiet: true });
    if (!r.ok) showBanner(await r.text(), 5000);
    midiNow();
  };
  document.querySelectorAll('[data-mdel]').forEach(el => el.onclick = async () => {
    const mp = midiMaps[+el.dataset.mdel];
    await post('/api/midi/mapping/delete', { port: mp.port, index: mp.index });
    midiNow();
  });
  document.querySelectorAll('[data-menc]').forEach(el => el.onchange = () => {
    const [mode, encoding] = el.value.split(':');
    update(midiMaps[+el.dataset.menc], encoding ? { mode, encoding } : { mode });
  });
  document.querySelectorAll('[data-mledv]').forEach(el => el.onchange = () => {
    const v = el.value.trim().split(/[\s,/;]+/).filter(Boolean).map(Number);
    if (v.some(n => !Number.isInteger(n) || n < 0 || n > 127) || v.length === 1 || v.length > 3) {
      showBanner('LED : 2 ou 3 nombres de 0 à 127 (éteint allumé clignotant), ou vide', 5000);
      return;
    }
    const mp = midiMaps[+el.dataset.mledv];
    // « Cue présent » (templates' grid pads) has no field here: kept as is.
    const present = mp.led?.present != null ? { present: mp.led.present } : {};
    update(mp, { led: v.length ? { off: v[0], on: v[1], ...(v.length > 2 ? { blink: v[2] } : {}), ...present } : null });
  });
}
function openMidiMenu(id, x, y) {
  menuControl = id;
  $('midiMenuName').textContent = labelOf(id) + ' · ' + id;
  const maps = midiMaps.filter(mp => mp.target === id).map(mapLabel);
  $('midiMenuMaps').textContent = maps.length ? 'Affecté : ' + maps.join(', ') : 'Pas d\'affectation MIDI';
  $('midiMenuNeutral').hidden = typeof controlDefaults[id] !== 'number';
  const m = $('midiMenu');
  m.hidden = false;
  m.style.left = Math.max(8, Math.min(x, innerWidth - m.offsetWidth - 8)) + 'px';
  m.style.top = Math.max(8, Math.min(y, innerHeight - m.offsetHeight - 8)) + 'px';
}
function closeMidiMenu() { $('midiMenu').hidden = true; }
document.addEventListener('contextmenu', e => {
  const el = e.target.closest?.('[data-control]');
  // Cues keep their own menu (« Propriétés du cue »), which has the MIDI
  // entries; so do the executors (« Affecter », T-418).
  if (!el || el.closest('#cues, #execRow')) return;
  e.preventDefault(); e.stopPropagation();
  if (showMode) return flash(LOCKED_FR); // T-283: no MIDI learn
  openMidiMenu(el.dataset.control, e.clientX, e.clientY);
}, true);
document.addEventListener('pointerdown', e => {
  if (!$('midiMenu').hidden && !e.target.closest('#midiMenu')) closeMidiMenu();
  // Learn mode: a click picks the control instead of using it.
  if (learnMode && e.target.closest?.('[data-control]')) { e.preventDefault(); e.stopPropagation(); }
}, true);
document.addEventListener('click', e => {
  if (!learnMode) return;
  const el = e.target.closest?.('[data-control]');
  if (!el) return;
  e.preventDefault(); e.stopPropagation();
  startLearn(el.dataset.control, false);
}, true);
$('midiMenuLearn').onclick = () => startLearn(menuControl, false);
$('midiMenuShift').onclick = () => startLearn(menuControl, true);
$('midiMenuForget').onclick = () => forgetMidi(menuControl);
$('midiMenuNeutral').onclick = () => { closeMidiMenu(); toNeutral(menuControl); };
$('cueMenuLearn').onclick = () => { $('cueMenu').hidden = true; startLearn(menuControl, false); };
$('cueMenuShift').onclick = () => { $('cueMenu').hidden = true; startLearn(menuControl, true); };
$('cueMenuForget').onclick = () => { $('cueMenu').hidden = true; forgetMidi(menuControl); };
$('midiLearnMode').onclick = () => { if (learnMode) stopLearning(); else setLearnMode(true); };
$('learnCancel').onclick = stopLearning;
$('learnYes').onclick = () => post('/api/midi/learn/confirm', { replace: true }).then(midiNow);
$('learnNo').onclick = () => post('/api/midi/learn/confirm', { replace: false }).then(midiNow);

$('midiAllowArm').onchange = e => post('/api/midi/safety', { allow_arm: e.target.checked });
// Opening « Contrôleur » switches to the fast refresh at once (activity lights, monitor).
$('midiPanel').addEventListener('toggle', () => { if ($('midiPanel').open) midiNow(); });

export { assignControlsLoaded, assignMenuControl, closeMidiMenu, controlLabels, controlsLoaded, esc,
  exportProfile, labelOf, learnBusy, learnMode, loadControls, menuControl, midiLoop, midiNow,
  showBanner, startLearn, stopLearning };
