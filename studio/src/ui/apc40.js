// Laser Studio — apc40.js (T-435).
// Sections (as in the former index.html script):
//   APC40 à l'écran (T-210)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { closeMidiMenu, esc, exportProfile, labelOf, learnBusy, menuControl, midiNow, showBanner } from './midi.js';
import { $, api, apiJson, noticeCondition, post, sendLatest } from './core.js';
import { LOCKED_FR, activeWs, flash, setWorkspace, showMode } from './workspaces.js';
import { pzToPx, pzZone, takeProjection } from './zones.js';

// ---------- APC40 à l'écran (T-210) ----------
// Our own drawing of the APC40 / mkII (layout from /api/midi/virtual), lit
// like the real one (/api/midi/leds, 10 Hz while « Contrôleur » is open).
// A click sends what the hardware button would (POST /api/midi/virtual):
// same mappings and safety rules, recorded as the UI; it can never arm.
const SVGNS = 'http://www.w3.org/2000/svg';
let vapc = null, vapcKey = '', vapcTimer = null, vapcLayoutAt = 0, vapcLeds = null, vapcMenuEl = null, vapcTargets = null;
const vapcDrag = {}; // element id → position (0..1) while the pointer drags it
// APC40 clip pads (§1.3): 1 green, 2 green blink, 3 red, 4 red blink, 5 yellow, 6 yellow blink.
const VAPC_MK1 = { 1: ['#3ddc84', ''], 2: ['#3ddc84', 'blink'], 3: ['#ff4d5e', ''], 4: ['#ff4d5e', 'blink'], 5: ['#ffd400', ''], 6: ['#ffd400', 'blink'] };
// One-colour LEDs, by button row.
const VAPC_ROW = { rec: '#ff4d5e', solo: '#3d9bff', select: '#ffb020', master: '#ffb020', activator: '#ffd400', ab: '#ff9a3d', stop: '#3ddc84', scene: '#3ddc84' };
const vapcEls = () => vapc ? vapc.elements : [];
const vapcEl = id => vapcEls().find(e => e.id === id);

async function vapcLoadLayout() {
  const l = await apiJson('GET', '/api/midi/virtual');
  vapcLayoutAt = Date.now();
  const key = JSON.stringify(l);
  if (key === vapcKey) return;
  vapcKey = key;
  // Same drawing, other mappings (a page, a learn): updated in place, so
  // an element under the pointer stays. A new drawing is not built under
  // the pointer (T-484): mid-drag or while a button is held, it would drop
  // the element and its listeners, and the moves (and the release) with it.
  if (vapc && vapcShape(l) === vapcShape(vapc) && $('vapc').firstChild) { vapc = l; vapcPending = null; vapcRefresh(); vapcPaint(); return; }
  if (vapcHolds) { vapcPending = l; return; }
  vapc = l;
  vapcBuild();
}
const vapcShape = l => JSON.stringify([l.model, l.width, l.height, l.elements.map(e => [e.id, e.widget, e.x, e.y, e.w, e.h])]);
// A drag or a held button on the drawing; the layout waits for its end.
let vapcHolds = 0, vapcPending = null;
function vapcRelease() {
  vapcHolds = Math.max(0, vapcHolds - 1);
  if (vapcHolds || !vapcPending) return;
  vapc = vapcPending; vapcPending = null;
  vapcBuild();
}
async function vapcLoop() {
  clearTimeout(vapcTimer);
  const visible = $('midiPanel').open && !$('settingsPanel').hidden && !document.hidden;
  if (visible) {
    try {
      if (!vapc || Date.now() - vapcLayoutAt > 2000) await vapcLoadLayout();
      const asked = performance.now();
      const L = await apiJson('GET', '/api/midi/leds');
      // Asked before our Shift was answered: it would undo the box just ticked.
      if (vapcLeds && (vapcShiftBusy || asked < vapcShiftAt)) L.shift = vapcLeds.shift;
      if (L.model !== vapc.model || L.port !== vapc.port) await vapcLoadLayout();
      vapcLeds = L;
      vapcPaint();
    } catch (e) { /* studio restarting */ }
  }
  vapcTimer = setTimeout(vapcLoop, visible ? 100 : 600);
}
const vapcNow = () => { vapcLayoutAt = 0; vapcLoop(); };

function svgEl(tag, attrs, parent) {
  const n = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, v);
  if (parent) parent.appendChild(n);
  return n;
}
// Short text drawn on a button: the number for track rows, else its name.
function vapcText(e) {
  const row = e.id.split('.')[0];
  if (['stop', 'select', 'activator', 'solo', 'rec', 'ab'].includes(row)) return e.id.split('.')[1];
  if (e.widget === 'pad' || e.widget === 'round' || e.widget === 'fader' || e.widget === 'knob') return '';
  return e.label.length > 11 ? e.label.slice(0, 10) + '…' : e.label;
}
function vapcBuild() {
  const box = $('vapc');
  box.innerHTML = '';
  if (!vapc) return;
  const svg = svgEl('svg', { viewBox: `0 0 ${vapc.width} ${vapc.height}`, role: 'img', 'aria-label': `${vapc.model_label} à l'écran` }, box);
  // Two dashed zones, the clip grid and the right-hand controls: our own frame.
  const pads = vapc.elements.filter(e => e.widget === 'pad' || e.widget === 'round');
  const x2 = Math.max(...pads.map(e => e.x + e.w)), y1 = Math.min(...pads.map(e => e.y)), y2 = Math.max(...pads.map(e => e.y + e.h));
  svgEl('rect', { class: 'zone', x: 8, y: y1 - 6, width: x2 - 2, height: y2 - y1 + 12, rx: 8 }, svg);
  for (const e of vapc.elements) {
    const g = svgEl('g', { class: `ve ${e.widget === 'key' ? 'key' : ''} ${e.map ? 'mapped' : ''}`, 'data-vapc': e.id, role: 'img', 'aria-label': e.label }, svg);
    const cx = e.x + e.w / 2, cy = e.y + e.h / 2;
    if (e.widget === 'fader' || e.widget === 'crossfader') {
      const horiz = e.widget === 'crossfader';
      svgEl('rect', { class: 'body', x: e.x, y: e.y, width: e.w, height: e.h, rx: 4, 'fill-opacity': 0 }, g);
      svgEl('rect', { class: 'track', x: horiz ? e.x + 6 : cx - 2, y: horiz ? cy - 2 : e.y + 6, width: horiz ? e.w - 12 : 4, height: horiz ? 4 : e.h - 12, rx: 2 }, g);
      svgEl('rect', { class: 'cap', x: horiz ? cx - 6 : e.x, y: horiz ? e.y : cy - 6, width: horiz ? 12 : e.w, height: horiz ? e.h : 12, rx: 2 }, g);
      if (!horiz) svgEl('text', { x: cx, y: e.y + e.h + 7 }, g).textContent = e.id === 'fader.master' ? 'M' : e.id.split('.')[1];
    } else if (e.widget === 'knob' || e.widget === 'encoder') {
      const r = e.w / 2 - 4;
      svgEl('circle', { class: 'body' + (e.widget === 'encoder' ? ' enc' : ''), cx, cy, r }, g);
      if (e.widget === 'knob') {
        svgEl('path', { class: 'ring', d: vapcArc(cx, cy, r + 3, 1) }, g);
        svgEl('path', { class: 'ring val', d: '' }, g);
      }
      svgEl('line', { class: 'ptr', x1: cx, y1: cy, x2: cx, y2: cy - r + 3 }, g);
      if (e.widget === 'encoder') svgEl('text', { x: cx, y: e.y + e.h + 5 }, g).textContent = e.label;
    } else {
      svgEl('rect', { class: 'body', x: e.x, y: e.y, width: e.w, height: e.h, rx: e.widget === 'round' ? e.h / 2 : e.widget === 'pad' ? 4 : 3 }, g);
      const t = vapcText(e);
      if (t) svgEl('text', { x: cx, y: cy }, g).textContent = t;
    }
    vapcBind(g, e);
  }
  vapcPaint();
  vapcRefresh();
}
// What follows the mappings and the port: « affecté » and the line above.
function vapcRefresh() {
  document.querySelectorAll('#vapc [data-vapc]').forEach(g => g.classList.toggle('mapped', !!vapcEl(g.dataset.vapc)?.map));
  $('vapcModel').value = vapc.model === 'Apc40' ? 'Apc40' : 'Apc40Mk2';
  $('vapcModel').disabled = !!vapc.port;
  $('vapcInfo').textContent = (vapc.port ? `· ${vapc.model_label} branché (${vapc.port})` : '· aucun APC branché : schéma seul')
    + ` · profil « ${vapc.profile_name || vapc.profile} »${vapc.builtin ? ' (intégré)' : ''}`;
}
// Ring of a knob from 7 o'clock (0) to 5 o'clock (1).
function vapcArc(cx, cy, r, f) {
  const a0 = Math.PI * 0.75, a1 = a0 + Math.PI * 1.5 * Math.max(0.001, Math.min(1, f));
  const p = a => [cx + r * Math.cos(a), cy + r * Math.sin(a)].map(v => v.toFixed(1)).join(' ');
  return `M ${p(a0)} A ${r} ${r} 0 ${a1 - a0 > Math.PI ? 1 : 0} 1 ${p(a1)}`;
}
// What one LED shows: [colour or null, animation class].
function vapcLed(e) {
  const L = vapcLeds;
  if (!L || e.kind !== 'note' || e.widget === 'key') return [null, ''];
  const v = L.leds[`note:${e.number}:${e.channel}`] || 0;
  if (!v) return [null, ''];
  const mk2 = vapc.model !== 'Apc40';
  if (mk2 && (e.widget === 'pad' || e.widget === 'round')) {
    const mode = L.modes[`note:${e.number}:0`];
    const anim = !mode ? '' : mode[0] >= 6 && mode[0] <= 10 ? 'pulse' : mode[0] >= 11 ? 'blink' : '';
    return [vapc.palette[v], anim];
  }
  if (e.widget === 'pad') return VAPC_MK1[v] || ['#3ddc84', ''];
  const row = e.widget === 'round' ? 'scene' : e.id.split('.')[0];
  const blink = v === 2 && (row === 'scene' || row === 'stop') ? 'blink' : '';
  return [VAPC_ROW[row] || 'var(--accent)', blink];
}
function vapcPos(e) {
  if (e.id in vapcDrag) return vapcDrag[e.id];
  const v = vapcLeds?.values?.[e.id];
  return typeof v === 'number' ? v : e.widget === 'crossfader' ? 0.5 : 0;
}
function vapcPaint() {
  if (!vapc) return;
  const shift = !!vapcLeds?.shift;
  $('vapcShift').checked = shift;
  document.querySelectorAll('#vapc [data-vapc]').forEach(g => {
    const e = vapcEl(g.dataset.vapc);
    if (!e) return;
    if (e.widget === 'fader' || e.widget === 'crossfader') {
      const f = vapcPos(e), cap = g.querySelector('.cap');
      if (e.widget === 'fader') cap.setAttribute('y', (e.y + 6 + (1 - f) * (e.h - 12) - 6).toFixed(1));
      else cap.setAttribute('x', (e.x + 6 + f * (e.w - 12) - 6).toFixed(1));
      return;
    }
    if (e.widget === 'knob' || e.widget === 'encoder') {
      const cx = e.x + e.w / 2, cy = e.y + e.h / 2, r = e.w / 2 - 4;
      const f = e.widget === 'knob' ? vapcPos(e) : 0.5;
      const a = Math.PI * 0.75 + Math.PI * 1.5 * f;
      const ptr = g.querySelector('.ptr');
      ptr.setAttribute('x2', (cx + (r - 3) * Math.cos(a)).toFixed(1));
      ptr.setAttribute('y2', (cy + (r - 3) * Math.sin(a)).toFixed(1));
      // The ring shows the value the studio would send to the real one.
      const ring = vapcLeds?.leds?.[`cc:${e.number}:${e.channel}`];
      const val = g.querySelector('.ring.val');
      if (val) val.setAttribute('d', ring > 0 ? vapcArc(cx, cy, r + 3, ring / 127) : '');
      return;
    }
    const [color, anim] = vapcLed(e);
    const body = g.querySelector('.body');
    body.style.fill = color || '';
    body.classList.toggle('pulse', anim === 'pulse');
    body.classList.toggle('blink', anim === 'blink');
    g.classList.toggle('lit', !!color);
    g.classList.toggle('shift-on', e.id === 'shift' && shift);
  });
}

// `latest`: a fader or knob value, latest wins per element (T-484).
async function vapcSend(body, latest = false) {
  const r = latest ? await sendLatest('vapc:' + body.id, 'POST', '/api/midi/virtual', body)
    : await api('POST', '/api/midi/virtual', body, { quiet: !!body.learn });
  if (!r) return true; // replaced by a newer value
  if (!r.ok) {
    const t = await r.text();
    if (r.status === 423) flash(t);
    else if (body.learn) showBanner(t, 5000);
    else console.warn('/api/midi/virtual', t);
  }
  return r.ok;
}
// A learn is waiting (« Apprendre MIDI »): the drawing's buttons answer it.
const vapcLearning = () => learnBusy && !$('learnYes').offsetParent;
function vapcBind(g, e) {
  const button = ['pad', 'round', 'button', 'key'].includes(e.widget);
  // The mappings may change under a drawing kept in place: read them now.
  const cur = () => vapcEl(e.id) || e;
  g.addEventListener('pointerenter', ev => vapcTip(cur(), ev));
  g.addEventListener('pointermove', ev => vapcTipMove(ev));
  g.addEventListener('pointerleave', () => { $('vapcTip').hidden = true; });
  g.addEventListener('contextmenu', ev => { ev.preventDefault(); ev.stopPropagation(); vapcOpenMenu(cur(), ev.clientX, ev.clientY); });
  g.addEventListener('pointerdown', ev => {
    if (ev.button !== 0) return;
    ev.preventDefault();
    closeVapcMenu();
    if (vapcLearning()) { vapcSend({ id: e.id, learn: true }).then(() => { midiNow(); vapcNow(); }); return; }
    if (e.id === 'shift') { vapcSetShift(!vapcLeds?.shift); return; }
    try { g.setPointerCapture(ev.pointerId); } catch (err) { /* synthetic pointer */ }
    vapcHolds++;
    if (button) {
      g.classList.add('down');
      // The press leaves at once; the release after the press's answer, so
      // a quick click can't reach the studio as release-then-press.
      const pressed = vapcSend({ id: e.id, down: true }).catch(() => {});
      const up = () => {
        g.classList.remove('down');
        g.removeEventListener('pointerup', up); g.removeEventListener('pointercancel', up);
        pressed.then(() => vapcSend({ id: e.id, down: false }));
        vapcRelease();
      };
      g.addEventListener('pointerup', up); g.addEventListener('pointercancel', up);
      return;
    }
    vapcDragStart(g, e, ev);
  });
  if (e.widget === 'knob' || e.widget === 'encoder') {
    g.addEventListener('wheel', ev => {
      ev.preventDefault();
      const step = ev.deltaY < 0 ? 1 : -1;
      if (e.widget === 'encoder') vapcSend({ id: e.id, delta: step });
      else vapcSend({ id: e.id, value: Math.round(Math.max(0, Math.min(1, vapcPos(e) + step * 2 / 127)) * 127) }, true);
    }, { passive: false });
  }
}
// Faders follow the pointer; knobs turn with a vertical drag; encoders
// send a step every 6 px (steps add up, in any order). A value goes out
// latest wins: one request in flight, the last position always last.
function vapcDragStart(g, e, ev) {
  const svg = g.ownerSVGElement;
  const toUnits = (x, y) => { const p = svg.createSVGPoint(); p.x = x; p.y = y; return p.matrixTransform(svg.getScreenCTM().inverse()); };
  const start = { y: ev.clientY, pos: vapcPos(e), steps: 0 };
  let sent = null;
  const move = mv => {
    if (e.widget === 'encoder') {
      const steps = Math.trunc((start.y - mv.clientY) / 6);
      if (steps !== start.steps) { vapcSend({ id: e.id, delta: steps - start.steps }); start.steps = steps; }
      return;
    }
    let f;
    if (e.widget === 'knob') f = start.pos + (start.y - mv.clientY) / 150;
    else {
      const p = toUnits(mv.clientX, mv.clientY);
      f = e.widget === 'fader' ? 1 - (p.y - e.y - 6) / (e.h - 12) : (p.x - e.x - 6) / (e.w - 12);
    }
    f = Math.max(0, Math.min(1, f));
    vapcDrag[e.id] = f;
    vapcPaint();
    const value = Math.round(f * 127);
    if (value !== sent) { sent = value; vapcSend({ id: e.id, value }, true); }
  };
  const end = () => {
    g.removeEventListener('pointermove', move); g.removeEventListener('pointerup', end); g.removeEventListener('pointercancel', end);
    vapcRelease();
    // Keep showing the dragged position until the studio reports its value.
    setTimeout(() => { delete vapcDrag[e.id]; }, 300);
  };
  g.addEventListener('pointermove', move); g.addEventListener('pointerup', end); g.addEventListener('pointercancel', end);
  if (e.widget !== 'knob' && e.widget !== 'encoder') move(ev);
}
let vapcShiftBusy = 0, vapcShiftAt = 0;
async function vapcSetShift(on) {
  vapcShiftBusy++;
  if (vapcLeds) vapcLeds.shift = on;
  try { await vapcSend({ id: 'shift', down: on }); } finally { vapcShiftBusy--; vapcShiftAt = performance.now(); }
  vapcLoop();
}
function vapcTipHtml(e) {
  const map = e.map ? esc(e.map.label) : '<span class="m">non affecté</span>';
  return `<b>${esc(e.label)} — ${map}</b>`
    + (e.shift_map ? `<br>Shift : ${esc(e.shift_map.label)}` : '')
    + `<br><span class="m">${esc(e.message)}</span>`;
}
function vapcTip(e, ev) {
  const tip = $('vapcTip');
  tip.innerHTML = vapcTipHtml(e);
  tip.hidden = false;
  vapcTipMove(ev);
}
function vapcTipMove(ev) {
  const tip = $('vapcTip');
  if (tip.hidden) return;
  tip.style.left = Math.max(8, Math.min(ev.clientX + 14, innerWidth - tip.offsetWidth - 8)) + 'px';
  tip.style.top = Math.max(8, Math.min(ev.clientY + 16, innerHeight - tip.offsetHeight - 8)) + 'px';
}

// Right click: « Apprendre ici » (pick a control, the element is bound to
// it in the profile the drawing uses) or « Retirer l'affectation ».
async function vapcOpenMenu(e, x, y) {
  if (showMode) return flash(LOCKED_FR); // T-283: no MIDI learn
  $('vapcTip').hidden = true;
  vapcMenuEl = e;
  if (!vapcTargets) {
    try {
      const list = await apiJson('GET', '/api/controls');
      vapcTargets = (list.controls || list).filter(c => c.external && c.id !== 'transport.arm' && !c.id.startsWith('grid.'));
    } catch (err) { vapcTargets = []; }
  }
  $('vapcMenuName').textContent = e.label;
  $('vapcMenuMaps').innerHTML = vapcTipHtml(e).replace(/^<b>[^]*?<\/b><br>?/, '') || '';
  const slot = e.widget === 'pad' ? (+e.id.split('.')[1] - 1) * 8 + (+e.id.split('.')[2] - 1) : null;
  const groups = {};
  for (const c of vapcTargets) (groups[c.group] ||= []).push(c);
  $('vapcMenuTarget').innerHTML = (slot != null ? `<option value="grid:${slot}">Case ${slot + 1} de la grille (cue de la page)</option>` : '')
    + Object.entries(groups).map(([g, list]) => `<optgroup label="${esc(g)}">${list.map(c => `<option value="${esc(c.id)}">${esc(c.label_fr)}</option>`).join('')}</optgroup>`).join('');
  const current = e.map?.target && e.map.target !== 'grid' ? e.map.target : null;
  if (current) $('vapcMenuTarget').value = current;
  $('vapcMenuShift').checked = false;
  $('vapcMenuUnmap').hidden = !e.map || e.id === 'shift';
  $('vapcMenuUnmapShift').hidden = !e.shift_map;
  $('vapcMenuLearn').disabled = e.id === 'shift';
  const m = $('vapcMenu');
  m.hidden = false;
  m.style.left = Math.max(8, Math.min(x, innerWidth - m.offsetWidth - 8)) + 'px';
  m.style.top = Math.max(8, Math.min(y, innerHeight - m.offsetHeight - 8)) + 'px';
}
function closeVapcMenu() { $('vapcMenu').hidden = true; }
document.addEventListener('pointerdown', e => {
  if (!$('vapcMenu').hidden && !e.target.closest('#vapcMenu')) closeVapcMenu();
}, true);
$('vapcMenuLearn').onclick = async () => {
  const e = vapcMenuEl, v = $('vapcMenuTarget').value;
  closeVapcMenu();
  if (!e || !v) return;
  const [target, args] = v.startsWith('grid:') ? ['grid', { slot: +v.slice(5) }] : [v, null];
  const r = await api('POST', '/api/midi/learn', { target, args, shift: $('vapcMenuShift').checked }, { quiet: true });
  if (!r.ok) { showBanner(await r.text(), 6000); return; }
  await vapcSend({ id: e.id, learn: true });
  midiNow();
  vapcNow();
};
async function vapcUnmap(shift) {
  const e = vapcMenuEl;
  closeVapcMenu();
  if (!e) return;
  const r = await api('POST', '/api/midi/virtual/unmap', { id: e.id, shift }, { quiet: true });
  showBanner(r.ok ? `Affectation retirée : ${e.label}${shift ? ' (Shift)' : ''}` : await r.text(), 3000);
  midiNow();
  vapcNow();
}
$('vapcMenuUnmap').onclick = () => vapcUnmap(false);
$('vapcMenuUnmapShift').onclick = () => vapcUnmap(true);
$('vapcShift').onchange = e => vapcSetShift(e.target.checked);
$('vapcModel').onchange = async e => { await post('/api/midi/virtual/model', { model: e.target.value }); vapcNow(); };
$('vapcExport').onclick = () => vapc && exportProfile(null, vapc.profile);
$('midiPanel').addEventListener('toggle', () => { if ($('midiPanel').open) vapcNow(); });
document.addEventListener('visibilitychange', () => { if (!document.hidden) vapcLoop(); });
vapcLoop();

// RÉGLAGES › Entrées réseau (T-013): OSC and Art-Net settings, listener
// state, the OSC monitor and the chart's live DMX values. Polled while open.
const NI_SIGNAL = { off: ['Coupé', ''], waiting: ['En attente de données', 'warn'], ok: ['Signal reçu', 'ok'], lost: ['Signal perdu', 'bad'] };
let netin = null;
function niListen(st, on, cli) {
  if (!on) return ['Coupé', ''];
  if (st.error) return [st.error, 'bad'];
  const where = st.local ? `Écoute sur ${st.local}` : 'Ouverture…';
  return [where + (cli ? ' (imposé par la ligne de commande)' : '') + ` — ${st.packets} paquet(s)` + (st.rejected ? `, ${st.rejected} rejeté(s)` : '') + (st.dropped ? `, ${st.dropped} perdu(s)` : ''), st.local ? 'ok' : 'warn'];
}
function setField(id, v) { if (document.activeElement !== $(id)) $(id).value = v; }
function showNetin(n) {
  netin = n;
  const c = n.config, e = n.effective;
  $('oscEnabled').checked = e.osc.enabled; $('oscEnabled').disabled = n.cli.osc;
  setField('oscBind', c.osc.bind); setField('oscPort', c.osc.port);
  $('artnetEnabled').checked = e.artnet.enabled; $('artnetEnabled').disabled = n.cli.artnet;
  setField('artnetBind', c.artnet.bind); setField('artnetPort', c.artnet.port);
  setField('artnetUniverse', c.artnet.universe); setField('artnetAddress', c.artnet.address);
  if ($('netinPolicy').options.length !== n.policies.length) {
    $('netinPolicy').replaceChildren(...n.policies.map(p => { const o = document.createElement('option'); o.value = p.id; o.textContent = p.label; return o; }));
  }
  setField('netinPolicy', c.loss.policy); setField('netinTimeout', c.loss.timeout_ms / 1000); setField('netinFade', c.loss.fade_ms / 1000);
  showDmxInput(n);
  const [ot, oc] = niListen(n.osc.status, e.osc.enabled, n.cli.osc);
  $('oscState').textContent = ot; $('oscState').className = 'ni-state ' + oc;
  let [at, ac] = niListen(n.artnet.status, e.artnet.enabled, n.cli.artnet);
  if (e.artnet.enabled && !n.artnet.status.error) {
    const [sig, cls] = NI_SIGNAL[n.artnet.signal] || NI_SIGNAL.off;
    at = `${sig}${n.artnet.source ? ' de ' + n.artnet.source : ''} — ${at}`; ac = cls;
  }
  $('artnetState').textContent = at; $('artnetState').className = 'ni-state ' + ac;
  $('dmxChart').replaceChildren(...n.artnet.channels.map(ch => {
    const tr = document.createElement('tr');
    for (const t of [ch.channel, ch.label, ch.value ?? '—']) { const td = document.createElement('td'); td.textContent = t; tr.append(td); }
    return tr;
  }));
  const mon = $('oscMonitor');
  if (!n.osc.recent.length) mon.innerHTML = '<div class="hint">Aucun message reçu.</div>';
  else mon.replaceChildren(...n.osc.recent.map(m => {
    const d = document.createElement('div');
    d.textContent = `${m.address} ${m.args.map(a => JSON.stringify(a)).join(' ')}` + (m.error ? ` — ${m.error}` : '');
    if (m.error) d.className = 'err';
    return d;
  }));
}
async function loadNetin() {
  try { const r = await api('GET', '/api/netin'); if (r.ok) showNetin(await r.json()); } catch {}
}
async function saveNetin(patch) {
  const r = await api('POST', '/api/netin', patch, { quiet: true });
  $('netinError').textContent = r.ok ? '' : await r.text();
  if (r.ok) showNetin(await r.json()); else loadNetin();
}
$('oscEnabled').onchange = e => saveNetin({ osc: { enabled: e.target.checked } });
$('oscBind').onchange = e => saveNetin({ osc: { bind: e.target.value.trim() } });
$('oscPort').onchange = e => saveNetin({ osc: { port: Math.round(+e.target.value) } });
$('artnetEnabled').onchange = e => saveNetin({ artnet: { enabled: e.target.checked } });
$('artnetBind').onchange = e => saveNetin({ artnet: { bind: e.target.value.trim() } });
$('artnetPort').onchange = e => saveNetin({ artnet: { port: Math.round(+e.target.value) } });
$('artnetUniverse').onchange = e => saveNetin({ artnet: { universe: Math.round(+e.target.value) } });
$('artnetAddress').onchange = e => saveNetin({ artnet: { address: Math.round(+e.target.value) } });
$('netinPolicy').onchange = e => saveNetin({ loss: { policy: e.target.value } });
$('netinTimeout').onchange = e => saveNetin({ loss: { timeout_ms: Math.round(+e.target.value * 1000) } });
$('netinFade').onchange = e => saveNetin({ loss: { fade_ms: Math.round(+e.target.value * 1000) } });

// sACN, the free patch and the DMX monitor (T-406).
let dmxIfaces = [], dmxMapsKey = '';
async function loadDmxIfaces() {
  try { const r = await api('GET', '/api/dmx/interfaces'); if (r.ok) dmxIfaces = await r.json(); } catch {}
}
function showDmxInput(n) {
  const c = n.config, sc = c.sacn;
  $('sacnEnabled').checked = sc.enabled;
  const opts = [...dmxIfaces];
  if (!opts.some(i => i.ip === sc.bind)) opts.push({ name: '?', ip: sc.bind });
  const key = opts.map(i => i.ip).join();
  if ($('sacnBind').dataset.key !== key) {
    $('sacnBind').dataset.key = key;
    $('sacnBind').replaceChildren(...opts.map(i => { const o = document.createElement('option'); o.value = i.ip; o.textContent = `${i.ip} (${i.name}${i.loopback ? ', ce Mac seulement' : ''})`; return o; }));
  }
  setField('sacnBind', sc.bind); setField('sacnPort', sc.port);
  setField('sacnUniverses', sc.universes.join(', '));
  $('sacnMulticast').checked = sc.multicast;
  let [st, cls] = niListen(n.sacn.status, sc.enabled, false);
  if (sc.enabled && n.sacn.status.groups.length) st += ` — multidiffusion : ${n.sacn.status.groups.join(', ')}`;
  $('sacnState').textContent = st; $('sacnState').className = 'ni-state ' + cls;
  const acc = c.dmx.access;
  setField('dmxAccessU', acc ? acc.universe : 1); setField('dmxAccessC', acc ? acc.channel : 0);
  showDmxFixtures(n);
  const mk = JSON.stringify([c.dmx.mappings, n.dmx.controls.length]);
  if (mk !== dmxMapsKey && !$('dmxMaps').contains(document.activeElement)) { dmxMapsKey = mk; renderDmxMaps(n); }
  $('dmxLearnState').textContent = n.dmx.learn ? `Apprendre DMX : bougez un canal pour ${labelOf(n.dmx.learn)}…`
    : n.dmx.learned ? `Appris : univers ${n.dmx.learned.universe}, canal ${n.dmx.learned.channel} → ${labelOf(n.dmx.learned.control)}` : '';
}
function renderDmxMaps(n) {
  const maps = n.config.dmx.mappings;
  const head = '<tr><th>Univers</th><th>Canal</th><th>Contrôle</th><th>Fin (16 bits)</th><th class="c">Inversé</th><th class="c">Reprise</th><th></th></tr>';
  if (!maps.length) { $('dmxMaps').innerHTML = head + '<tr><td colspan="7" class="hint">Aucune affectation.</td></tr>'; return; }
  const options = n.dmx.controls.map(d => `<option value="${esc(d.id)}">${esc(d.label)} · ${esc(d.id)}</option>`).join('');
  $('dmxMaps').innerHTML = head + maps.map((m, i) => `<tr data-i="${i}">
    <td><input type="number" data-k="universe" min="0" max="63999" value="${m.universe}" aria-label="Univers"></td>
    <td><input type="number" data-k="channel" min="1" max="512" value="${m.channel}" aria-label="Canal"></td>
    <td><select data-k="control" aria-label="Contrôle">${options}</select></td>
    <td><input type="number" data-k="fine" min="0" max="512" value="${m.fine ?? ''}" placeholder="—" aria-label="Canal fin (16 bits)"></td>
    <td class="c"><input type="checkbox" data-k="invert" ${m.invert ? 'checked' : ''} aria-label="Inversé"></td>
    <td class="c"><input type="checkbox" data-k="pickup" ${m.pickup ? 'checked' : ''} aria-label="Reprise"></td>
    <td><button data-del="${i}" title="Supprimer">✕</button></td></tr>`).join('');
  $('dmxMaps').querySelectorAll('tr[data-i]').forEach(tr => { tr.querySelector('select').value = maps[+tr.dataset.i].control; });
}
function dmxMappings() { return JSON.parse(JSON.stringify(netin?.config.dmx.mappings || [])); }
$('dmxMaps').addEventListener('change', e => {
  const tr = e.target.closest('tr[data-i]'); if (!tr) return;
  const maps = dmxMappings(), m = maps[+tr.dataset.i], k = e.target.dataset.k;
  if (k === 'invert' || k === 'pickup') m[k] = e.target.checked;
  else if (k === 'control') m.control = e.target.value;
  else if (k === 'fine') m.fine = +e.target.value > 0 ? Math.round(+e.target.value) : null;
  else m[k] = Math.round(+e.target.value);
  dmxMapsKey = ''; e.target.blur(); saveNetin({ dmx: { mappings: maps } });
});
$('dmxMaps').addEventListener('click', e => {
  const b = e.target.closest('[data-del]'); if (!b) return;
  const maps = dmxMappings(); maps.splice(+b.dataset.del, 1);
  dmxMapsKey = ''; b.blur(); saveNetin({ dmx: { mappings: maps } });
});
$('dmxAdd').onclick = () => {
  const maps = dmxMappings(), last = maps[maps.length - 1];
  maps.push({ universe: last ? last.universe : 1, channel: last ? Math.min(512, last.channel + 1) : 1, fine: null, control: 'master.brightness', invert: false, pickup: true });
  dmxMapsKey = ''; saveNetin({ dmx: { mappings: maps } });
};
function saveAccess() {
  const ch = Math.round(+$('dmxAccessC').value);
  saveNetin({ dmx: { access: ch > 0 ? { universe: Math.round(+$('dmxAccessU').value), channel: ch } : null } });
}
$('dmxAccessU').onchange = saveAccess; $('dmxAccessC').onchange = saveAccess;
$('sacnEnabled').onchange = e => saveNetin({ sacn: { enabled: e.target.checked } });
$('sacnBind').onchange = e => saveNetin({ sacn: { bind: e.target.value } });
$('sacnPort').onchange = e => saveNetin({ sacn: { port: Math.round(+e.target.value) } });
$('sacnMulticast').onchange = e => saveNetin({ sacn: { multicast: e.target.checked } });
$('sacnUniverses').onchange = e => saveNetin({ sacn: { universes: e.target.value.split(/[ ,;]+/).filter(Boolean).map(Number) } });
// Appareils DMX (T-407): one instance per projection zone, our
// published personality. Nothing here arms; the server refuses overlaps.
let dmxFixKey = '';
const FIX_SIGNAL = { off: ['coupé', ''], waiting: ['en attente', ''], ok: ['reçu', 'ok'], lost: ['signal perdu', 'bad'] };
function dmxFixtureList() { return JSON.parse(JSON.stringify(netin?.config.dmx.fixtures || [])); }
function showDmxFixtures(n) {
  const fx = n.dmx.fixtures, list = n.config.dmx.fixtures || [];
  const key = JSON.stringify([list, fx.zones]);
  if (key !== dmxFixKey && !$('dmxFixtures').contains(document.activeElement)) { dmxFixKey = key; renderDmxFixtures(fx, list); }
  // Live status only (no re-render while editing).
  fx.list.forEach((f, i) => {
    const td = $('dmxFixtures').querySelector(`tr[data-i="${i}"] .st`); if (!td) return;
    const [sig, cls] = FIX_SIGNAL[f.signal] || FIX_SIGNAL.off;
    td.textContent = `${f.fixture.address}–${f.last} · ${sig}` + (f.signal === 'ok' ? (f.applied ? ' · appliqué' : f.access ? ' · porte ou périmètre fermé' : ' · accès baissé') : '');
    td.className = 'st ' + (f.signal === 'ok' && f.applied ? 'ok' : cls === 'bad' ? 'bad' : '');
  });
  $('dmxFixAdd').disabled = list.length >= fx.zones.length;
}
function renderDmxFixtures(fx, list) {
  const head = '<tr><th>Zone</th><th>Personnalité</th><th>Univers</th><th>Adresse</th><th>Lissage (ms)</th><th class="c">Activé</th><th>État</th><th></th></tr>';
  if (!list.length) { $('dmxFixtures').innerHTML = head + '<tr><td colspan="8" class="hint">Aucun appareil.</td></tr>'; return; }
  const zones = fx.zones.map(z => `<option value="${z.id}">${esc(z.name)}</option>`).join('');
  const modes = fx.personalities.map(p => `<option value="${p.id}">${esc(p.label)}</option>`).join('');
  $('dmxFixtures').innerHTML = head + list.map((f, i) => `<tr data-i="${i}">
    <td><select data-k="zone" aria-label="Zone">${zones}${fx.zones.some(z => z.id === f.zone) ? '' : `<option value="${f.zone}">Zone ${f.zone} (absente)</option>`}</select></td>
    <td><select data-k="mode" aria-label="Personnalité">${modes}</select></td>
    <td><input type="number" data-k="universe" min="0" max="63999" value="${f.universe}" aria-label="Univers"></td>
    <td><input type="number" data-k="address" min="1" max="512" value="${f.address}" aria-label="Adresse"></td>
    <td><input type="number" data-k="smoothing_ms" min="0" max="1000" step="10" value="${f.smoothing_ms}" aria-label="Lissage (ms)"></td>
    <td class="c"><input type="checkbox" data-k="enabled" ${f.enabled ? 'checked' : ''} aria-label="Activé"></td>
    <td class="st"></td>
    <td><button data-del="${i}" title="Supprimer">✕</button></td></tr>`).join('');
  $('dmxFixtures').querySelectorAll('tr[data-i]').forEach(tr => {
    const f = list[+tr.dataset.i];
    tr.querySelector('[data-k=zone]').value = f.zone; tr.querySelector('[data-k=mode]').value = f.mode;
  });
}
$('dmxFixtures').addEventListener('change', e => {
  const tr = e.target.closest('tr[data-i]'); if (!tr) return;
  const list = dmxFixtureList(), f = list[+tr.dataset.i], k = e.target.dataset.k;
  if (k === 'enabled') f.enabled = e.target.checked;
  else if (k === 'mode') f.mode = e.target.value;
  else f[k] = Math.round(+e.target.value);
  dmxFixKey = ''; e.target.blur(); saveNetin({ dmx: { fixtures: list } });
});
$('dmxFixtures').addEventListener('click', e => {
  const b = e.target.closest('[data-del]'); if (!b) return;
  const list = dmxFixtureList(); list.splice(+b.dataset.del, 1);
  dmxFixKey = ''; b.blur(); saveNetin({ dmx: { fixtures: list } });
});
$('dmxFixAdd').onclick = () => {
  const list = dmxFixtureList(), zones = netin?.dmx.fixtures.zones || [];
  const zone = zones.find(z => !list.some(f => f.zone === z.id)); if (!zone) return;
  // After the last instance of universe 1 (the server refuses an overlap).
  const next = Math.max(1, ...list.filter(f => f.universe === 1).map(f => f.address + (f.mode === 'full' ? 32 : 16)));
  list.push({ enabled: true, zone: zone.id, mode: 'compact', universe: 1, address: Math.min(next, 497), smoothing_ms: 60 });
  dmxFixKey = ''; saveNetin({ dmx: { fixtures: list } });
};
$('dmxFixChart').onclick = async () => {
  const box = $('dmxFixChartBox');
  if (!box.hidden) { box.hidden = true; $('dmxFixChart').setAttribute('aria-expanded', 'false'); return; }
  let j;
  try { const r = await api('GET', '/api/dmx/personality'); if (!r.ok) return; j = await r.json(); } catch { return; }
  box.innerHTML = j.modes.map(m => `<h4>${esc(m.label)} (table v${j.version})</h4><table><tr><th>Canal</th><th>Nom</th><th>Défaut</th><th>Valeurs</th></tr>${
    m.channels.map(c => `<tr><td>${c.channel}</td><td>${esc(c.label)}${c.smoothed ? ' <span class="hint">(lissé)</span>' : ''}</td><td>${c.default}</td><td>${esc(c.values)}</td></tr>`).join('')}</table>`).join('');
  box.hidden = false; $('dmxFixChart').setAttribute('aria-expanded', 'true');
};
$('dmxFixGdtf').onclick = () => {
  const a = document.createElement('a');
  a.href = '/api/dmx/personality?format=gdtf';
  a.download = '';
  document.body.append(a); a.click(); a.remove();
};
const FIX_COLORS = ['var(--accent-2)', 'var(--violet)', 'var(--warn)', 'var(--ok)', 'var(--ws-timeline)', 'var(--ws-live)', 'var(--accent)', 'var(--danger)'];
async function loadDmxMonitor() {
  const u = Math.round(+$('dmxMonU').value) || 0;
  let m;
  try { const r = await api('GET', '/api/dmx/monitor?universe=' + u); if (!r.ok) return; m = await r.json(); } catch { return; }
  const grid = $('dmxMonGrid');
  if (grid.children.length !== 512) grid.replaceChildren(...Array.from({ length: 512 }, (_, i) => { const c = document.createElement('span'); c.title = 'Canal ' + (i + 1); return c; }));
  const vals = m.values || [];
  for (let i = 0; i < 512; i++) {
    const v = vals[i], c = grid.children[i];
    c.textContent = v == null ? '·' : v;
    c.classList.toggle('on', !!v);
    c.style.setProperty('--v', ((v || 0) / 255).toFixed(2));
    c.classList.remove('fx', 'fx-start'); c.style.removeProperty('--fx'); c.title = 'Canal ' + (i + 1);
  }
  // The fixture instances on this universe: framed and named.
  const fixes = m.fixtures || [];
  fixes.forEach((f, k) => {
    for (let ch = f.start; ch <= f.last && ch <= 512; ch++) {
      const c = grid.children[ch - 1];
      c.classList.add('fx'); if (ch === f.start) c.classList.add('fx-start');
      c.style.setProperty('--fx', FIX_COLORS[k % FIX_COLORS.length]);
      c.title = `Canal ${ch} — zone ${f.zone} : ${f.channels[ch - f.start]}`;
    }
  });
  $('dmxMonFix').replaceChildren(...fixes.map((f, k) => {
    const d = document.createElement('span');
    d.style.setProperty('--fx', FIX_COLORS[k % FIX_COLORS.length]);
    const zone = netin?.dmx.fixtures.zones.find(z => z.id === f.zone);
    d.textContent = `${zone ? zone.name : 'Zone ' + f.zone} — ${f.label}, canaux ${f.start}–${f.last}${f.enabled ? '' : ' (désactivé)'}`;
    return d;
  }));
  $('dmxMonState').textContent = !m.values ? `Univers ${u} : rien reçu (il doit figurer dans le patch ou les univers sACN)`
    : `Univers ${u} : ${m.lost ? 'signal perdu' : 'reçu'} de ${m.source} (${m.protocol === 'sacn' ? 'sACN' : 'Art-Net'}) — ${m.rate.toFixed(1)} paquets/s`;
  $('dmxMonState').className = 'ni-state ' + (!m.values ? '' : m.lost ? 'bad' : 'ok');
  $('dmxSources').replaceChildren(...m.sources.map(src => {
    const d = document.createElement('div');
    d.textContent = `${src.active ? '● ' : '○ '}${src.name} — ${src.protocol === 'sacn' ? 'sACN' : 'Art-Net'} ${src.addr}` + (src.protocol === 'sacn' ? `, priorité ${src.priority}` : '') + `, il y a ${(src.age_ms / 1000).toFixed(1)} s`;
    if (src.active) d.className = 'active';
    return d;
  }));
}
$('dmxMonU').onchange = loadDmxMonitor;
$('netinPanel').addEventListener('toggle', () => { if ($('netinPanel').open) { loadDmxIfaces().then(loadNetin); loadDmxMonitor(); } });
setInterval(() => { if (activeWs === 'settings' && $('netinPanel').open && !document.hidden) loadDmxMonitor(); }, 500);

// Entrées externes (T-408): the top-bar gate, the DMX pill and banner,
// and RÉGLAGES › Entrées externes. The gate is never saved: closed at
// every start and after an emergency stop; nothing external can arm.
let ext = null, extKey = '';
function showExt(x) {
  ext = x;
  const b = $('extGate');
  b.classList.toggle('open', x.open);
  b.setAttribute('aria-pressed', String(x.open));
  b.querySelector('.long').textContent = 'Entrées externes ' + (x.open ? 'ouvertes' : 'fermées');
  b.querySelector('.short').textContent = 'Ext.';
  const active = x.sources.filter(src => src.access || !src.require_access).map(src => src.label);
  b.title = (x.open ? 'Entrées externes ouvertes' + (active.length ? ' · sources actives : ' + active.join(', ') : ' · aucun accès levé')
    : 'Entrées externes fermées : DMX, OSC et timecode ne changent rien (sauf le noir et l\'arrêt d\'urgence)') + ' · clic ou Alt+X';
  const d = x.dmx, pill = $('dmxPill');
  pill.hidden = !d.enabled;
  pill.className = d.lost ? 'lost' : d.receiving ? 'ok' : '';
  pill.title = (d.lost ? 'Signal DMX perdu' : d.receiving ? `DMX reçu (${d.sources} source(s))` : 'DMX : en attente de données') + ' · clic = RÉGLAGES › Entrées réseau';
  $('dmxLost').textContent = 'Signal DMX perdu — ' + (d.lost_source || '');
  $('dmxLost').classList.toggle('on', !!d.lost);
  noticeCondition('dmx', !!d.lost, { level: 'warning', source: 'MIDI', text: $('dmxLost').textContent, toast: false });
  if ($('extPanel').open) renderExt(x);
}
function renderExt(x) {
  const key = JSON.stringify(x.sources.map(src => [src.groups, src.require_access]));
  if (key !== extKey && !$('extTable').contains(document.activeElement)) {
    extKey = key;
    $('extTable').innerHTML = '<tr><th>Source</th>' + x.sets.map(set => `<th class="c">${esc(set.label)}</th>`).join('')
      + '<th class="c">Exiger l\'accès</th><th>Accès</th><th class="c">Refusés</th></tr>'
      + x.sources.map(src => `<tr data-src="${src.source}"><td>${esc(src.label)}</td>`
        + x.sets.map(set => `<td class="c"><input type="checkbox" data-set="${set.id}" ${src.sets.includes(set.id) ? 'checked' : ''} aria-label="${esc(src.label)} : ${esc(set.label)}"></td>`).join('')
        + `<td class="c"><input type="checkbox" data-access ${src.require_access ? 'checked' : ''} aria-label="${esc(src.label)} : exiger l'accès"></td>`
        + '<td class="acc"></td><td class="c ref"></td></tr>').join('');
  }
  for (const src of x.sources) {
    const tr = $('extTable').querySelector(`tr[data-src="${src.source}"]`); if (!tr) continue;
    const acc = tr.querySelector('.acc');
    acc.textContent = !src.require_access ? 'non exigé' : src.access ? 'levé' + (src.clients.length ? ' (' + src.clients.join(', ') + ')' : '') : 'fermé';
    acc.classList.toggle('up', src.access || !src.require_access);
    const ref = tr.querySelector('.ref');
    ref.textContent = src.refused;
    ref.title = src.last_refusal ? `Dernier : ${src.last_refusal.what} — ${src.last_refusal.why_fr}` : '';
  }
}
$('extTable').addEventListener('change', async e => {
  const tr = e.target.closest('tr[data-src]'); if (!tr || !ext) return;
  const groups = [...tr.querySelectorAll('[data-set]:checked')].flatMap(el => ext.sets.find(set => set.id === el.dataset.set).groups);
  const body = { scopes: [{ source: tr.dataset.src, groups, require_access: tr.querySelector('[data-access]').checked }] };
  e.target.blur();
  const r = await api('POST', '/api/external/scopes', body, { quiet: true });
  $('extError').textContent = r.ok ? '' : await r.text();
  extKey = '';
  if (r.ok) showExt(await r.json()); else loadExt();
});
async function loadExt() {
  try { const r = await api('GET', '/api/external'); if (r.ok) showExt(await r.json()); } catch { /* studio restarting */ }
}
async function setGate(on, source) {
  try {
    const r = await api('POST', '/api/external/gate', { on, source }, { quiet: true });
    if (r.ok) return showExt(await r.json());
    const j = await r.json().catch(() => null);
    flash(j?.error || 'Refusé');
  } catch {}
  loadExt();
}
$('extGate').onclick = () => setGate(!(ext && ext.open), 'ui');
$('dmxPill').onclick = () => { if (showMode) return flash(LOCKED_FR); setWorkspace('settings'); $('netinPanel').open = true; };
$('extPanel').addEventListener('toggle', () => { if ($('extPanel').open) { extKey = ''; loadExt(); } });
setInterval(() => { if (!document.hidden) loadExt(); }, 1000);
loadExt();
// Machine de secours (T-456): the top bar from every frame (`f.standby`),
// the panel from GET /api/standby. Taking over never arms the laser.
let standby = null;
let standbyBar = null;
const BACKUP_LOST_FR = 'Machine de secours injoignable : laser désarmé — elle peut prendre le relais. Arrêt réel : matériel';
function standbyLinkText(v) {
  if (v.role === 'standby') {
    if (!v.paired) return 'Non appairée : saisissez le code de la machine principale';
    return v.link ? `Lien : établi — retard ${Math.round(v.lag_ms)} ms` : `Lien perdu depuis ${v.lost_s ?? 0} s`;
  }
  if (v.role === 'primary') {
    if (v.conflict) return 'Une autre machine est principale : choisissez laquelle reste principale';
    if (v.backup_lost) return BACKUP_LOST_FR;
    return v.link ? 'Lien : établi' : 'Lien : aucune réponse de la machine de secours';
  }
  return v.demoted || 'Autonome : aucune synchronisation, aucun port ouvert';
}
function showStandbyBar(v) {
  standbyBar = v;
  const pill = $('standbyPill');
  pill.hidden = v.role === 'standalone' && !v.demoted;
  pill.classList.toggle('standby', v.role === 'standby');
  pill.classList.toggle('warn', v.role !== 'standby' && (!!v.conflict || !!v.demoted || !!v.backup_lost));
  pill.textContent = v.role === 'standby'
    ? 'SECOURS — en veille' + (v.paired && !v.link ? ` · Lien perdu depuis ${v.lost_s ?? 0} s` : '') + (v.project_mismatch ? ' · Projet différent' : '')
    : v.role === 'primary' ? 'PRINCIPALE' + (v.conflict ? ' · conflit' : '') + (v.backup_lost ? ' · Secours injoignable : désarmé' : '') : 'Autonome (relais pris ailleurs)';
  pill.title = standbyLinkText(v) + ' — clic = RÉGLAGES › Machine de secours';
  $('takeoverBtn').hidden = v.role !== 'standby';
  document.body.classList.toggle('standby-mode', v.role === 'standby');
}
function showStandby(v) {
  standby = v;
  showStandbyBar(v);
  if (document.activeElement !== $('sbRole')) $('sbRole').value = v.role;
  if (document.activeElement !== $('sbPeer')) $('sbPeer').value = v.peer || '';
  if (document.activeElement !== $('sbPort')) $('sbPort').value = v.bound_port || v.port;
  if (document.activeElement !== $('sbGrace')) $('sbGrace').value = v.takeover_grace_s;
  const ifs = ['', ...v.interfaces];
  if (v.interface && !ifs.includes(v.interface)) ifs.push(v.interface);
  const key = ifs.join(',');
  if ($('sbInterface').dataset.key !== key && document.activeElement !== $('sbInterface')) {
    $('sbInterface').dataset.key = key;
    $('sbInterface').innerHTML = ifs.map(i => `<option value="${esc(i)}">${i ? esc(i) : 'Toutes les interfaces'}</option>`).join('');
  }
  if (document.activeElement !== $('sbInterface')) $('sbInterface').value = v.interface || '';
  $('sbPrimary').hidden = v.role !== 'primary';
  $('sbCode').textContent = v.code || '––––––';
  $('sbPaired').textContent = v.standby_paired ? `Machine de secours appairée : ${v.paired_addr || ''}` : 'Aucune machine de secours appairée';
  $('sbBackup').hidden = v.role !== 'standby';
  $('sbMismatch').hidden = !v.project_mismatch;
  $('sbLink').textContent = standbyLinkText(v) + (v.role === 'standby' && v.last_error ? ` (${v.last_error})` : '');
}
async function loadStandby() {
  try { const r = await api('GET', '/api/standby', undefined, { quiet: true }); if (r.ok) showStandby(await r.json()); } catch { /* studio restarting */ }
}
async function standbyPost(path, body) {
  try {
    const r = await api('POST', path, body, { quiet: true });
    const text = r.ok ? '' : await r.text();
    $('sbError').textContent = text;
    if (r.ok) showStandby(await r.json());
    else loadStandby();
    return r.ok ? null : text;
  } catch { return 'studio injoignable'; }
}
$('sbRole').addEventListener('change', () => standbyPost('/api/standby', { role: $('sbRole').value }));
$('sbPeer').addEventListener('change', () => standbyPost('/api/standby', { peer: $('sbPeer').value.trim() }));
$('sbInterface').addEventListener('change', () => standbyPost('/api/standby', { interface: $('sbInterface').value }));
$('sbPort').addEventListener('change', () => {
  const port = Number($('sbPort').value);
  if (!Number.isInteger(port) || port < 1024 || port > 65535) { $('sbError').textContent = 'Port : 1024 à 65535'; return; }
  standbyPost('/api/standby', { port });
});
$('sbGrace').addEventListener('change', () => {
  const g = Number($('sbGrace').value);
  if (!Number.isInteger(g) || g < 1 || g > 60) { $('sbError').textContent = 'Délai de reprise : 1 à 60 s'; return; }
  standbyPost('/api/standby', { takeover_grace_s: g });
});
$('sbPairBtn').onclick = () => standbyPost('/api/standby/pair', { code: $('sbPairCode').value.trim() }).then(e => { if (!e) $('sbPairCode').value = ''; });
$('sbReceiveBtn').onclick = () => standbyPost('/api/standby/receive');
// No shortcut: a voluntary action, always confirmed.
$('takeoverBtn').onclick = async () => {
  const v = standbyBar || {};
  const held = v.peer_held_s == null ? 'La machine principale n\'a pas été vue tenant une sortie depuis l\'appairage.'
    : `La machine principale tenait une sortie il y a ${v.peer_held_s} s${v.peer_holds_output ? ' (dernière réponse)' : ''}.`;
  // Unpaired (a main machine restarted after a crash forgets the pairing)
  // or no state since pairing: no lease, it never disarms by itself.
  const unfenced = v.main_fenced ? '' : '\n\nAttention : cette machine de secours n\'est pas appairée avec la principale actuelle (non appairée : la principale ne se désarme pas d\'elle-même). Elle peut encore être armée : coupez sa sortie avant de continuer.';
  if (!confirm('Prendre le relais ? Le laser restera désarmé : vous devrez l\'armer vous-même.\n\n'
    + 'Vérifiez d\'abord que la machine principale est arrêtée ou que sa sortie est coupée (arrêt matériel).\n\n' + held + unfenced)) return;
  const err = await standbyPost('/api/standby/takeover');
  flash(err ? err : 'Cette machine est maintenant la principale. Laser désarmé.');
};
$('standbyPill').onclick = () => { if (showMode) return flash(LOCKED_FR); setWorkspace('settings'); $('standbyPanel').open = true; $('standbyPanel').scrollIntoView({ block: 'nearest' }); loadStandby(); };
setInterval(() => { if (!document.hidden && $('standbyPanel').open) loadStandby(); }, 1000);
$('standbyPanel').addEventListener('toggle', () => { if ($('standbyPanel').open) loadStandby(); });
loadStandby();

// Télécommande (T-428): RÉGLAGES › Télécommande and the top-bar pill.
// Off at every start; the code and the remotes are shown here only.
let remote = null;
function showRemote(v) {
  remote = v;
  const pill = $('remotePill');
  pill.hidden = !v.enabled;
  pill.textContent = 'Télécommande : ' + v.sessions.length;
  if (document.activeElement !== $('remoteOn')) $('remoteOn').checked = v.enabled;
  if (document.activeElement !== $('remotePort')) $('remotePort').value = v.bound_port || v.port;
  $('remoteActive').hidden = !v.enabled;
  $('remoteCode').textContent = v.enabled ? v.code : '––––––';
  $('remoteUrls').textContent = v.urls.length ? v.urls.join(' · ') : 'aucune adresse réseau trouvée';
  $('remoteBlocked').textContent = v.blocked_s ? `Trop de codes faux : nouveau code, appairage bloqué ${Math.ceil(v.blocked_s)} s` : '';
  $('remoteCount').textContent = v.sessions.length;
  const key = JSON.stringify(v.sessions.map(r => [r.name, r.addr, Math.round(r.idle_s)]));
  if ($('remoteList').dataset.key !== key && !$('remoteList').contains(document.activeElement)) {
    $('remoteList').dataset.key = key;
    $('remoteList').innerHTML = v.sessions.map(r => `<li><span>${esc(r.name)} — ${esc(r.addr)} — il y a ${Math.round(r.idle_s)} s</span>`
      + `<button data-kick="${esc(r.name)}" aria-label="Déconnecter ${esc(r.name)}">Déconnecter</button></li>`).join('');
  }
}
async function loadRemote() {
  try { const r = await api('GET', '/api/remote', undefined, { quiet: true }); if (r.ok) showRemote(await r.json()); } catch { /* studio restarting */ }
}
async function setRemote(body) {
  try {
    const r = await api('POST', '/api/remote', body, { quiet: true });
    $('remoteError').textContent = r.ok ? '' : await r.text();
    if (r.ok) return showRemote(await r.json());
  } catch {}
  loadRemote();
}
$('remoteOn').addEventListener('change', () => setRemote({ enabled: $('remoteOn').checked, port: Number($('remotePort').value) || undefined }));
$('remotePort').addEventListener('change', () => {
  const port = Number($('remotePort').value);
  if (!Number.isInteger(port) || port < 1024 || port > 65535) { $('remoteError').textContent = 'Port : 1024 à 65535'; return; }
  setRemote({ enabled: !!(remote && remote.enabled), port });
});
async function kickRemote(body) {
  try { const r = await api('POST', '/api/remote/kick', body, { quiet: true }); if (r.ok) showRemote(await r.json()); } catch {}
}
$('remoteList').addEventListener('click', e => { const b = e.target.closest('[data-kick]'); if (b) kickRemote({ name: b.dataset.kick }); });
$('remoteKickAll').onclick = () => kickRemote({});
$('remotePill').onclick = () => { if (showMode) return flash(LOCKED_FR); setWorkspace('settings'); $('remotePanel').open = true; $('remotePanel').scrollIntoView({ block: 'nearest' }); };
setInterval(() => { if (!document.hidden) loadRemote(); }, 1000);
loadRemote();
// Alignement depuis la télécommande (T-460): RÉGLAGES › Zones de
// projection. The phone moves a draft; only « Appliquer » here writes the
// zone. The session comes with the state (`f.align`, T-411).
let phoneAlign = null;
function paRemotes() {
  const names = remote && remote.enabled ? remote.sessions.map(r => r.name) : [];
  const sel = $('paRemote'), key = JSON.stringify(names);
  if (sel.dataset.key !== key) {
    sel.dataset.key = key;
    const keep = sel.value;
    sel.replaceChildren(...names.map(n => new Option(n, n)));
    if (names.includes(keep)) sel.value = keep;
  }
  $('paOpen').disabled = !names.length;
  $('paOpen').title = names.length ? 'Ouvre l\'alignement de cette zone pour la télécommande choisie' : 'Appairez d\'abord un téléphone (RÉGLAGES › Télécommande)';
}
function paDraw(a) {
  const cv = $('paCanvas'), ctx = cv.getContext('2d');
  ctx.clearRect(0, 0, cv.width, cv.height);
  const css = getComputedStyle(document.documentElement), color = n => css.getPropertyValue(n).trim();
  ctx.strokeStyle = color('--line'); ctx.lineWidth = 1;
  const [p, q] = [pzToPx(cv, [-1, 1]), pzToPx(cv, [1, -1])];
  ctx.strokeRect(p[0], p[1], q[0] - p[0], q[1] - p[1]);
  const quad = (c, dash, col) => {
    ctx.strokeStyle = col; ctx.lineWidth = 2; ctx.setLineDash(dash);
    ctx.beginPath();
    c.forEach((pt, i) => { const [x, y] = pzToPx(cv, pt); if (i) ctx.lineTo(x, y); else ctx.moveTo(x, y); });
    ctx.closePath(); ctx.stroke(); ctx.setLineDash([]);
  };
  quad(a.base, [], color('--accent'));
  quad(a.draft, [5, 4], color('--warn'));
}
function showPhoneAlign(a) {
  phoneAlign = a;
  paRemotes();
  $('paActive').hidden = !a;
  $('paStart').hidden = !!a;
  if (!a) return;
  $('paTitle').textContent = `Alignement en cours — zone « ${a.zone_name} » — télécommande « ${a.remote} » : ${a.changed ? 'modification proposée par la télécommande « ' + a.remote + ' »' : 'aucun déplacement pour l\'instant'}`;
  $('paCorners').textContent = a.corners.map((n, i) => `${n} ${a.draft[i].map(v => v.toFixed(3)).join(' ; ')}`).join('   ');
  $('paApply').disabled = !a.changed;
  // No test pattern on: the laser no longer draws the draft (T-492).
  const note = a.pattern_note || '';
  $('paPattern').hidden = !note;
  if ($('paPattern').textContent !== note) $('paPattern').textContent = note;
  paDraw(a);
}
// Why the last session closed by itself (its phone went quiet), until
// another one opens (T-492).
function showPhoneAlignNote(n) {
  const t = n || '';
  if ($('paNote').textContent !== t) $('paNote').textContent = t;
}
async function paPost(path, body) {
  try {
    const r = await api('POST', path, body, { quiet: true });
    const t = r.ok ? null : await r.text();
    if (r.status === 423) flash(t); else $('paError').textContent = t || '';
    if (r.ok) {
      const j = await r.json();
      showPhoneAlign(j.align);
      if (j.projection) takeProjection(j.projection);
    }
  } catch {}
}
$('paOpen').onclick = () => { const z = pzZone(); if (z && $('paRemote').value) paPost('/api/zones/align/open', { zone: z.id, remote: $('paRemote').value }); };
$('paApply').onclick = () => paPost('/api/zones/align/apply');
$('paReject').onclick = () => paPost('/api/zones/align/reject');
$('paClose').onclick = () => paPost('/api/zones/align/close');
setInterval(paRemotes, 1000);
// Apprendre DMX (right-click menu): the first channel that moves is mapped.
let dmxLearnTimer = null;
async function dmxLearnWatch() {
  clearTimeout(dmxLearnTimer);
  try {
    const n = await apiJson('GET', '/api/netin');
    if (n.dmx.learn) { dmxLearnTimer = setTimeout(dmxLearnWatch, 400); return; }
    if (n.dmx.learned) showBanner(`Appris : univers ${n.dmx.learned.universe}, canal ${n.dmx.learned.channel} → ${labelOf(n.dmx.learned.control)}`, 3000);
  } catch {}
}
$('midiMenuDmx').onclick = async () => {
  const id = menuControl; closeMidiMenu();
  const r = await api('POST', '/api/dmx/learn', { control: id }, { quiet: true });
  if (!r.ok) return flash(await r.text());
  showBanner(`Apprendre DMX : bougez un canal de la console pour ${labelOf(id)}`, 4000);
  dmxLearnWatch();
};
$('netinPanel').addEventListener('toggle', () => { if ($('netinPanel').open) loadNetin(); });
setInterval(() => { if (activeWs === 'settings' && $('netinPanel').open && !document.hidden) loadNetin(); }, 500);

// RÉGLAGES › Retour OSC (T-416): the feedback settings, the subscribers,
// the monitor (last 200 messages in and out) and the address list, all
// generated by the server from the control registry. Polled while open.
const OSC_KIND_FR = { continuous: 'continu', toggle: 'bascule', momentary: 'momentané', trigger: 'déclencheur', choice: 'choix' };
const OSC_RESULT_CLS = { 'appliqué': 'r-ok', 'envoyé': 'r-ok', 'refusé': 'r-refused', 'abandonné': 'r-refused', 'inconnu': 'r-unknown' };
let oscAddrList = [];
function oscCell(text, cls) { const td = document.createElement('td'); td.textContent = text; if (cls) td.className = cls; return td; }
function showOscFb(v) {
  const c = v.config;
  $('oscFbEnabled').checked = c.enabled;
  $('oscFbSubscribe').checked = c.allow_subscribe;
  $('oscFbBeat').checked = c.beat_messages;
  setField('oscFbTargets', c.targets.join(', '));
  setField('oscFbRate', c.max_rate_hz);
  let st = 'Coupé', cls = '';
  if (c.enabled) {
    const from = v.status.local ? `depuis ${v.status.local}` : `depuis l'interface ${v.osc.bind}`;
    st = `Actif ${from} — ${v.status.sent} message(s) envoyé(s)` + (v.dropped ? `, ${v.dropped} abandonné(s) (file pleine)` : '');
    cls = 'ok';
    if (v.status.error) { st += ` — ${v.status.error}`; cls = 'warn'; }
    if (!v.osc.enabled) { st += ' — l\'entrée OSC est coupée : pas d\'abonnements'; cls = 'warn'; }
  }
  $('oscFbState').textContent = st; $('oscFbState').className = 'ni-state ' + cls;
  const subs = v.subscribers;
  if (!subs.length) $('oscFbSubs').replaceChildren(Object.assign(document.createElement('li'), { textContent: 'Aucun abonné.' }));
  else $('oscFbSubs').replaceChildren(...subs.map(sb => Object.assign(document.createElement('li'), {
    textContent: sb.addr + (sb.fixed ? ' (destination fixe)' : ` (dernier message il y a ${Math.round(sb.age_ms / 1000)} s)`),
  })));
}
async function loadOscFb() {
  try { const r = await api('GET', '/api/osc/config', undefined, { quiet: true }); if (r.ok) showOscFb(await r.json()); } catch {}
}
async function saveOscFb(patch) {
  const r = await api('POST', '/api/osc/config', patch, { quiet: true });
  $('oscFbError').textContent = r.ok ? '' : await r.text();
  if (r.ok) showOscFb(await r.json()); else loadOscFb();
}
async function loadOscMonitor() {
  const dir = $('oscMonDir').value;
  let j;
  try { const r = await api('GET', '/api/osc/monitor' + (dir ? '?dir=' + dir : ''), undefined, { quiet: true }); if (!r.ok) return; j = await r.json(); } catch { return; }
  const body = $('oscMonTable').tBodies[0];
  if (!j.entries.length) { const tr = document.createElement('tr'); const td = oscCell('Aucun message.'); td.colSpan = 6; tr.append(td); body.replaceChildren(tr); return; }
  body.replaceChildren(...j.entries.map(e => {
    const tr = document.createElement('tr');
    tr.className = e.incoming ? 'osc-in' : 'osc-out';
    tr.append(oscCell(new Date(e.at_ms).toLocaleTimeString('fr-FR')), oscCell(e.incoming ? 'entrant' : 'sortant'), oscCell(e.peer),
      oscCell(e.address), oscCell(e.args), oscCell(e.result + (e.detail ? ' : ' + e.detail : ''), OSC_RESULT_CLS[e.result]));
    return tr;
  }));
}
async function loadOscAddresses() {
  try { const r = await api('GET', '/api/osc/addresses', undefined, { quiet: true }); if (r.ok) oscAddrList = await r.json(); } catch {}
  return oscAddrList;
}
function renderOscAddresses() {
  const q = $('oscAddrSearch').value.trim().toLowerCase();
  const list = oscAddrList.filter(a => !q || a.address.toLowerCase().includes(q) || a.control.includes(q) || a.label_fr.toLowerCase().includes(q));
  $('oscAddrTable').tBodies[0].replaceChildren(...list.map(a => {
    const tr = document.createElement('tr');
    const range = a.min == null ? '—' : `${a.min} … ${a.max}` + (a.unit && a.unit !== 'none' ? ` (${a.unit})` : '');
    const td = document.createElement('td');
    const b = Object.assign(document.createElement('button'), { textContent: 'Copier', title: 'Copier ' + a.address });
    b.setAttribute('aria-label', 'Copier ' + a.address);
    b.onclick = () => copyOscAddress(a.address);
    td.append(b);
    tr.append(oscCell(a.address), oscCell(`${a.label_fr} · ${a.control}`), oscCell(OSC_KIND_FR[a.kind] || a.kind), oscCell(range), td);
    return tr;
  }));
}
async function copyOscAddress(address) {
  try { await navigator.clipboard.writeText(address); flash('Adresse copiée : ' + address); }
  catch { flash('Adresse OSC : ' + address + ' (copie impossible, sélectionnez-la)'); }
}
$('oscFbEnabled').onchange = e => saveOscFb({ enabled: e.target.checked });
$('oscFbSubscribe').onchange = e => saveOscFb({ allow_subscribe: e.target.checked });
$('oscFbBeat').onchange = e => saveOscFb({ beat_messages: e.target.checked });
$('oscFbTargets').onchange = e => saveOscFb({ targets: e.target.value.split(/[\s,;]+/).filter(Boolean) });
$('oscFbRate').onchange = e => saveOscFb({ max_rate_hz: +e.target.value });
$('oscMonDir').onchange = loadOscMonitor;
$('oscMonClear').onclick = async () => { await api('POST', '/api/osc/monitor/clear', {}); loadOscMonitor(); };
$('oscAddrBtn').onclick = async () => {
  const open = $('oscAddrBox').hidden;
  $('oscAddrBox').hidden = !open;
  $('oscAddrBtn').setAttribute('aria-expanded', String(open));
  if (open) { await loadOscAddresses(); renderOscAddresses(); }
};
$('oscAddrSearch').oninput = renderOscAddresses;
$('oscFbPanel').addEventListener('toggle', () => { if ($('oscFbPanel').open) { loadOscFb(); loadOscMonitor(); } });
setInterval(() => { if (activeWs === 'settings' && $('oscFbPanel').open && !document.hidden) { loadOscFb(); loadOscMonitor(); } }, 500);
// « Copier l'adresse OSC » (menu of a control): exactly the listed address.
$('midiMenuOsc').onclick = async () => {
  const id = menuControl.replace(/^live\./, 'master.');
  closeMidiMenu();
  const a = (await loadOscAddresses()).find(x => x.control === id);
  if (!a) return flash('Pas d\'adresse OSC : ce contrôle ne se pilote pas depuis le réseau');
  copyOscAddress(a.address);
};
$('midiBlackoutLost').onchange = e => post('/api/midi/safety', { blackout_on_disconnect: e.target.checked });

export { closeVapcMenu, ext, setGate, showPhoneAlign, showPhoneAlignNote, showStandbyBar };
