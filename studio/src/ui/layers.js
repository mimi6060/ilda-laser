// Laser Studio — layers.js (T-435).
// Sections (as in the former index.html script):
//   layers
//   executors (T-418)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './timeline-canvas.js';
import { $, ROTATING, api, apiJson, controlLatest, post } from './core.js';
import { LOCKED_FR, flash, showMode } from './workspaces.js';
import { ZC_RATES_FR, proj, zcOptions } from './zones.js';
import { mtView, reloadTargets } from './targets.js';
import { SPEED_NAMES } from './tempo.js';
import { grid, presetById, presets, refreshLook } from './cues.js';
import { esc, learnMode, startLearn } from './midi.js';

// ---------- layers ----------
// Four strips: dimmer, Muet, Solo, Vider. Values follow the server (MIDI
// faders show up here); a slider being dragged is left alone.
let layerKey = '';
function buildLayers() {
  for (let n = 1; n <= 4; n++) {
    const d = document.createElement('div');
    d.className = 'layer';
    d.dataset.layer = n;
    d.innerHTML = `<label class="t" for="layerDim${n}">Calque ${n}</label><span class="pts">0 pts</span>
      <input type="range" id="layerDim${n}" min="0" max="100" value="100" data-control="layer.${n}.dimmer" title="layer.${n}.dimmer" aria-label="Gradateur calque ${n}">
      <span class="dv">100 %</span>
      <div class="btns"><button class="mute" data-control="layer.${n}.mute" title="layer.${n}.mute">Muet</button><button class="solo" data-control="layer.${n}.solo" title="layer.${n}.solo">Solo</button></div>
      <button class="clear btn btn--small btn--block" data-control="layer.${n}.clear" title="layer.${n}.clear : arrête les cues du calque">Vider</button>
      <select class="spd u-fs-small" aria-label="Vitesse du calque ${n}" title="Groupe de vitesse du calque ${n} (LIVE › Direct › Vitesses) : ses cues sans groupe propre et ses réglages">
        <option value="0">Tempo</option><option value="1">Vit. A</option><option value="2">Vit. B</option><option value="3">Vit. C</option><option value="4">Vit. D</option>
      </select>`;
    const range = d.querySelector('input');
    range.addEventListener('input', () => {
      d.querySelector('.dv').textContent = range.value + ' %';
      controlLatest({ id: `layer.${n}.dimmer`, value: range.value / 100 });
    });
    for (const p of ['mute', 'solo']) {
      const b = d.querySelector('.' + p);
      // The next frame shows the new state.
      b.addEventListener('click', () => post('/api/control', { id: `layer.${n}.${p}`, value: !b.classList.contains('active') }));
    }
    d.querySelector('.clear').addEventListener('click', async () => {
      if (await post('/api/control', { id: `layer.${n}.clear` })) refreshLook();
    });
    d.querySelector('.spd').addEventListener('change', e => post('/api/speed-groups', { layer: n, layer_group: +e.target.value }));
    $('layers').appendChild(d);
  }
}
// Group masters (T-420): one vertical fader per zone group, hidden while
// there is none. Values follow the server (MIDI, OSC, DMX); a fader being
// dragged is left alone.
let gmKey = '';
function showGroupMasters(groups) {
  groups = (groups || []).filter(g => g.control);
  EXEC.groups = groups.map(g => ({ n: g.n, name: g.name }));
  $('groupMasters').hidden = !groups.length;
  const key = groups.map(g => g.n + ':' + g.name).join('|');
  if (key !== gmKey) {
    gmKey = key;
    $('gmRow').replaceChildren(...groups.map(g => {
      const d = document.createElement('div');
      d.className = 'layer';
      d.dataset.group = g.n;
      const id = `group.${g.n}.master`;
      d.innerHTML = `<label class="t" for="gmDim${g.n}"></label>
        <input type="range" id="gmDim${g.n}" min="0" max="100" value="100" data-control="${id}" title="${id}">
        <span class="dv">100 %</span>`;
      d.querySelector('label').textContent = g.name;
      d.querySelector('label').title = g.name;
      const range = d.querySelector('input');
      range.setAttribute('aria-label', 'Maître du groupe ' + g.name);
      range.addEventListener('input', () => {
        d.querySelector('.dv').textContent = range.value + ' %';
        controlLatest({ id, value: range.value / 100 });
      });
      return d;
    }));
  }
  groups.forEach((g, i) => {
    const d = $('gmRow').children[i];
    const range = d.querySelector('input');
    if (document.activeElement !== range) {
      range.value = Math.round(g.master * 100);
      d.querySelector('.dv').textContent = range.value + ' %';
    }
    d.classList.toggle('off', g.master <= 0);
    // The reminder in RÉGLAGES › Zones de projection › Groupes.
    const lvl = document.querySelector(`#pzGroups .pz-group[data-group="${g.n}"] .gm-level`);
    if (lvl) lvl.textContent = `Maître : ${Math.round(g.master * 100)} % (LIVE)`;
  });
}
$('gmAllFull').addEventListener('click', () => post('/api/control', { id: 'group.all_full' }));
// Zone chasers (T-170) in LIVE: one toggle per chaser (its name and the
// group it is on now), the speed of them all. Hidden while there is none.
let zcKey = '';
function showZoneChase(v) {
  const chasers = (v && v.chasers) || [];
  $('zoneChase').hidden = !chasers.length;
  const rate = $('zcRate');
  if (!rate.options.length) zcOptions(rate, ZC_RATES_FR.map((t, i) => [i, t]), 2);
  if (v && document.activeElement !== rate) rate.value = String(v.rate);
  if ($('pzChaseRate').options.length && document.activeElement !== $('pzChaseRate') && v) $('pzChaseRate').value = String(v.rate);
  const key = chasers.map(c => c.n + ':' + c.name).join('|');
  if (key !== zcKey) {
    zcKey = key;
    $('zcRow').replaceChildren(...chasers.map(c => {
      const b = document.createElement('button');
      b.className = 'zc-btn';
      b.dataset.control = c.control;
      b.dataset.chaser = c.n;
      b.title = c.control;
      b.innerHTML = '<span class="zc-name"></span><span class="zc-on"></span>';
      b.querySelector('.zc-name').textContent = c.name;
      b.addEventListener('click', () => post('/api/control', { id: c.control, value: b.getAttribute('aria-pressed') === 'true' ? 0 : 1 }));
      return b;
    }));
  }
  const groups = (proj && proj.groups) || [];
  chasers.forEach((c, i) => {
    const b = $('zcRow').children[i];
    const on = c.group ? (groups[c.group - 1]?.name || 'Groupe ' + c.group) : '—';
    b.setAttribute('aria-pressed', String(!!c.enabled));
    b.setAttribute('aria-label', `Chenillard ${c.name} : ${c.enabled ? 'en marche' : 'arrêté'}, groupe ${on}`);
    b.querySelector('.zc-on').textContent = on;
    // RÉGLAGES: running and where each one stands.
    const box = document.querySelector(`#pzChasers .pz-chaser[data-chaser="${c.n}"]`);
    if (box) {
      const run = box.querySelector('.zc-run input');
      if (document.activeElement !== run) run.checked = !!c.enabled;
      box.querySelector('.zc-now').textContent = c.step ? `Étape ${c.step} · ${on}` : '';
    }
  });
}
$('zcRate').addEventListener('change', () => post('/api/control', { id: 'chaser.rate', value: +$('zcRate').value }));

// ---------- executors (T-418) ----------
// Pages of 8 executors: a fader and two buttons each, assigned with a
// right click (« Affecter ») or a cue dropped from the grid. The handles
// on screen drive the page shown (`exec.cur.*`), like a controller's;
// changing page cuts nothing and a dot marks the pages where something
// is active. Assigning is an edit (history, locked in show mode); playing
// is not. No executor ever arms.
const EXEC_N = 8, EXEC_MAX_PAGES = 8;
const EXEC_FADER_FR = { level: 'Niveau', fade_in: 'Fondu au curseur', speed: 'Vitesse' };
const EXEC_BUTTON_FR = { none: 'Aucune', toggle: 'Basculer', flash: 'Flash', temp: 'Temp', black_held: 'Noir tenu', go: 'Go' };
// What each object accepts (the server checks the same, executors.rs).
const EXEC_FADERS = { cue: ['fade_in', 'level'], layer: ['level', 'speed'], group_master: ['level'], speed_group: ['speed'], cue_list: ['level'], none: ['level'] };
const EXEC_BUTTONS = { cue: ['none', 'toggle', 'flash', 'temp', 'black_held'], layer: ['none', 'toggle', 'flash', 'black_held'],
  group_master: ['none', 'toggle', 'flash', 'black_held'], cue_list: ['none', 'go'], none: ['none'] };
const EXEC = { pages: null, rev: -1, page: 0, key: '', state: null, groups: [], menu: null, keys: new Map(), held: new Map() };
const execTarget = (t = {}) => t.type || 'none';
function execTargetName(t) {
  switch (execTarget(t)) {
    case 'cue': return presetById.get(t.id)?.name || t.id;
    case 'layer': return 'Calque ' + t.n;
    case 'group_master': return EXEC.groups.find(g => g.n === t.group)?.name || 'Groupe ' + t.group;
    case 'cue_list': return 'Liste de cues';
    case 'speed_group': return 'Vitesse ' + (SPEED_NAMES[t.n] || t.n);
    case 'macro': return 'Macro';
    default: return '—';
  }
}
const execHandle = n => EXEC.pages?.[EXEC.page]?.handles?.[n] || { target: { type: 'none' }, fader: 'level', buttons: ['none', 'none'], label: '' };
async function loadExecutors() {
  let r;
  try { r = await apiJson('GET', '/api/executors', undefined, { quiet: true }); } catch { return; }
  EXEC.pages = r.pages; EXEC.rev = r.state.rev; EXEC.key = '';
  showExecutors(r.state);
}
// The 8 columns of the page shown, rebuilt when the page or its
// assignments change; values follow the server (MIDI, OSC, DMX).
function buildExecRow() {
  const key = JSON.stringify([EXEC.page, EXEC.pages?.[EXEC.page], presets.length, EXEC.groups.map(g => g.name)]);
  if (key === EXEC.key) return;
  EXEC.key = key;
  $('execRow').replaceChildren(...Array.from({ length: EXEC_N }, (_, n) => {
    const h = execHandle(n), t = execTarget(h.target), i = n + 1;
    const d = document.createElement('div');
    d.className = 'exec-col' + (t === 'none' ? ' empty' : '');
    d.dataset.n = i;
    const name = h.label || execTargetName(h.target);
    const fn = t === 'none' ? '' : EXEC_FADER_FR[h.fader] || '';
    // The object's name (right click: « Affecter »), then the fader's
    // value, which is also its label (right click: back to 0).
    d.innerHTML = `<span class="t"></span>
      <label class="dv" id="execF${i}V" for="execF${i}">0 %</label>
      <input type="range" id="execF${i}" min="0" max="100" value="0" data-control="exec.cur.${i}.fader">
      <button class="b1" data-control="exec.cur.${i}.button"></button>
      <button class="b2" data-control="exec.cur.${i}.button2"></button>`;
    const lab = d.querySelector('.t');
    lab.textContent = name;
    lab.title = `${i}. ${name}${fn ? ' · ' + fn : ''} · clic droit : affecter`;
    const range = d.querySelector('input');
    range.setAttribute('aria-label', `Exécuteur ${i} : ${name}${fn ? ', ' + fn : ''}`);
    range.title = `exec.cur.${i}.fader${fn ? ' · ' + fn : ''}`;
    range.addEventListener('input', () => {
      d.querySelector('.dv').textContent = range.value + ' %';
      controlLatest({ id: `exec.cur.${i}.fader`, value: range.value / 100 });
    });
    ['b1', 'b2'].forEach((c, b) => {
      const btn = d.querySelector('.' + c), f = h.buttons?.[b] || 'none';
      btn.textContent = f === 'none' ? '—' : EXEC_BUTTON_FR[f];
      btn.title = `exec.cur.${i}.button${b ? '2' : ''} · bouton ${b + 1} : ${EXEC_BUTTON_FR[f]}`;
      btn.setAttribute('aria-label', `Exécuteur ${i}, bouton ${b + 1} : ${EXEC_BUTTON_FR[f]}`);
    });
    return d;
  }));
}
function showExecutors(st) {
  if (!st) return;
  EXEC.state = st;
  if (st.rev !== EXEC.rev || !EXEC.pages) {
    // Assignments changed (another page, undo, a project opened).
    if (EXEC.loading !== st.rev) { EXEC.loading = st.rev; loadExecutors(); }
    return;
  }
  EXEC.page = st.page;
  const pages = EXEC.pages.length;
  const tabKey = JSON.stringify([EXEC.pages.map(p => p.name), st.page, st.active_pages]);
  if (tabKey !== EXEC.tabKey) {
    EXEC.tabKey = tabKey;
    $('execTabs').replaceChildren(...EXEC.pages.map((p, i) => {
      const b = document.createElement('button');
      b.setAttribute('role', 'tab');
      b.setAttribute('aria-selected', String(i === st.page));
      b.dataset.page = i + 1;
      b.dataset.control = `exec.page.${i + 1}`;
      b.textContent = p.name;
      const live = !!st.active_pages?.[i];
      b.classList.toggle('live', live);
      b.title = `exec.page.${i + 1}${live ? ' · quelque chose y est actif' : ''}`;
      return b;
    }));
  }
  $('execPageAdd').disabled = pages >= EXEC_MAX_PAGES;
  buildExecRow();
  document.querySelectorAll('#execRow .exec-col').forEach((d, n) => {
    const range = d.querySelector('input'), v = Math.round((st.levels?.[n] ?? 0) * 100);
    if (document.activeElement !== range) {
      range.value = v;
      d.querySelector('.dv').textContent = v + ' %';
    }
    d.classList.toggle('on', !!st.active?.[n]);
    d.querySelector('.b1').classList.toggle('held', !!st.held?.[n]?.[0]);
    d.querySelector('.b2').classList.toggle('held', !!st.held?.[n]?.[1]);
  });
}
// Buttons: held while the pointer is down (Flash, Temp, Noir tenu);
// Basculer / Go act on the press.
function execPress(id) { EXEC.held.set(id, true); return post('/api/control', { id, value: 1 }); }
function execRelease(id) { if (!EXEC.held.delete(id)) return; post('/api/control', { id, value: 0 }); }
function execReleaseAll() {
  for (const id of [...EXEC.held.keys()]) execRelease(id);
  EXEC.keys.clear();
}
const execPointers = new Map();
$('execRow').addEventListener('pointerdown', e => {
  const b = e.target.closest('.exec-col button');
  if (!b || e.button !== 0 || learnMode) return;
  try { b.setPointerCapture(e.pointerId); } catch { /* synthetic pointer */ }
  execPointers.set(e.pointerId, b.dataset.control);
  execPress(b.dataset.control);
});
['pointerup', 'pointercancel', 'lostpointercapture'].forEach(ev => $('execRow').addEventListener(ev, e => {
  const id = execPointers.get(e.pointerId);
  if (!id) return;
  execPointers.delete(e.pointerId);
  execRelease(id);
}));
function execKeyDown(e, i) {
  if (e.repeat || EXEC.keys.has(e.code)) return;
  const id = `exec.cur.${i + 1}.button`;
  EXEC.keys.set(e.code, id);
  execPress(id);
}
function execKeyUp(e) {
  const id = EXEC.keys.get(e.code);
  if (!id) return;
  EXEC.keys.delete(e.code);
  execRelease(id);
}
$('execTabs').addEventListener('click', e => {
  const b = e.target.closest('button[data-page]');
  if (b && !learnMode) post('/api/control', { id: `exec.page.${b.dataset.page}` });
});
// Assignments: the whole bank goes back to the server (validated there).
async function saveExecBank(pages, msg) {
  if (showMode) { flash(LOCKED_FR); return false; }
  const r = await api('POST', '/api/executors', { pages }, { quiet: true });
  if (!r.ok) {
    const text = await r.text();
    if (msg) $('execMenuMsg').textContent = text;
    else flash(text);
    return false;
  }
  const j = await r.json();
  EXEC.pages = j.pages; EXEC.rev = j.state.rev; EXEC.key = ''; EXEC.tabKey = '';
  showExecutors(j.state);
  return true;
}
function setExecHandle(page, n, h) {
  const pages = structuredClone(EXEC.pages);
  pages[page].handles[n] = h;
  return saveExecBank(pages, !$('execMenu').hidden);
}
$('execPageAdd').addEventListener('click', async () => {
  if (!EXEC.pages || EXEC.pages.length >= EXEC_MAX_PAGES) return;
  const pages = structuredClone(EXEC.pages);
  pages.push({ name: 'Page ' + (pages.length + 1), handles: [] });
  if (await saveExecBank(pages)) post('/api/control', { id: `exec.page.${pages.length}` });
});
// « Affecter »: the menu of one column.
function fillExecMenu() {
  const { page, n } = EXEC.menu, h = execHandle(n), t = execTarget(h.target);
  $('execMenuTitle').textContent = `Exécuteur ${page + 1}.${n + 1}`;
  const opt = (v, text, sel) => `<option value="${esc(v)}"${sel ? ' selected' : ''}>${esc(text)}</option>`;
  const cur = JSON.stringify(h.target || { type: 'none' });
  const cues = [];
  for (const pg of grid.pages || []) {
    const ids = (pg.cells || []).filter(id => id && presetById.has(id));
    if (ids.length) cues.push(`<optgroup label="${esc(pg.name)}">${ids.map(id => opt(JSON.stringify({ type: 'cue', id }), presetById.get(id).name, cur === JSON.stringify({ type: 'cue', id }))).join('')}</optgroup>`);
  }
  // A cue not on any page (set elsewhere) stays choosable.
  if (t === 'cue' && !(grid.pages || []).some(pg => (pg.cells || []).includes(h.target.id))) cues.unshift(opt(cur, execTargetName(h.target), true));
  const others = [opt('{"type":"none"}', 'Aucun', t === 'none')]
    .concat([1, 2, 3, 4].map(l => opt(JSON.stringify({ type: 'layer', n: l }), 'Calque ' + l, cur === JSON.stringify({ type: 'layer', n: l }))))
    .concat(EXEC.groups.map(g => opt(JSON.stringify({ type: 'group_master', group: g.n }), 'Maître : ' + g.name, cur === JSON.stringify({ type: 'group_master', group: g.n }))))
    // Speed groups (T-419): the fader's fifths are ×¼ … ×4.
    .concat([1, 2, 3, 4].map(g => opt(JSON.stringify({ type: 'speed_group', n: g }), 'Groupe de vitesse ' + SPEED_NAMES[g], cur === JSON.stringify({ type: 'speed_group', n: g }))))
    .concat([opt('{"type":"cue_list"}', 'Liste de cues', t === 'cue_list')]);
  $('execMenuTarget').innerHTML = others.join('') + cues.join('');
  $('execMenuFader').innerHTML = (EXEC_FADERS[t] || ['level']).map(f => opt(f, EXEC_FADER_FR[f], f === h.fader)).join('');
  $('execMenuFader').disabled = t === 'none' || t === 'cue_list';
  for (const [b, el] of [[0, 'execMenuB1'], [1, 'execMenuB2']]) {
    $(el).innerHTML = (EXEC_BUTTONS[t] || ['none']).map(f => opt(f, EXEC_BUTTON_FR[f], f === (h.buttons?.[b] || 'none'))).join('');
    $(el).disabled = t === 'none';
  }
  if (document.activeElement !== $('execMenuLabel')) $('execMenuLabel').value = h.label || '';
}
function openExecMenu(n, x, y) {
  if (showMode) return flash(LOCKED_FR);
  if (!EXEC.pages) return;
  EXEC.menu = { page: EXEC.page, n };
  $('execMenuMsg').textContent = '';
  fillExecMenu();
  const m = $('execMenu');
  m.hidden = false;
  m.style.left = Math.max(8, Math.min(x, innerWidth - m.offsetWidth - 8)) + 'px';
  m.style.top = Math.max(8, Math.min(y, innerHeight - m.offsetHeight - 8)) + 'px';
  $('execMenuTarget').focus();
}
function closeExecMenu() { $('execMenu').hidden = true; EXEC.menu = null; }
$('execRow').addEventListener('contextmenu', e => {
  const col = e.target.closest('.exec-col');
  if (!col) return;
  e.preventDefault();
  openExecMenu(+col.dataset.n - 1, e.clientX, e.clientY);
});
async function execMenuChange(edit) {
  if (!EXEC.menu) return;
  const { page, n } = EXEC.menu;
  const h = structuredClone(execHandle(n));
  edit(h);
  const t = execTarget(h.target);
  // Functions the new object doesn't take fall back to its first one.
  if (!(EXEC_FADERS[t] || ['level']).includes(h.fader)) h.fader = (EXEC_FADERS[t] || ['level'])[0];
  h.buttons = (h.buttons || ['none', 'none']).map(f => (EXEC_BUTTONS[t] || ['none']).includes(f) ? f : 'none');
  if (await setExecHandle(page, n, h) && EXEC.menu) { $('execMenuMsg').textContent = ''; fillExecMenu(); }
}
$('execMenuTarget').addEventListener('change', e => execMenuChange(h => {
  const t = JSON.parse(e.target.value);
  const was = execTarget(h.target);
  h.target = t;
  // A cue just set: a fader that fades it in, Flash on button 1.
  if (t.type === 'cue' && was !== 'cue') { h.fader = 'fade_in'; h.buttons = ['flash', 'none']; }
  if (t.type === 'cue_list') h.buttons = ['go', 'none'];
}));
$('execMenuFader').addEventListener('change', e => execMenuChange(h => { h.fader = e.target.value; }));
$('execMenuB1').addEventListener('change', e => execMenuChange(h => { h.buttons = [e.target.value, h.buttons?.[1] || 'none']; }));
$('execMenuB2').addEventListener('change', e => execMenuChange(h => { h.buttons = [h.buttons?.[0] || 'none', e.target.value]; }));
$('execMenuLabel').addEventListener('change', e => execMenuChange(h => { h.label = e.target.value.trim(); }));
$('execMenuClear').addEventListener('click', () => execMenuChange(h => { h.target = { type: 'none' }; h.label = ''; h.buttons = ['none', 'none']; h.fader = 'level'; }));
$('execMenuClose').addEventListener('click', closeExecMenu);
for (const [el, which] of [['execMenuLearnF', 'fader'], ['execMenuLearnB1', 'button'], ['execMenuLearnB2', 'button2']]) {
  $(el).addEventListener('click', () => { const n = EXEC.menu?.n; closeExecMenu(); if (n != null) startLearn(`exec.cur.${n + 1}.${which}`, false); });
}
document.addEventListener('pointerdown', e => {
  if (!$('execMenu').hidden && !e.target.closest('#execMenu, #execRow')) closeExecMenu();
}, true);
$('execMenu').addEventListener('keydown', e => { if (e.key === 'Escape') closeExecMenu(); });
// A cue dropped on a column (from « Réorganiser » or the timeline's library).
$('execRow').addEventListener('dragover', e => {
  const col = e.target.closest('.exec-col');
  if (!col || showMode) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = 'copy';
  document.querySelectorAll('#execRow .drop').forEach(x => x !== col && x.classList.remove('drop'));
  col.classList.add('drop');
});
$('execRow').addEventListener('dragleave', e => { e.target.closest?.('.exec-col')?.classList.remove('drop'); });
$('execRow').addEventListener('drop', e => {
  const col = e.target.closest('.exec-col');
  document.querySelectorAll('#execRow .drop').forEach(x => x.classList.remove('drop'));
  if (!col) return;
  e.preventDefault();
  const id = e.dataTransfer.getData('text/plain');
  if (!presetById.has(id) || !EXEC.pages) return;
  const n = +col.dataset.n - 1, h = structuredClone(execHandle(n));
  const keep = execTarget(h.target) === 'cue';
  setExecHandle(EXEC.page, n, { ...h, target: { type: 'cue', id }, fader: keep ? h.fader : 'fade_in', buttons: keep ? h.buttons : ['flash', 'none'] });
});
loadExecutors();
function showLayers(l) {
  if (!l) return;
  const key = JSON.stringify(l);
  if (key === layerKey) return;
  layerKey = key;
  showGroupMasters(l.groups);
  showZoneChase(l.chase);
  const { mixer, mix } = l;
  const anySolo = mixer.layers.some(x => x.solo);
  document.querySelectorAll('#layers .layer').forEach((d, i) => {
    const x = mixer.layers[i], n = i + 1;
    const range = d.querySelector('input');
    if (document.activeElement !== range) {
      range.value = Math.round(x.dimmer * 100);
      d.querySelector('.dv').textContent = range.value + ' %';
    }
    d.querySelector('.mute').classList.toggle('active', x.mute);
    d.querySelector('.solo').classList.toggle('active', x.solo);
    const spd = d.querySelector('.spd');
    if (document.activeElement !== spd) spd.value = String(x.speed_group || 0);
    d.classList.toggle('off', x.mute || x.dimmer <= 0 || (anySolo && !x.solo));
    const pts = d.querySelector('.pts');
    const cut = mix.dropped.includes(n);
    pts.textContent = cut ? 'coupé' : mix.per_layer[i] + ' pts';
    pts.classList.toggle('cut', cut);
  });
  const over = mix.demand > mix.budget;
  $('layerPoints').classList.toggle('over', over);
  $('layerPoints').querySelector('b').textContent = `${mix.demand} / ${mix.budget}`;
  $('layerWarn').textContent = !over ? ''
    : mix.dropped.length > 1 ? `· budget dépassé : calques ${mix.dropped.slice().sort().join(', ')} coupés`
    : mix.dropped.length ? `· budget dépassé : calque ${mix.dropped[0]} coupé`
    : '· budget dépassé : points espacés';
}
buildLayers();

const GEN_NAMES = { lissajous: 'Lissajous', spirograph: 'Spirographe', rose: 'Rosace', tunnel: 'Tunnel',
  polygon_tunnel: 'Tunnel polygonal', spiral_arms: 'Spirale à bras', starburst: 'Soleil', beam_fan: 'Faisceaux en éventail',
  beam_circle: 'Cône de faisceaux', beam_wave: 'Vague de faisceaux', sweep: 'Balayage', liquid_sky: 'Nappe',
  grid_scan: 'Grille de balayage', sine_stack: 'Oscillateurs', helix: 'Hélice', pulse_rings: 'Anneaux pulsés',
  spectrum: 'Spectre audio', clock: 'Horloge / anneau de progression', vortex: 'Vortex', flower: 'Fleur',
  fan: 'Éventail', fan_sweep: 'Éventail balayé', fan_tilt: 'Éventail qui se lève', fan_wave: 'Éventail vague',
  positions: 'Positions au temps', finger_tunnel: 'Tunnel de faisceaux', tunnel_pump: 'Tunnel qui pompe',
  twin_tunnel: 'Double tunnel', sunburst: 'Soleil levant',
  ceiling: 'Plafond liquide', blade: 'Lame', curtain: 'Rideaux', waterfall: 'Cascade', scanner: 'Scanner',
  slats: 'Lamelles', aurora: 'Aurore', grid: 'Grille', chase_fan: 'Chaser',
  scissor: 'Ciseaux (X)', hot_beam: 'Faisceau chaud', bundle_burst: 'Faisceaux qui s’ouvrent', beam_v: 'V de faisceaux',
  starfield: 'Ciel étoilé', lightning: 'Éclairs', fat_beams: 'Faisceaux épais',
  countdown: 'Compte à rebours', wire3d: 'Objet 3D', mirror_targets: 'Cibles miroir' };

// Festival fans (T-102) and sheets (T-106): what Nombre / A / B mean for
// each, plus how the tempo settings drive it. Old generators keep the
// generic labels. Sheets stay above the audience (never scan it).
const GEN_HELP = {
  // Mirror targets (T-109).
  mirror_targets: ['Non utilisé.', 'Masque des cibles : cochez-les ci-dessous (1 = cible 1, 2 = cible 2, 4 = cible 3…).', 'Allumage : 0 = toutes ensemble, 1 = une par pas.',
    'En chase, une cible par pas (Pas par temps 1 = une par temps), jamais plus vite que le limiteur de strobe ; ← : dans l’autre sens. Taille et rotation ne déplacent pas les cibles.'],
  // Graphics (T-108).
  countdown: ['Nombre de départ (4 = 4, 3, 2, 1 ; jusqu’à 99).', 'Non utilisé.', 'Non utilisé.',
    'Un nombre par pas depuis le début du cue (Pas par temps 1/4 = un par mesure, 1 = un par temps), puis noir dès le dernier 1/2 temps. Taille = hauteur du chiffre.'],
  clock: ['Non utilisé.', 'Non utilisé.', 'Non utilisé.',
    'En tempo : anneau de progression qui se remplit sur une période (16 temps = se ferme en 4 mesures), puis repart ; ← : il se vide. Sans tempo : horloge à aiguilles.'],
  wire3d: ['Non utilisé.', 'Objet (voir la liste ci-dessous).', 'Non utilisé.',
    'Fil de fer en perspective, un tour par période (8 temps = 2 mesures) sur deux axes ; ← : dans l’autre sens. Taille = rayon.'],
  // Textures (T-107).
  starfield: ['Étoiles (4 à 16, 4 à 10 conseillé).', 'Chute : 0 = étoiles fixes qui scintillent, 1 = pluie (unités par temps).', 'Hauteur la plus basse au-dessus du public.',
    'Nouvelles étoiles à chaque pas (Pas par temps : 4 = 1/4 temps) ; chacune vit 1/8 à 1/4 de temps, scintille et dérive, jamais immobile. Taille = largeur du ciel.'],
  lightning: ['Segments de l’éclair (6 à 10).', '0 = un éclair à chaque temps, 1 = à chaque mesure.', 'Hauteur du point bas au-dessus du public.',
    'Visible 2 à 4 images puis éteint ; à tempo très rapide, un temps sur deux pour rester sous la limite du strobe.'],
  fat_beams: ['Faisceaux (4 à 6 conseillé, 3 à 8).', 'Rayon du petit cercle (0,01 à 0,03) : épaissit le faisceau dans la fumée.', 'Hauteur au-dessus du public.',
    'L’éventail se balance doucement sur une période. Taille = demi-largeur de l’éventail.'],
  // Crossings, hot beam and convergences (T-104).
  scissor: ['Faisceaux, en deux groupes (8 conseillé).', 'Amplitude (0,4 à 0,6) : au-dessus de 0,3, les groupes se croisent deux fois par période.',
    'Hauteur au-dessus du public.', 'Groupe gauche en couleur principale, droit en deuxième couleur (Uni ou Alterné) ; inclinés de ±15° pour lire un X. Taille = largeur des groupes.'],
  hot_beam: ['Non utilisé (un seul faisceau).', 'Amplitude du panoramique (0,3 conseillé).', 'Luminosité max (0,6 par défaut, 0 à 1).',
    'Un aller-retour par période (16 temps conseillés) ; un petit cercle s’y ajoute pour qu’il ne reste jamais immobile.'],
  bundle_burst: ['Faisceaux (8 à 12).', 'Durée d’ouverture en temps (0,25 = 1/4 temps).', 'Hauteur au-dessus du public.',
    'Groupés en un seul faisceau, ils s’ouvrent en éventail sur le premier temps de chaque période, puis se referment avant le suivant. Taille = demi-largeur ouverte.'],
  beam_v: ['Faisceaux, en deux jambes (10 = 2 × 5).', 'Bascule en degrés (0 = fixe, jusqu’à 45).', '0 = V, 1 = Λ (pointe en haut).',
    'Jambes à ±30° de la verticale ; avec A, le V penche d’un côté puis de l’autre, une position par pas. Taille = longueur des jambes.'],
  chase_fan: ['Faisceaux (8 conseillé).', 'Forme du chaser (voir la liste ci-dessous).', 'Traîne : 0 à 3 faisceaux derrière la tête (40 %, 15 %, 6 %).',
    'Un pas par Pas par temps (2 = 1/2 temps) ; seuls les faisceaux allumés changent, l’éventail reste fixe. Alterné : traîne en deuxième couleur.'],
  fan: ['Faisceaux (6 à 16).', 'Pompage : 0 = fixe, 1 = s’ouvre sur chaque temps puis se referme.', 'Hauteur au-dessus du public.',
    'Taille = demi-largeur de l’éventail.'],
  fan_sweep: ['Faisceaux (6 à 16).', 'Amplitude du balayage (0,25 à 0,45).', 'Hauteur au-dessus du public.',
    'Un aller-retour par période ; Miroir : les groupes se croisent, Décalé : une vague d’un groupe à l’autre.'],
  fan_tilt: ['Faisceaux (6 à 16).', 'Non utilisé.', 'Non utilisé.',
    'Monte sur une période puis redescend d’un coup ; ← : descend ; ↔ : monte et redescend. Miroir : un groupe monte, l’autre descend.'],
  fan_wave: ['Faisceaux (10 à 16).', 'Amplitude de la vague (0,15 à 0,4).', 'Hauteur moyenne au-dessus du public.',
    'Une longueur d’onde tous les 2 temps ; ← : la vague part dans l’autre sens.'],
  positions: ['Faisceaux (6 à 16).', 'Non utilisé.', 'Hauteur des positions.',
    '4 positions (large haut, incliné gauche, incliné droite, V), une par pas : Pas par temps règle la cadence.'],
  // Tunnels, cones and sun (T-105): Forme A = turns per beat when it turns.
  finger_tunnel: ['Faisceaux sur le cercle (8 à 16).', 'Tours par temps (voir Vitesse).', 'Non utilisé.',
    'Taille = rayon du cône (0,25 à 0,4). ← : tourne dans l’autre sens.'],
  tunnel_pump: ['Non utilisé.', 'Pompage : le rayon grandit de A (0,3 = 30 %) sur chaque temps et revient en 1/2 temps.',
    '0 = pompe sur le temps, 1 = rampe : se resserre sur la période puis s’ouvre.',
    'Taille = rayon au repos. Rampe : de la taille à un seul faisceau sur la période ; ← : s’ouvre au lieu de se resserrer.'],
  twin_tunnel: ['Non utilisé.', 'Tours par temps (1/8 = un tour en 2 mesures).', 'Non utilisé.',
    'Deux anneaux qui tournent en sens contraires, avec un trou pour voir la rotation. Taille = rayon extérieur.'],
  sunburst: ['Rayons (12 à 24).', 'Tours par temps (voir Vitesse).', '0 = tous pareils, 1 = pair/impair : 100 % / 40 % qui s’échangent à chaque temps.',
    'Rayons au-dessus de l’horizon seulement ; ceux qui passent dessous sont éteints. Taille = longueur des rayons.'],
  ceiling: ['Non utilisé.', 'Ondulation (0,01 à 0,03).', 'Hauteur du plafond au-dessus du public (0,05 à 0,6).',
    'L’ondulation avance d’un quart de cycle par temps ; en tempo, la luminosité respire (70 à 100 %) sur 8 temps.'],
  blade: ['Non utilisé.', 'Non utilisé.', 'Hauteur du pivot au-dessus du public.',
    'S’incline de ±30° sur une période (16 temps conseillés) ; la partie qui passerait sous l’horizon est coupée.'],
  curtain: ['Rideaux (3 à 5).', 'Glissement latéral (0 = fixes, jusqu’à 0,3).', 'Bas des rideaux au-dessus du public.',
    'Traits verticaux de 0,6 de haut ; avec A, ils glissent d’un côté à l’autre sur une période.'],
  waterfall: ['Couloirs en quinconce (4 à 8).', 'Non utilisé.', 'Hauteur d’arrivée au-dessus du public.',
    'Chaque trait descend de 0,9 à l’arrivée en 2 temps ; un nouveau part à chaque pas (Pas par temps 2 = tous les 1/2 temps).'],
  scanner: ['Non utilisé.', '0 = barre horizontale qui monte et descend, 1 = barre verticale qui balaye.', 'Bas de la zone au-dessus du public.',
    'Un passage par période (4 temps = une mesure), linéaire ; Fin de passage : aller-retour ou bouclé.'],
  slats: ['Segments (6 à 12).', 'Longueur des parties allumées, en segments (moitié allumée).', 'Hauteur de la nappe.',
    'Le motif avance d’un segment à chaque pas (Pas par temps 2 = tous les 1/2 temps) ; ← : dans l’autre sens.'],
  aurora: ['Non utilisé.', 'Amplitude (0,1 à 0,25).', 'Hauteur moyenne au-dessus du public.',
    'Trois ondulations lentes (0,03, 0,05 et 0,08 cycle par temps) ; mode Dégradé conseillé.'],
  grid: ['Lignes horizontales (6 max).', 'Lignes verticales (0 à 6).', 'Bas de la grille au-dessus du public.',
    'La grille défile d’une ligne par temps ; ← : dans l’autre sens.'],
};
const GEN_DEFAULTS = {
  mirror_targets: { count: 1, a: 1, b: 0, period: 4, steps: 1 },
  countdown: { count: 4, a: 0, b: 0, period: 4, steps: 0.25 },
  wire3d: { count: 1, a: 0, b: 0, period: 8, steps: 1 },
  starfield: { count: 6, a: 0, b: 0.1, period: 4, steps: 4 },
  lightning: { count: 8, a: 0, b: 0.1, period: 4, steps: 1 },
  fat_beams: { count: 5, a: 0.02, b: 0.3, period: 8, steps: 1 },
  scissor: { count: 8, a: 0.5, b: 0.3, period: 2, steps: 1 },
  hot_beam: { count: 1, a: 0.3, b: 0.6, period: 16, steps: 1 },
  bundle_burst: { count: 10, a: 0.25, b: 0.35, period: 4, steps: 1 },
  beam_v: { count: 10, a: 0, b: 0, period: 4, steps: 1 },
  chase_fan: { count: 8, a: 0, b: 2, period: 4, steps: 2 },
  fan: { count: 8, a: 0, b: 0.3, period: 4, steps: 1 },
  fan_sweep: { count: 8, a: 0.35, b: 0.3, period: 4, steps: 1 },
  fan_tilt: { count: 8, a: 0, b: 0, period: 8, steps: 1 },
  fan_wave: { count: 12, a: 0.25, b: 0.4, period: 4, steps: 1 },
  positions: { count: 8, a: 0, b: 0.3, period: 4, steps: 1 },
  finger_tunnel: { count: 12, a: 0.0625, b: 0, period: 4, steps: 1 },
  tunnel_pump: { count: 8, a: 0.3, b: 0, period: 16, steps: 1 },
  twin_tunnel: { count: 8, a: 0.125, b: 0, period: 8, steps: 1 },
  sunburst: { count: 16, a: 0.03125, b: 0, period: 4, steps: 1 },
  ceiling: { count: 1, a: 0.02, b: 0.15, period: 16, steps: 1 },
  blade: { count: 1, a: 0, b: 0.4, period: 16, steps: 1 },
  curtain: { count: 4, a: 0, b: 0, period: 8, steps: 1 },
  waterfall: { count: 6, a: 0, b: 0, period: 4, steps: 2 },
  scanner: { count: 1, a: 0, b: 0.05, period: 4, steps: 1 },
  slats: { count: 8, a: 2, b: 0.3, period: 4, steps: 2 },
  aurora: { count: 1, a: 0.2, b: 0.4, period: 16, steps: 1, mode: 'gradient' },
  grid: { count: 4, a: 4, b: 0.1, period: 4, steps: 1 },
};
function showGenHelp(name) {
  const h = GEN_HELP[name] || [];
  ['gCountH', 'gAH', 'gBH', 'gSyncH'].forEach((id, i) => { $(id).textContent = h[i] || ''; $(id).hidden = !h[i]; });
  $('gTurnsRow').hidden = !ROTATING.has(name);
  $('gSnapRow').hidden = name !== 'polygon_tunnel';
  $('gChaseRow').hidden = name !== 'chase_fan';
  $('gSolidRow').hidden = name !== 'wire3d';
  $('gTargetsRow').hidden = name !== 'mirror_targets';
  if (name === 'mirror_targets' && !mtView) reloadTargets();
  $('gCrossRow').hidden = name !== 'scissor';
  // Slow rotations (1/64 turn per beat) need any value; the ceiling's
  // ripple (0.01 to 0.03) needs hundredths.
  $('gA').step = ROTATING.has(name) ? 'any' : '0.01';
}

export { GEN_DEFAULTS, GEN_NAMES, execKeyDown, execKeyUp, execReleaseAll, showExecutors, showGenHelp,
  showLayers };
