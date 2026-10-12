// Laser Studio — direct.js (T-435).
// Sections (as in the former index.html script):
//   live (master)
//   Direct: Maître / Calque / Cue (T-144)
//   Direct › Cue › Cible : une partie de figure (T-441)
//   live effects (T-142)
//   live colour
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './targets.js';
import { $, api, apiJson, controlLatest, hex, post, rgb, setCheck, setVal } from './core.js';
import { syncFill } from './laser.js';
import { LONG_PRESS_MS, flash } from './workspaces.js';
import { loadSlots, presetById, slots } from './cues.js';
import { assignControlsLoaded, controlsLoaded, loadControls } from './midi.js';

// ---------- live (master) ----------
// Like the laser button: a frame fetched before our last live change was
// answered carries the old values and must not be drawn over the controls
// (it would e.g. untick « Synchro tempo » right after it was ticked).
let liveSentAt = 0, liveAnsweredAt = 0;
// `latest`: a slider or pad value, latest wins per control (T-484); the
// hold buttons (Flash, Figer, Noir) post directly.
async function postLive(body, latest = false) {
  if (directScope === 'cue' && directPart) return postPart(body);
  const id = scopedId(body.id);
  if (!id) return;
  liveSentAt = performance.now();
  if (latest) await controlLatest({ ...body, id }); else await post('/api/control', { ...body, id });
  liveAnsweredAt = performance.now();
}

// ---------- Direct: Maître / Calque / Cue (T-144) ----------
// The same controls drive the master, one layer (`layer.<n>.<param>`) or
// one cue on show (`cue.<id>.<param>`). « Vitesse de balayage », « Figer »
// and « Noir » stay the master's.
let directScope = 'master', directLayer = 1, directCue = null, directFrame = null, directCueKey = '';
function scopedId(id) {
  if (!id || !id.startsWith('master.') || id === 'master.scan' || directScope === 'master') return id;
  if (directScope === 'layer') return `layer.${directLayer}.${id.slice(7)}`;
  if (directPart) return null; // a part: only `postPart`'s parameters (T-441)
  return directCue ? `cue.${directCue}.${id.slice(7)}` : null;
}
// What the Direct panel shows: the master, the chosen layer's or cue's.
function scopeMods(f) {
  if (directScope === 'layer') return f.layers && f.layers.mixer.layers[directLayer - 1].modifiers;
  if (directScope === 'cue' && directPart) return partShown(f);
  if (directScope === 'cue') return directCue && f.cue_mods ? f.cue_mods[directCue] : null;
  return f.live;
}

// ---------- Direct › Cue › Cible : une partie de figure (T-441) ----------
// A figure cue with parts: « Cible » picks the whole figure (the cue level
// above) or one part. A part's Taille, Position, Rotation and Luminosité
// are stored in the cue's cell (`/api/cues/parts`, an edit: undo, refused
// in show mode) and applied to that part only, before the cue's own
// modifiers and the whole chain.
let directPart = 0, partEdit = null, partTimer = null;
const PART_KEYS = {
  'master.brightness': 'brightness', 'master.size': 'size', 'master.size_x': 'size_x', 'master.size_y': 'size_y',
  'master.pos_x': 'pos_x', 'master.pos_y': 'pos_y', 'master.rot_x.angle': 'rot_angle.0', 'master.rot_y.angle': 'rot_angle.1',
  'master.rot_z.angle': 'rot_angle.2', 'master.rot_x.speed': 'rot_speed.0', 'master.rot_y.speed': 'rot_speed.1', 'master.rot_z.speed': 'rot_speed.2',
};
const PART_NEUTRAL = { brightness: 1, size: 1, size_x: 1, size_y: 1, pos_x: 0, pos_y: 0, rot_angle: [0, 0, 0], rot_speed: [0, 0, 0] };
const directParts = () => (directCue && (presetById.get(directCue) || {}).parts) || [];
// The part's own values (only what a part sets), being edited or stored.
function partValues() {
  if (partEdit) return partEdit;
  const stored = ((slots[directCue] || {}).part_modifiers || {})[directPart] || {};
  const v = structuredClone(PART_NEUTRAL);
  for (const k of Object.keys(PART_NEUTRAL)) if (stored[k] !== undefined) v[k] = structuredClone(stored[k]);
  return v;
}
// What the Direct panel shows for a part: its values over a neutral copy.
function partShown(f) {
  const m = structuredClone(f.live);
  Object.assign(m, { rot_sync: false, rot_reverse: false, speed: 1, trace: 1, dots: 0, prism: 1 }, structuredClone(partValues()));
  return m;
}
function partHint(text, ok) {
  const hint = $('directScopeHint');
  hint.dataset.msg = '1'; hint.className = 'hint ' + (ok ? 'ok' : 'bad'); hint.textContent = text;
  clearTimeout(partHint.t);
  partHint.t = setTimeout(() => { delete hint.dataset.msg; refreshDirect(); }, 4000);
}
function postPart(body) {
  const key = PART_KEYS[body.id];
  if (!key || !directCue) { partHint('Sur une partie : Taille, Position, Rotation et Luminosité seulement.', false); return Promise.resolve(); }
  const v = partEdit || (partEdit = partValues());
  const value = Number(body.value);
  if (!Number.isFinite(value)) return Promise.resolve();
  const [k, i] = key.split('.');
  if (i === undefined) v[k] = value; else v[k][+i] = value;
  clearTimeout(partTimer);
  partTimer = setTimeout(sendPart, 150);
  return Promise.resolve();
}
async function sendPart() {
  if (!partEdit || !directCue || !directPart) { partEdit = null; return; }
  const [cue, part, modifiers] = [directCue, directPart, partEdit];
  const r = await api('POST', '/api/cues/parts', { id: cue, part, modifiers }, { quiet: true });
  if (partEdit === modifiers) partEdit = null;
  if (r.ok) {
    slots[cue] = { ...(slots[cue] || {}), part_modifiers: await r.json() };
  } else {
    partHint((await r.text()).trim() || 'Réglage de la partie refusé.', false);
  }
  refreshDirect();
}
function showDirectParts() {
  const parts = directParts(), row = $('directPartRow'), sel = $('directPart');
  row.hidden = directScope !== 'cue' || !parts.length;
  if (!parts.some(p => p.id === directPart)) directPart = 0;
  const key = directCue + '|' + parts.map(p => p.id + p.name).join('|');
  if (sel.dataset.key !== key) {
    sel.dataset.key = key;
    sel.replaceChildren();
    for (const [v, t] of [[0, 'Figure entière'], ...parts.map(p => [p.id, p.name])]) {
      const o = document.createElement('option'); o.value = String(v); o.textContent = t; sel.appendChild(o);
    }
  }
  if (document.activeElement !== sel) sel.value = String(directPart);
}
$('directPart').addEventListener('change', () => {
  clearTimeout(partTimer); if (partEdit) sendPart();
  directPart = +$('directPart').value || 0;
  refreshDirect();
});
// The cues on show, newest last; the newest is chosen when ours stopped.
function showDirectCues(f) {
  const ids = [...new Set([...(f.cues ? f.cues.active : []), ...(f.cues ? f.cues.ending : [])].map(c => c.cue))];
  const key = ids.join('\n');
  if (key !== directCueKey) {
    directCueKey = key;
    const name = id => (presetById.get(id) || { name: id }).name;
    $('directCue').innerHTML = ids.length ? ids.map(id => `<option value="${id.replace(/"/g, '&quot;')}"></option>`).join('') : '<option value="">Aucun cue en cours</option>';
    [...$('directCue').options].forEach((o, i) => { if (ids.length) o.textContent = name(ids[i]); });
  }
  if (!ids.includes(directCue)) directCue = ids.includes(f.active_cue) ? f.active_cue : (ids[ids.length - 1] || null);
  if (directCue && $('directCue').value !== directCue) $('directCue').value = directCue;
  $('directCue').disabled = !ids.length;
  $('directStore').disabled = !directCue;
  showDirectParts();
}
function showDirectScope(f) {
  if (!f) return;
  directFrame = f;
  showDirectCues(f);
  const off = directScope === 'cue' && !directCue;
  $('livePanel').classList.toggle('scope-off', off);
  const hint = $('directScopeHint');
  if (!hint.dataset.msg) {
    hint.className = 'hint';
    hint.textContent = directScope === 'layer' ? `Réglages du calque ${directLayer}, avant le maître.`
      : directScope === 'cue' ? (off ? 'Aucun cue en cours : lancez un cue pour le régler.'
        : directPart ? `Réglages de la partie « ${(directParts().find(p => p.id === directPart) || {}).name || directPart} » seule (Taille, Position, Rotation, Luminosité), enregistrés dans la case du cue.`
        : 'Réglages de ce cue seul, avant son calque et le maître. Ils reviennent à ceux de sa case à chaque lancement.')
      : '';
  }
  const on = f.levels_on || {}, layerOn = (on.layers || []).some(Boolean), cueOn = !!on.cues;
  document.querySelectorAll('#directTabs button').forEach(b => b.classList.toggle('lvl-on', (b.dataset.scope === 'layer' && layerOn) || (b.dataset.scope === 'cue' && cueOn)));
  const sm = f.layers && f.layers.mixer.smoothing;
  if (sm) {
    setCheck('smoothOn', sm.enabled);
    setVal('smoothInertia', Math.round(sm.mass * 100));
    $('smoothInertiaV').textContent = `× ${sm.mass.toFixed(2)}`;
  }
}
function setDirectScope(scope) {
  directScope = scope;
  document.querySelectorAll('#directTabs button').forEach(b => {
    b.classList.toggle('active', b.dataset.scope === scope);
    b.setAttribute('aria-selected', b.dataset.scope === scope ? 'true' : 'false');
  });
  $('directLayerRow').hidden = scope !== 'layer';
  $('directCueRow').hidden = scope !== 'cue';
  $('directPartRow').hidden = scope !== 'cue' || !directParts().length;
  $('mScan').disabled = scope !== 'master';
  delete $('directScopeHint').dataset.msg;
  refreshDirect();
}
function refreshDirect() {
  if (!directFrame) return;
  showDirectScope(directFrame);
  const m = scopeMods(directFrame);
  if (m) showLive(m);
}
document.querySelectorAll('#directTabs button').forEach(b => b.addEventListener('click', () => setDirectScope(b.dataset.scope)));
$('directLayer').addEventListener('change', () => { directLayer = +$('directLayer').value; refreshDirect(); });
$('directCue').addEventListener('change', () => { directCue = $('directCue').value || null; directPart = 0; partEdit = null; refreshDirect(); });
$('directStore').addEventListener('click', async () => {
  if (!directCue) return;
  const hint = $('directScopeHint'), name = (presetById.get(directCue) || { name: directCue }).name;
  const r = await api('POST', '/api/cues/modifiers', { id: directCue, with_master: $('directStoreMaster').checked }, { quiet: true });
  hint.dataset.msg = '1';
  hint.className = 'hint ' + (r.ok ? 'ok' : 'bad');
  hint.textContent = r.ok ? `Enregistré dans le cue « ${name} » : il les reprendra à chaque lancement.` : ((await r.text()).trim() || 'Enregistrement refusé.');
  setTimeout(() => { delete hint.dataset.msg; refreshDirect(); }, 4000);
  if (r.ok) loadSlots();
});
$('smoothOn').addEventListener('change', () => post('/api/control', { id: 'smoothing.on', value: $('smoothOn').checked ? 1 : 0 }));
$('smoothInertia').addEventListener('input', () => {
  $('smoothInertiaV').textContent = `× ${($('smoothInertia').value / 100).toFixed(2)}`;
  controlLatest({ id: 'smoothing.inertia', value: $('smoothInertia').value / 100 });
});
let live = null;
const liveGet = (path) => path.split('.').reduce((o, k) => o[k], live);
function liveSet(path, v) {
  const keys = path.split('.'), last = keys.pop();
  keys.reduce((o, k) => o[k], live)[last] = v;
}
function showLive(l) {
  if (!l) return;
  live = l;
  document.querySelectorAll('[data-live]').forEach(el => {
    const v = liveGet(el.dataset.live), k = +el.dataset.k;
    if (el.dataset.spin !== undefined) {
      // Rotation speeds: degrees per second, or turns per bar with Sync tempo.
      const [lim, step] = l.rot_sync ? [4, 0.05] : [720, 1];
      if (+el.max !== lim) { el.min = -lim; el.max = lim; el.step = step; }
      if (document.activeElement !== el) el.value = v;
      $(el.id + 'V').textContent = l.rot_sync ? `${+v.toFixed(2)} tr/mesure` : `${Math.round(v)} °/s`;
      return;
    }
    if (document.activeElement !== el) el.value = Math.round(v * k);
    const unit = k === 1 ? '°' : '%';
    $(el.id + 'V').textContent = Math.round(v * k) + ' ' + unit;
  });
  document.querySelectorAll('#speedPresets button').forEach(b => b.classList.toggle('active', Math.abs(+b.dataset.speed - l.speed) < 1e-3));
  $('posDot').style.left = (50 + l.pos_x * 50) + '%';
  $('posDot').style.top = (50 - l.pos_y * 50) + '%';
  $('rotSync').checked = l.rot_sync;
  const table = l.rot_sync ? [0, 0.25, 1, 2] : [0, 30, 90, 270];
  document.querySelectorAll('#rotPresets button').forEach(b => b.classList.toggle('active', table[+b.dataset.preset] === l.rot_speed[2]));
  showColor(l);
  if (l.strobe) showFx(l);
}
// Each slider drives its control id (in its title) directly, so a slider
// move can't be overwritten by a slightly older state from the server.
document.querySelectorAll('[data-live]').forEach(el => el.addEventListener('input', () => {
  const v = el.value / +el.dataset.k;
  if (live) { liveSet(el.dataset.live, v); showLive(live); }
  postLive({ id: el.title, value: v }, true);
}));
document.querySelectorAll('#rotPresets button').forEach(b => b.addEventListener('click', () =>
  postLive({ id: 'master.rot.preset', value: +b.dataset.preset })));
// One live value, shown at once and sent to its control.
function setLive(path, id, v) {
  if (live) { liveSet(path, v); showLive(live); }
  return postLive({ id, value: v }, true);
}
document.querySelectorAll('#speedPresets button').forEach(b => b.addEventListener('click', () => setLive('speed', 'master.speed', +b.dataset.speed)));
// Zoom −/+ : 10 % steps of the master size, within its 0..200 %.
const zoom = d => setLive('size', 'master.size', Math.min(2, Math.max(0, Math.round(((live ? live.size : 1) + d) * 10) / 10)));
$('zoomOut').addEventListener('click', () => zoom(-0.1));
$('zoomIn').addEventListener('click', () => zoom(0.1));
// Sliders (T-431, after T-143): one behaviour for every input[type=range]
// of the page, static or built later (layers, modulators, audio links,
// safety, calibration), applied by `enhanceSlider`:
// - double-click on the slider: type a value (in the unit shown: « 42,5 »,
//   « 80 % », « -45° »), clamped to the bounds; Entrée or a click
//   elsewhere validates, Tab validates and moves on, Échap cancels (and is
//   still the e-stop);
// - Maj (or Alt) + drag: fine, a tenth of the movement;
// - a press alone sends nothing: a drag sends once it moved 4 px, a click
//   only when no second click came (so a double-click never jumps);
// - with the keyboard focus: arrows = a step (Maj: a tenth of one), Page
//   préc./suiv. = ten steps, Début/Fin = the bounds; the wheel only then,
//   so a page scrolled over a slider never moves it;
// - right click or double-click on its name: the default value (a right
//   click on the slider itself stays the MIDI menu, which has it too).
// Bounds and default: the registry's (GET /api/controls) for a
// [data-control] slider, else min / max and `data-default`. A value always
// takes the slider's usual road: a master's control (/api/control, like
// the MIDI menu's « Valeur neutre ») or the slider's own input / change
// handlers (safety still asks before loosening, #sfLoosen).
const controlDefaults = {}, controlKinds = {};
const SLIDER_HINT = 'Double-clic : taper une valeur · Maj + glisser : réglage fin · clic droit sur le libellé : valeur par défaut';
const UNIT_FR = { percent: ' %', deg: '°', deg_per_sec: ' °/s', bpm: ' BPM', hz: ' Hz', ms: ' ms', none: '' };
const sliderLabel = el => {
  const prev = n => n?.previousElementSibling?.tagName === 'LABEL' ? n.previousElementSibling : null;
  return el.closest('label') || (el.id && document.querySelector(`label[for="${CSS.escape(el.id)}"]`))
    || prev(el) || (el.parentElement?.classList.contains('row') ? prev(el.parentElement) : null);
};
// Where the slider shows its value: #<id>V, the name's « .v », or the
// value right after it (layer strips).
function sliderDisplay(el) {
  const byId = el.id ? $(el.id + 'V') : null;
  if (byId) return byId;
  const lab = sliderLabel(el);
  const inLab = lab && !lab.contains(el) ? lab.querySelector('.v') : null;
  if (inLab) return inLab;
  const next = el.nextElementSibling;
  return next && next.matches('span.v, span.dv, span[id$="V"]') ? next : null;
}
// Typed text → number: comma or point, a sign, an optional unit after it.
function parseTyped(text) {
  const m = String(text).replace(/\s+/g, '').replace('\u2212', '-').replace(',', '.').match(/^[-+]?(\d+\.?\d*|\.\d+)/);
  return m ? parseFloat(m[0]) : NaN;
}
const clampTyped = (v, min, max) => Math.min(max, Math.max(min, v));
// Slider units per native unit, for a [data-control] slider: `data-k`,
// else its top over the registry's (0..200 for 0..2 = 100).
function sliderK(el) {
  if (el.dataset.k) return +el.dataset.k;
  const k = controlKinds[el.dataset.control];
  if (!k) return 1;
  const r = k.max ? +el.max / k.max : k.min ? +el.min / k.min : 1;
  return Math.round(r * 1000) / 1000 || 1;
}
// A master slider (LIVE › Direct, top bar): set through its control, exact
// (not rounded to its step), like the MIDI menu's « Valeur neutre ».
const exactSlider = el => !!el.dataset.live && !!el.dataset.control;
// Shown number per slider unit (`data-display-k`, e.g. 0.01 when a 0..400
// slider shows 0,00…4,00).
const displayK = el => +(el.dataset.displayK || 1);
function sliderUnit(el) {
  const k = controlKinds[el.dataset.control];
  if (el.dataset.unit !== undefined) return el.dataset.unit;
  if (k && UNIT_FR[k.unit] !== undefined && displayK(el) === 1) return UNIT_FR[k.unit];
  const m = (sliderDisplay(el)?.textContent || '').match(/\d\s*([^\d\s][^]*)$/);
  return m ? (m[1].startsWith('%') ? ' ' : '') + m[1].trim() : '';
}
const fmtTyped = v => (+v.toFixed(3)).toLocaleString('fr-FR', { maximumFractionDigits: 3 });
// The slider's value as `v` (slider units, already within bounds): a
// master slider's control (exact), else the slider's own input / change
// handlers, as a drag would. `done` = the end of a gesture (change).
function setSlider(el, v, done = true) {
  if (exactSlider(el)) {
    el.value = v;
    el.dataset.exact = v; el.dataset.exactAt = el.value;
    syncFill(el);
    setLive(el.dataset.live, el.dataset.control, v / sliderK(el));
    return;
  }
  el.value = v;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  if (done) el.dispatchEvent(new Event('change', { bubbles: true }));
}
function sliderDefault(el) {
  const id = el.dataset.control;
  if (id && typeof controlDefaults[id] === 'number') return controlDefaults[id] * sliderK(el);
  return el.dataset.default !== undefined ? +el.dataset.default : null;
}
async function resetSlider(el) {
  if (el.dataset.control && !(el.dataset.control in controlKinds)) {
    await controlsLoaded;
    if (!(el.dataset.control in controlKinds)) await (assignControlsLoaded(loadControls()));
  }
  const d = sliderDefault(el);
  if (d == null || el.disabled) return;
  setSlider(el, clampTyped(d, +el.min, +el.max));
}
// The registry's default for a control (MIDI menu « Valeur neutre »).
function toNeutral(id) {
  const v = controlDefaults[id];
  if (typeof v !== 'number') return;
  const el = document.querySelector(`input[type=range][data-control="${id}"]`);
  return el ? resetSlider(el) : postLive({ id, value: v });
}
// The slider's value: the exact one a control was set to (finer than its
// step), while its position is still the one that gave; else its position.
const sliderValue = el => el.dataset.exactAt === el.value && el.dataset.exact !== undefined ? +el.dataset.exact : +el.value;
function stepOf(el) { return el.step === 'any' ? (+el.max - +el.min) / 100 : +el.step || 1; }
// One move of `n` steps; `fine` = a tenth of a step when the slider can
// hold it (a master slider: always), else one step.
function nudgeSlider(el, n, fine = false) {
  const step = stepOf(el), cur = sliderValue(el);
  let v = clampTyped(cur + n * (fine ? step / 10 : step), +el.min, +el.max);
  if (!exactSlider(el) && fine) {
    // The step would round a tenth of it away: one step then.
    const probe = document.createElement('input');
    probe.type = 'range'; probe.min = el.min; probe.max = el.max; probe.step = el.step || '1'; probe.value = v;
    if (+probe.value === +el.value) v = clampTyped(cur + n * step, +el.min, +el.max);
  }
  setSlider(el, v);
}
// A press becomes a drag past this many pixels; a click waits this long
// for a second click before it sends its value.
const SLIDER_DRAG_PX = 4, SLIDER_DBL_MS = 450;
// `v` rounded to the slider's step.
function snapSlider(el, v) {
  if (el.step === 'any') return v;
  const step = +el.step || 1, min = +el.min || 0;
  return clampTyped(+(min + Math.round((v - min) / step) * step).toFixed(6), +el.min, +el.max);
}
let sliderEntry = null;
function openSliderEntry(el) {
  if (el.disabled) return;
  closeSliderEntry(true);
  const disp = sliderDisplay(el);
  const box = (disp && disp.offsetParent ? disp : el).getBoundingClientRect();
  const input = document.createElement('input');
  input.type = 'text';
  input.className = 'slider-entry';
  input.setAttribute('inputmode', 'decimal');
  input.setAttribute('aria-label', 'Valeur');
  input.value = fmtTyped(+el.value * displayK(el));
  const w = Math.max(64, disp && disp.offsetParent ? box.width + 12 : 72);
  input.style.width = w + 'px';
  input.style.left = Math.max(4, box.right - w) + 'px';
  input.style.top = (box.top + box.height / 2 - 13) + 'px';
  el.after(input);
  if (disp) disp.style.visibility = 'hidden';
  sliderEntry = { el, input, disp };
  input.addEventListener('keydown', e => {
    if (e.key === 'Enter' || e.key === 'NumpadEnter') {
      // Validate here, never a tap tempo.
      e.preventDefault(); e.stopPropagation();
      closeSliderEntry(true);
      el.focus();
    } else if (e.key === 'Escape') {
      // Cancels; the e-stop still runs (the page's handler, after this).
      closeSliderEntry(false);
    }
    // Tab: the focus moves on (the field sits right after the slider) and
    // the blur validates.
  });
  input.addEventListener('blur', () => closeSliderEntry(true));
  // Its typing is not the panel's (delegated input / change handlers).
  ['input', 'change'].forEach(t => input.addEventListener(t, e => e.stopPropagation()));
  input.focus();
  input.select();
}
function closeSliderEntry(commit) {
  const s = sliderEntry;
  if (!s) return;
  sliderEntry = null;
  const typed = parseTyped(s.input.value);
  if (s.disp) s.disp.style.visibility = '';
  s.input.remove();
  if (!commit || !Number.isFinite(typed)) return;
  const dk = displayK(s.el), min = +s.el.min, max = +s.el.max;
  const want = typed / dk, v = clampTyped(want, min, max);
  if (v !== want) flash(`Limité à ${fmtTyped((v === max ? max : min) * dk)}${sliderUnit(s.el)}`);
  setSlider(s.el, v);
}
function enhanceSlider(el) {
  if (el.dataset.enh !== undefined) return;
  el.dataset.enh = '';
  // The pointer is ours, not the browser's: a press never sends a value
  // by itself (laser safety: the first click of a double-click must not
  // flash the output or make it jump). A drag sends once it has moved a
  // few pixels; a plain click sends its value only after the double-click
  // window has passed without a second click. Maj or Alt: fine drag.
  let pendingClick = 0;
  el.addEventListener('pointerdown', e => {
    if (e.button !== 0 || el.disabled) return;
    e.preventDefault();
    el.focus();
    el.setPointerCapture(e.pointerId);
    clearTimeout(pendingClick);
    const fine = e.shiftKey || e.altKey, r = el.getBoundingClientRect();
    const vertical = getComputedStyle(el).writingMode.startsWith('vertical');
    const pos = ev => vertical ? -ev.clientY : ev.clientX;
    const p0 = pos(e), v0 = sliderValue(el), min = +el.min, max = +el.max, span = max - min;
    const len = (vertical ? r.height : r.width) || 1, thumb = Math.min(16, len / 4);
    // Where the pointer is along the track, as the browser would place the thumb.
    const at = ev => {
      const t = vertical ? (r.bottom - ev.clientY - thumb / 2) / (len - thumb) : (ev.clientX - r.left - thumb / 2) / (len - thumb);
      return snapSlider(el, min + clampTyped(t, 0, 1) * span);
    };
    let dragging = false, held = false;
    // Long press with a finger or a pen (T-282): back to the neutral value,
    // like the right click / double-click on the slider's name.
    const hold = e.pointerType === 'mouse' ? 0 : setTimeout(() => { held = true; resetSlider(el); }, LONG_PRESS_MS);
    const move = ev => {
      if (held) return;
      if (!dragging) { if (Math.abs(pos(ev) - p0) < SLIDER_DRAG_PX) return; dragging = true; clearTimeout(hold); }
      setSlider(el, fine ? clampTyped(v0 + (pos(ev) - p0) / len * span / 10, min, max) : at(ev), false);
    };
    const up = ev => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
      clearTimeout(hold);
      if (held) return;
      if (dragging) { el.dispatchEvent(new Event('change', { bubbles: true })); return; }
      if (ev.type !== 'pointerup' || fine) return;
      const v = at(e);
      pendingClick = setTimeout(() => { if (v !== sliderValue(el)) setSlider(el, v); }, SLIDER_DBL_MS);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  });
  el.addEventListener('dblclick', e => {
    e.preventDefault();
    clearTimeout(pendingClick);
    openSliderEntry(el);
  });
  // A plain drag forgets the exact value (a value from the studio moves the
  // position, which does too).
  el.addEventListener('input', e => { if (e.isTrusted) delete el.dataset.exact; });
  el.addEventListener('keydown', e => {
    const fine = e.shiftKey;
    const moves = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1, PageUp: 10, PageDown: -10 };
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (moves[e.key] !== undefined) nudgeSlider(el, moves[e.key], fine && Math.abs(moves[e.key]) === 1);
    else if (e.key === 'Home' || e.key === 'End') setSlider(el, e.key === 'Home' ? +el.min : +el.max);
    else return;
    // Ours, not the page's (cue list Page suiv. / Début).
    e.preventDefault(); e.stopPropagation();
  });
  el.addEventListener('wheel', e => {
    if (document.activeElement !== el || el.disabled) return;
    e.preventDefault();
    nudgeSlider(el, e.deltaY < 0 || e.deltaX > 0 ? 1 : -1, e.shiftKey);
  }, { passive: false });
  const lab = sliderLabel(el);
  if (lab && lab.dataset.neutral === undefined) {
    lab.dataset.neutral = '';
    lab.title = (lab.title ? lab.title + '\n' : '') + SLIDER_HINT;
    const onName = e => !el.contains(e.target) && e.target !== el && !(sliderEntry && e.target === sliderEntry.input);
    lab.addEventListener('contextmenu', e => { if (!onName(e)) return; e.preventDefault(); e.stopPropagation(); resetSlider(el); });
    lab.addEventListener('dblclick', e => { if (onName(e)) resetSlider(el); });
  }
}
document.querySelectorAll('input[type=range]').forEach(enhanceSlider);
new MutationObserver(records => {
  for (const r of records) for (const n of r.addedNodes) {
    if (n.nodeType !== 1) continue;
    if (n.matches('input[type=range]')) enhanceSlider(n);
    else n.querySelectorAll?.('input[type=range]').forEach(enhanceSlider);
  }
}).observe(document.body, { childList: true, subtree: true });
// Position pad: drag = X/Y together (top = +Y), double-click = centre.
function padMove(e) {
  const box = $('posPad').getBoundingClientRect();
  const c = t => Math.round(Math.min(1, Math.max(-1, t)) * 100) / 100;
  const x = c((e.clientX - box.left) / box.width * 2 - 1), y = c(1 - (e.clientY - box.top) / box.height * 2);
  setLive('pos_x', 'master.pos_x', x);
  setLive('pos_y', 'master.pos_y', y);
}
$('posPad').addEventListener('pointerdown', e => { $('posPad').setPointerCapture(e.pointerId); padMove(e); });
$('posPad').addEventListener('pointermove', e => { if (e.buttons & 1) padMove(e); });
$('posPad').addEventListener('dblclick', () => { setLive('pos_x', 'master.pos_x', 0); setLive('pos_y', 'master.pos_y', 0); });
$('rotSync').addEventListener('change', () => postLive({ id: 'master.rot.sync', value: $('rotSync').checked }));
const reverse = on => postLive({ id: 'master.rot.reverse', value: on });
$('rotReverse').addEventListener('pointerdown', () => reverse(true));
['pointerup', 'pointerleave'].forEach(ev => $('rotReverse').addEventListener(ev, () => reverse(false)));
let reverseKey = false;
function assignReverseKey(v) { return (reverseKey = v); }

// ---------- live effects (T-142) ----------
// Hold buttons (Flash strobe, Figer, Noir): pressed while the pointer is
// down, released on up, leave or cancel, and when the window loses focus.
const FX_HOLDS = ['fxStrobeHold', 'fxFreeze', 'fxBlack'];
const fxHeld = new Set();
function fxHold(id, on) {
  if (on === fxHeld.has(id)) return;
  if (on) fxHeld.add(id); else fxHeld.delete(id);
  $(id).classList.toggle('active', on);
  postLive({ id: $(id).dataset.control, value: on });
}
FX_HOLDS.forEach(id => {
  $(id).addEventListener('pointerdown', e => { if (e.button === 0) { $(id).setPointerCapture?.(e.pointerId); fxHold(id, true); } });
  ['pointerup', 'pointercancel', 'lostpointercapture'].forEach(ev => $(id).addEventListener(ev, () => fxHold(id, false)));
});
$('fxStrobe').addEventListener('click', () => postLive({ id: 'master.strobe.on', value: !(live && live.strobe.on) }));
$('fxRate').addEventListener('input', () => { $('fxRateV').textContent = `${+$('fxRate').value} Hz`; postLive({ id: 'master.strobe.rate', value: +$('fxRate').value }, true); });
$('fxSync').querySelectorAll('button').forEach(b => b.addEventListener('click', () => postLive({ id: 'master.strobe.sync', value: +b.dataset.sync })));
$('fxDuty').addEventListener('input', () => { $('fxDutyV').textContent = `${$('fxDuty').value} %`; postLive({ id: 'master.strobe.duty', value: $('fxDuty').value / 100 }, true); });
$('fxMirror').querySelectorAll('button').forEach(b => b.addEventListener('click', () => postLive({ id: 'master.mirror', value: +b.dataset.mirror })));
$('fxPrism').addEventListener('input', () => { $('fxPrismV').textContent = $('fxPrism').value; postLive({ id: 'master.prism', value: +$('fxPrism').value }, true); });
const FX_SYNC_BEATS = [1, 0.5, 0.25, 0.125, 0.0625];
function showFx(l) {
  const st = l.strobe, hz = st.rate.hz, beats = st.rate.beats;
  $('fxStrobe').classList.toggle('active', st.on);
  if (!fxHeld.has('fxStrobeHold')) $('fxStrobeHold').classList.toggle('active', st.hold);
  if (!fxHeld.has('fxFreeze')) $('fxFreeze').classList.toggle('active', l.freeze);
  if (!fxHeld.has('fxBlack')) $('fxBlack').classList.toggle('active', l.black);
  const sync = hz !== undefined ? 0 : 1 + FX_SYNC_BEATS.findIndex(b => Math.abs(b - beats) < 1e-4);
  $('fxSync').querySelectorAll('button').forEach(b => b.classList.toggle('active', +b.dataset.sync === sync));
  $('fxRate').disabled = hz === undefined;
  if (hz !== undefined && document.activeElement !== $('fxRate')) $('fxRate').value = hz;
  $('fxRateV').textContent = hz !== undefined ? `${+hz.toFixed(1)} Hz` : `${['1/1', '1/2', '1/4', '1/8', '1/16'][sync - 1] ?? beats} temps`;
  if (document.activeElement !== $('fxDuty')) $('fxDuty').value = Math.round(st.duty * 100);
  $('fxDutyV').textContent = `${Math.round(st.duty * 100)} %`;
  const mirror = ['none', 'x', 'y', 'xy'].indexOf(l.mirror);
  $('fxMirror').querySelectorAll('button').forEach(b => b.classList.toggle('active', +b.dataset.mirror === mirror));
  if (document.activeElement !== $('fxPrism')) $('fxPrism').value = l.prism;
  $('fxPrismV').textContent = l.prism > 1 ? `${l.prism} copies` : 'aucun';
}

// ---------- live colour ----------
const COLOR_MODES = ['normal', 'fixed', 'hue', 'palette', 'rainbow', 'chase'];
const COLOR_STEPS = [0.125, 0.25, 0.5, 1, 2, 4];
let palettes = { builtin: [], user: [] };
const colorMode = () => live ? COLOR_MODES.indexOf(live.color.kind) : 0;
const control = (id, value) => { const sid = scopedId(id); return sid ? post('/api/control', { id: sid, value }) : Promise.resolve(false); };
// The same for a slider or a strip being dragged: latest wins (T-484).
const controlDrag = (id, value) => { const sid = scopedId(id); return sid ? controlLatest({ id: sid, value }) : Promise.resolve(false); };
const setColorMode = m => control('master.color.mode', m);
function showColor(l) {
  const p = l.color_params, mode = COLOR_MODES.indexOf(l.color.kind);
  document.querySelectorAll('#colorModes button').forEach(b => b.classList.toggle('active', +b.dataset.mode === mode));
  if (document.activeElement !== $('colorFixed')) $('colorFixed').value = hex(p.rgb);
  $('hueMark').style.left = (p.hue / 360 * 100) + '%';
  document.querySelectorAll('#palettes button').forEach(b => b.classList.toggle('active', +b.dataset.index === p.palette));
  document.querySelectorAll('#paletteModes button').forEach(b => b.classList.toggle('active', +b.dataset.pm === (p.palette_mode === 'step' ? 1 : 0)));
  if (document.activeElement !== $('colorOffset')) $('colorOffset').value = p.offset;
  $('colorOffsetV').textContent = p.offset;
  const rate = mode === 4 ? p.rainbow_rate : p.chase_step;
  document.querySelectorAll('#colorSteps button').forEach(b => b.classList.toggle('active', rate.beats === COLOR_STEPS[+b.dataset.step]));
  if (document.activeElement !== $('colorSpread')) $('colorSpread').value = Math.round(p.spread * 100);
  $('colorSpreadV').textContent = p.spread.toFixed(2) + ' cycle' + (p.spread > 1 ? 's' : '');
  const cs = ['whole', 'stroke', 'point'].indexOf(p.chase_spread);
  document.querySelectorAll('#chaseSpreads button').forEach(b => b.classList.toggle('active', +b.dataset.cs === cs));
}
async function loadPalettes() {
  palettes = await apiJson('GET', '/api/palettes');
  const all = [...palettes.builtin, ...palettes.user];
  const esc = s => s.replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
  $('palettes').innerHTML = all.map((p, i) => `<button data-index="${i}" title="${esc(p.name)}">`
    + `<div class="sw">${p.colors.map(c => `<i style="background:${hex(c)}"></i>`).join('')}</div>${esc(p.name)}`
    + (i >= palettes.builtin.length ? `<span class="del" data-del="${i - palettes.builtin.length}" title="Supprimer">×</span>` : '')
    + '</button>').join('');
  $('palettes').querySelectorAll('button').forEach(b => b.addEventListener('click', async e => {
    if (e.target.dataset.del !== undefined) {
      const i = +e.target.dataset.del;
      if (!confirm(`Supprimer la palette « ${palettes.user[i].name} » ?`)) return;
      await post('/api/palettes', palettes.user.filter((_, j) => j !== i));
      return loadPalettes();
    }
    await control('master.color.palette', +b.dataset.index);
    // A palette click shows it at once, unless a chase is already using palettes.
    if (colorMode() !== 3 && colorMode() !== 5) setColorMode(3);
  }));
  if (live) showColor(live);
}
$('colorModes').querySelectorAll('button').forEach(b => b.addEventListener('click', () => setColorMode(+b.dataset.mode)));
$('colorBlock').addEventListener('contextmenu', e => { e.preventDefault(); setColorMode(0); });
let fixedTimer = null;
$('colorFixed').addEventListener('input', () => {
  clearTimeout(fixedTimer);
  fixedTimer = setTimeout(async () => {
    const [r, g, b] = rgb($('colorFixed').value);
    await Promise.all([controlDrag('master.color.red', r / 255), controlDrag('master.color.green', g / 255), controlDrag('master.color.blue', b / 255)]);
    if (colorMode() !== 1) setColorMode(1);
  }, 40);
});
function pickHue(e) {
  const box = $('hueStrip').getBoundingClientRect();
  const hue = Math.min(360, Math.max(0, (e.clientX - box.left) / box.width * 360));
  $('hueMark').style.left = (hue / 360 * 100) + '%';
  controlDrag('master.color.hue', hue).then(() => { if (colorMode() !== 2) setColorMode(2); });
}
$('hueStrip').addEventListener('pointerdown', e => { $('hueStrip').setPointerCapture(e.pointerId); pickHue(e); });
$('hueStrip').addEventListener('pointermove', e => { if (e.buttons & 1) pickHue(e); });
$('paletteModes').querySelectorAll('button').forEach(b => b.addEventListener('click', () => control('master.color.palette_mode', +b.dataset.pm)));
$('colorOffset').addEventListener('input', () => { $('colorOffsetV').textContent = $('colorOffset').value; controlDrag('master.color.offset', +$('colorOffset').value); });
$('colorSteps').querySelectorAll('button').forEach(b => b.addEventListener('click', () => control('master.color.rate', +b.dataset.step)));
$('colorSpread').addEventListener('input', () => controlDrag('master.color.spread', $('colorSpread').value / 100));
$('chaseSpreads').querySelectorAll('button').forEach(b => b.addEventListener('click', () => control('master.color.chase_spread', +b.dataset.cs)));
$('paletteNew').addEventListener('click', async () => {
  const name = prompt('Nom de la nouvelle palette :');
  if (!name) return;
  const list = prompt('Couleurs (1 à 16, en hexadécimal, séparées par des espaces) :', '#ff0000 #ffffff #0000ff');
  const colors = (list || '').split(/[\s,;]+/).filter(s => /^#?[0-9a-f]{6}$/i.test(s)).map(s => rgb(s.startsWith('#') ? s : '#' + s));
  if (!colors.length) return alert('Aucune couleur valide.');
  if (!await post('/api/palettes', [...palettes.user, { name, colors: colors.slice(0, 16) }])) return alert('Palette refusée (8 palettes utilisateur au maximum).');
  loadPalettes();
});

export { FX_HOLDS, assignReverseKey, control, controlDefaults, controlKinds, fxHold, liveAnsweredAt,
  liveSentAt, loadPalettes, reverse, reverseKey, scopeMods, scopedId, showDirectScope, showLive,
  sliderLabel, toNeutral };
