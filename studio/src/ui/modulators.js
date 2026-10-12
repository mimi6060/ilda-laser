// Laser Studio — modulators.js (T-435).
// Sections (as in the former index.html script):
//   Étalement entre zones (T-421)
//   LFO modulators
//   FX grid (T-146)
//   audio routes (T-153)
//   audio presets (T-239)
//   audio triggers (T-240)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './desk.js';
import { $, api, apiJson, hex, post, postLatest, rgb } from './core.js';
import { CONFIRM_HOLD_MS, LOCKED_FR, flash, holdToRun, showMode } from './workspaces.js';
import { loadProjection, proj } from './zones.js';
import { control } from './direct.js';
import { SPEED_NAMES } from './tempo.js';
import { presets } from './cues.js';
import { learnMode } from './midi.js';

// ---------- Étalement entre zones (T-421) ----------
// A spread names a zone group (RÉGLAGES › Zones de projection › Groupes):
// each zone of the group gets a phase, a delay or a speed from its rank.
// The same editor serves a modulator (phase only), a cell of the grid and
// a timeline event. It only shifts when a zone shows the look: the safety
// stage, the e-stop and Noir act on every zone at once.
const SPREAD_PARAMS = [['phase', 'Phase'], ['delay', 'Délai'], ['speed', 'Vitesse']];
const SPREAD_DEFAULT = { group: '', param: 'phase', from: 0, to: 360, delay_in_beats: false, wings: 1, blocks: 1 };
const spreadGroups = () => ((proj && proj.groups) || []).filter(g => g.name.trim());
const M64 = (1n << 64n) - 1n;
function spreadMix(z) {
  z = (z + 0x9e3779b97f4a7c15n) & M64;
  z = ((z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n) & M64;
  z = ((z ^ (z >> 27n)) * 0x94d049bb133111ebn) & M64;
  return z ^ (z >> 31n);
}
// The server's `spread::permutation` and `spread::value`, for the chart.
function spreadPerm(seed, n) {
  const o = [...Array(n).keys()];
  let st = BigInt(seed) & M64;
  for (let i = n - 1; i >= 1; i--) {
    st = spreadMix(st);
    const j = Number(st % BigInt(i + 1));
    [o[i], o[j]] = [o[j], o[i]];
  }
  return o;
}
function spreadValue(s, index, count) {
  const from = +s.from || 0, to = +s.to || 0;
  if (count < 2) return from;
  index = Math.min(index, count - 1);
  const pos = s.shuffle != null ? spreadPerm(s.shuffle, count)[index] : index;
  const wings = Math.min(Math.max(s.wings || 1, 1), 16, count);
  const wl = Math.ceil(count / wings), wing = Math.floor(pos / wl), start = wing * wl, end = Math.min(start + wl, count);
  const local = wing % 2 ? end - 1 - pos : pos - start;
  const blocks = Math.min(Math.max(s.blocks || 1, 1), 16);
  const slot = Math.floor(local / blocks), slots = Math.ceil(wl / blocks);
  const frac = (s.param || 'phase') === 'phase' ? slot / slots : slots < 2 ? 0 : slot / (slots - 1);
  return from + (to - from) * frac;
}
const spreadUnit = s => s.param === 'delay' ? (s.delay_in_beats ? ' temps' : ' s') : s.param === 'speed' ? ' %' : '°';
// The editor's fields into `box` (`data-prefix` for the ids, `data-fixed`
// = phase for a modulator). Disabled, with its help, without any group.
function spreadRender(box, spread) {
  const s = { ...SPREAD_DEFAULT, ...(spread || {}) };
  if (box.dataset.fixed) s.param = box.dataset.fixed;
  const p = box.dataset.prefix, groups = spreadGroups();
  box.dataset.seed = s.shuffle != null ? String(s.shuffle) : (box.dataset.seed || String(Math.floor(Math.random() * 2 ** 32)));
  const gOpts = [['', 'Aucun'], ...groups.map(g => [g.name, `${g.name} (${g.zones.length} zone${g.zones.length > 1 ? 's' : ''})`])];
  if (s.group && !groups.some(g => g.name === s.group)) gOpts.push([s.group, `${s.group} (absent)`]);
  const opt = (list, sel) => list.map(([v, t]) => `<option value="${lfoEsc(v)}"${String(v) === String(sel) ? ' selected' : ''}>${lfoEsc(t)}</option>`).join('');
  const step = s.param === 'delay' ? 0.05 : s.param === 'speed' ? 5 : 15;
  box.innerHTML = `<fieldset class="spread-set"${groups.length || s.group ? '' : ' disabled'}><legend>Étalement</legend>
    ${groups.length ? '' : '<p class="hint">Nécessite au moins un groupe de zones (RÉGLAGES › Zones de projection).</p>'}
    <div class="spread-grid">
      ${box.dataset.fixed ? '' : `<label for="${p}Param">Paramètre</label><select id="${p}Param" data-sf="param">${opt(SPREAD_PARAMS, s.param)}</select>`}
      <label for="${p}Group">Groupe</label><select id="${p}Group" data-sf="group" title="Le groupe est une référence : ajouter une zone au groupe change l'étalement sans toucher au réglage">${opt(gOpts, s.group)}</select>
      <label for="${p}From">De (${spreadUnit(s).trim()})</label><input type="number" id="${p}From" data-sf="from" step="${step}" value="${+s.from || 0}">
      <label for="${p}To">À (${spreadUnit(s).trim()})</label><input type="number" id="${p}To" data-sf="to" step="${step}" value="${+s.to || 0}">
      <label for="${p}Wings">Ailes</label><input type="number" id="${p}Wings" data-sf="wings" min="1" max="16" step="1" value="${s.wings || 1}" title="1 = aucune ; 2 = miroir autour du centre ; plus = motif répété">
      <label for="${p}Blocks">Blocs</label><input type="number" id="${p}Blocks" data-sf="blocks" min="1" max="16" step="1" value="${s.blocks || 1}" title="Les zones voisines partagent la même valeur, par paquets">
    </div>
    <div class="row spread-opts">
      ${s.param === 'delay' ? `<label class="check" title="De et À comptés en temps du tempo au lieu de secondes"><input type="checkbox" data-sf="beats"${s.delay_in_beats ? ' checked' : ''}> En temps</label>` : ''}
      <label class="check" title="Ordre des zones permuté, le même à chaque lecture"><input type="checkbox" data-sf="shuffle"${s.shuffle != null ? ' checked' : ''}> Mélange</label>
      <button type="button" data-sf-redraw${s.shuffle != null ? '' : ' disabled'}>Autre tirage</button>
    </div>
    <div class="spread-chart" role="img"></div></fieldset>`;
  spreadChart(box);
}
// The spread the fields describe; null without a group (`draft`: what the
// fields hold all the same, to redraw them).
function spreadRead(box, draft = false) {
  const v = sf => box.querySelector(`[data-sf="${sf}"]`);
  const group = v('group') ? v('group').value : '';
  if (!group && !draft) return null;
  const num = (sf, lo, hi, d) => { const x = parseFloat(v(sf)?.value); return Number.isFinite(x) ? Math.min(hi, Math.max(lo, x)) : d; };
  const s = {
    group,
    param: box.dataset.fixed || (v('param') ? v('param').value : 'phase'),
    from: num('from', -3600, 3600, 0), to: num('to', -3600, 3600, 0),
    delay_in_beats: !!v('beats')?.checked,
    wings: Math.round(num('wings', 1, 16, 1)), blocks: Math.round(num('blocks', 1, 16, 1)),
  };
  if (v('shuffle')?.checked) s.shuffle = +box.dataset.seed;
  return s;
}
// One bar per zone of the group, its value as the height.
function spreadChart(box) {
  const chart = box.querySelector('.spread-chart');
  if (!chart) return;
  const s = spreadRead(box), g = s && spreadGroups().find(x => x.name === s.group);
  const n = g ? g.zones.length : 0;
  if (!s || n < 2) {
    chart.innerHTML = '';
    chart.setAttribute('aria-label', s ? 'Sans effet : le groupe a moins de deux zones' : 'Aucun étalement');
    chart.title = chart.getAttribute('aria-label');
    return;
  }
  const vals = g.zones.map((_, i) => spreadValue(s, i, n));
  const lo = Math.min(0, ...vals), hi = Math.max(...vals, s.param === 'phase' ? 360 : lo + 1e-6);
  const label = g.zones.map((z, i) => `zone ${z} : ${Math.round(vals[i] * 100) / 100}${spreadUnit(s)}`).join(', ');
  chart.setAttribute('aria-label', label);
  chart.title = label;
  chart.innerHTML = vals.map(v => `<span style="height:${Math.max(6, Math.round((v - lo) / (hi - lo) * 100))}%"></span>`).join('');
}
// Edits: the chart follows while typing; `onChange(spread)` on a change.
function spreadWire(box, onChange) {
  box.addEventListener('input', e => { if (e.target.dataset.sf) spreadChart(box); });
  box.addEventListener('change', e => {
    const f = e.target.dataset.sf;
    if (!f) return;
    e.stopPropagation();
    const s = spreadRead(box, true);
    if (f === 'param' || f === 'group' || f === 'beats' || f === 'shuffle') {
      // New units, steps and options; a new parameter starts from its
      // usual range.
      if (f === 'param') { s.from = s.param === 'speed' ? 100 : 0; s.to = s.param === 'phase' ? 360 : s.param === 'delay' ? 1 : 200; }
      spreadRender(box, s);
    } else spreadChart(box);
    onChange(spreadRead(box));
  });
  box.addEventListener('click', e => {
    if (!e.target.matches('[data-sf-redraw]')) return;
    box.dataset.seed = String(Math.floor(Math.random() * 2 ** 32));
    spreadChart(box);
    onChange(spreadRead(box));
  });
}

// ---------- LFO modulators ----------
const LFO_WAVES = [['sine', 'Sinus'], ['triangle', 'Triangle'], ['square', 'Carré'], ['saw_up', 'Dent de scie ↗'],
  ['saw_down', 'Dent de scie ↘'], ['random', 'Aléatoire']];
const LFO_PERIODS = [[0.25, '1/4 temps'], [0.5, '1/2 temps'], [1, '1 temps'], [2, '2 temps'], [4, '4 temps'], [8, '8 temps'], [16, '16 temps']];
let lfos = [], lfoTargets = [], lfoMax = 16, lfoParts = [];
// A part of a figure cue (T-441): its parameters.
const LFO_PART_PARAMS = [['brightness', 'Luminosité'], ['size', 'Taille'], ['pos_x', 'Position X'], ['pos_y', 'Position Y'], ['angle_z', 'Angle']];
const lfoEsc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
const lfoShape = (wave, p) => ({ sine: Math.sin(2 * Math.PI * p), triangle: p < 0.25 ? 4 * p : p < 0.75 ? 2 - 4 * p : 4 * p - 4,
  square: p < 0.5 ? 1 : -1, saw_up: 2 * p - 1, saw_down: 1 - 2 * p, random: 0 })[wave];
// The wave as the server shapes it (`Modulator::wave_shifted`): the whole
// wave within the first `width` of the cycle, attack and decay ramps from
// and back to rest (-1).
function lfoWave(m, p) {
  const base = q => m.wave === 'random' ? (Math.floor(q * 4) % 2 ? 0.6 : -0.4) : lfoShape(m.wave, q);
  const width = Math.min(1, Math.max(0, m.width ?? 1));
  let a = Math.min(1, Math.max(0, m.attack || 0)), d = Math.min(1, Math.max(0, m.decay || 0));
  if (a + d > 1) { const sum = a + d; a /= sum; d /= sum; }
  if (width >= 1 && !a && !d) return base(p);
  if (p >= width) return -1;
  const q = p / width, env = a && q < a ? q / a : d && q > 1 - d ? (1 - q) / d : 1;
  return -1 + (base(q) + 1) * Math.min(1, Math.max(0, env));
}
async function loadLfos() {
  const r = await apiJson('GET', '/api/lfos');
  lfos = r.lfos; lfoTargets = r.targets; lfoMax = r.max; lfoParts = r.parts || [];
  renderLfos();
}
async function saveLfos() {
  if (!await postLatest('lfos', '/api/lfos', lfos)) await loadLfos();
}
function renderLfos() {
  const opts = (list, sel) => list.map(([v, t]) => `<option value="${v}"${String(v) === String(sel) ? ' selected' : ''}>${lfoEsc(t)}</option>`).join('');
  $('lfoList').innerHTML = lfos.map((m, i) => {
    const period = m.rate.beats !== undefined ? m.rate.beats : 'hz';
    // « Partie »: the master (a control) or a part of a figure cue (T-441).
    const partOpts = [['', 'Maître'], ...lfoParts.flatMap(f => f.parts.map(p => [`${f.cue}|${p.id}`, `${f.figure} › ${p.name}`]))];
    const partVal = m.part ? `${m.part.cue}|${m.part.part}` : '';
    if (m.part && !partOpts.some(([v]) => v === partVal)) partOpts.push([partVal, `${m.part.cue.replace(/^figure:/, '')} › partie ${m.part.part} (absente)`]);
    return `<div class="lfo" data-i="${i}">
      <div class="row"><select data-f="part" aria-label="Partie" title="Maître, ou une partie d'un cue de figure">${opts(partOpts, partVal)}</select>
        ${m.part ? `<select data-f="param" aria-label="Paramètre" title="Paramètre de la partie">${opts(LFO_PART_PARAMS, m.part.param)}</select>` : ''}</div>
      <div class="row"><canvas width="144" height="60" data-graph="${i}"></canvas>
        <select data-f="target" title="Cible" aria-label="Cible"${m.part ? ' hidden' : ''}>${opts(lfoTargets.map(t => [t.id, t.label]), m.target)}</select>
        <button class="del" data-del="${i}" title="Supprimer ce modulateur" aria-label="Supprimer ce modulateur">×</button></div>
      <div class="row"><select data-f="wave" title="Forme">${opts(LFO_WAVES, m.wave)}</select>
        <select data-f="period" title="Période">${opts([...LFO_PERIODS, ['hz', 'Hz (libre)']], period)}</select>
        ${period === 'hz' ? `<input type="number" data-f="hz" min="0" max="20" step="0.05" value="${m.rate.hz}" title="Fréquence (Hz)">` : ''}
        <select data-f="speed_group" aria-label="Groupe de vitesse" title="Groupe de vitesse dont ce modulateur compte les temps (LIVE › Direct › Vitesses)"${period === 'hz' ? ' disabled' : ''}>${opts(SPEED_NAMES.map((t, g) => [g, g ? 'Groupe ' + t : 'Tempo maître']), m.speed_group || 0)}</select></div>
      <label>Profondeur <span class="v">${Math.round(m.depth * 100)} %</span></label>
      <input type="range" data-f="depth" min="0" max="100" data-default="50" value="${Math.round(m.depth * 100)}">
      <label>Phase <span class="v">${Math.round(m.phase * 360)}°</span></label>
      <input type="range" data-f="phase" min="0" max="99" data-default="0" data-display-k="3.6" value="${Math.round(m.phase * 100)}">
      <label>Largeur <span class="v">${Math.round((m.width ?? 1) * 100)} %</span></label>
      <input type="range" data-f="width" min="0" max="100" data-default="100" value="${Math.round((m.width ?? 1) * 100)}" title="Part du cycle pendant laquelle le modulateur est actif ; au repos le reste du temps">
      <label>Attaque <span class="v">${Math.round((m.attack || 0) * 100)} %</span></label>
      <input type="range" data-f="attack" min="0" max="100" data-default="0" value="${Math.round((m.attack || 0) * 100)}" title="Part de la partie active consacrée à la montée">
      <label>Déclin <span class="v">${Math.round((m.decay || 0) * 100)} %</span></label>
      <input type="range" data-f="decay" min="0" max="100" data-default="0" value="${Math.round((m.decay || 0) * 100)}" title="Part de la partie active consacrée à la descente">
      <label class="check u-justify-start"><input type="checkbox" data-f="enabled"${m.enabled ? ' checked' : ''}> Actif</label>
      ${m.part ? '' : `<div class="spread" data-prefix="lfoSp${i}" data-fixed="phase" data-i="${i}" title="Décale la phase de ce modulateur d'une zone à l'autre (contrôles du maître)"></div>`}
    </div>`;
  }).join('');
  document.querySelectorAll('#lfoList .spread').forEach(box => {
    spreadRender(box, lfos[+box.dataset.i].spread);
    spreadWire(box, sp => { const m = lfos[+box.dataset.i]; if (sp) m.spread = sp; else delete m.spread; saveLfos(); });
  });
  if (!proj && lfos.length) loadProjection().then(() => { if (proj && spreadGroups().length) renderLfos(); });
  $('lfoAdd').disabled = lfos.length >= lfoMax;
  $('lfoHint').style.display = lfos.length ? 'none' : '';
}
$('lfoHintAdd').onclick = () => $('lfoAdd').click(); // the empty state's first action (T-437)
$('lfoAdd').addEventListener('click', () => {
  if (lfos.length >= lfoMax) return;
  lfos.push({ target: 'master.size', wave: 'sine', rate: { beats: 4 }, depth: 0.5, phase: 0, offset: 0, enabled: true, speed_group: 0 });
  renderLfos(); saveLfos();
});
$('lfoList').addEventListener('click', e => {
  const del = e.target.dataset.del;
  if (del === undefined) return;
  lfos.splice(+del, 1); renderLfos(); saveLfos();
});
function lfoEdit(e, final) {
  const f = e.target.dataset.f, box = e.target.closest('.lfo');
  if (!f || !box) return;
  const m = lfos[+box.dataset.i], v = e.target.value;
  if (f === 'target' || f === 'wave') m[f] = v;
  else if (f === 'part') {
    if (!v) delete m.part;
    else { const at = v.lastIndexOf('|'); m.part = { cue: v.slice(0, at), part: +v.slice(at + 1), param: (m.part && m.part.param) || 'brightness' }; }
  }
  else if (f === 'param' && m.part) m.part.param = v;
  else if (f === 'period') m.rate = v === 'hz' ? { hz: 1 } : { beats: +v };
  else if (f === 'hz') m.rate = { hz: Math.min(20, Math.max(0, +v || 0)) };
  else if (f === 'depth' || f === 'phase') {
    m[f] = +v / 100;
    e.target.previousElementSibling.querySelector('.v').textContent = f === 'depth' ? Math.round(m.depth * 100) + ' %' : Math.round(m.phase * 360) + '°';
  }
  else if (f === 'width' || f === 'attack' || f === 'decay') {
    m[f] = +v / 100;
    // Attack and decay share the active part (at most 100 % together).
    if (f !== 'width' && (m.attack || 0) + (m.decay || 0) > 1) m[f === 'attack' ? 'decay' : 'attack'] = Math.max(0, 1 - m[f]);
    box.querySelectorAll('[data-f="width"], [data-f="attack"], [data-f="decay"]').forEach(r => {
      const k = r.dataset.f, x = Math.round((m[k] ?? (k === 'width' ? 1 : 0)) * 100);
      if (r !== e.target) r.value = x;
      r.previousElementSibling.querySelector('.v').textContent = x + ' %';
    });
  }
  else if (f === 'enabled') m.enabled = e.target.checked;
  else if (f === 'speed_group') m.speed_group = +v;
  if (final && (f === 'period' || f === 'target' || f === 'part')) renderLfos();
  saveLfos();
}
// Sliders and the Hz field send while moving; menus and the checkbox on change.
$('lfoList').addEventListener('input', e => { if (e.target.type === 'range' || e.target.type === 'number') lfoEdit(e, false); });
$('lfoList').addEventListener('change', e => { if (e.target.type !== 'range' && e.target.type !== 'number') lfoEdit(e, true); });
// Mini graph: one cycle of the wave, and a dot where the modulator is now.
function showLfos(pos) {
  if (!pos) return;
  document.querySelectorAll('#lfoList canvas').forEach(c => {
    const i = +c.dataset.graph, m = lfos[i], at = pos[i];
    if (!m || !at) return;
    const g = c.getContext('2d'), w = c.width, h = c.height, y = v => h / 2 - v * (h / 2 - 5);
    g.clearRect(0, 0, w, h);
    g.strokeStyle = m.enabled ? '#3ddc84' : '#8b929c'; g.lineWidth = 2; g.beginPath();
    for (let x = 0; x <= w; x++) {
      const p = x / w, v = lfoWave(m, p);
      x ? g.lineTo(x, y(v)) : g.moveTo(x, y(v));
    }
    g.stroke();
    g.fillStyle = '#fff'; g.beginPath(); g.arc(at.phase * w, y(at.value), 4, 0, 2 * Math.PI); g.fill();
  });
}

// ---------- FX grid (T-146) ----------
// 4 lines × 8 cells played on top of the cues through `fx.<ligne>.<case>`
// (down / up, like a cue pad); editing a cell posts the whole grid to
// /api/fx (an edit: in the history, locked in show mode).
const FG = { cells: [], targets: [], starter: [], max: 8, state: null, edit: false, sel: null, key: '' };
async function loadFxGrid() {
  const r = await apiJson('GET', '/api/fx');
  FG.cells = r.cells; FG.targets = r.targets; FG.starter = r.starter; FG.max = r.max_modulators;
  FG.key = ''; renderFxGrid(); showFxGrid(r.state);
  if (FG.sel) fgFillEditor();
}
async function saveFxGrid() {
  if (!await post('/api/fx', { cells: FG.cells })) await loadFxGrid();
  else renderFxGrid();
}
function renderFxGrid() {
  const st = FG.state;
  $('fgGrid').innerHTML = FG.cells.map((line, l) => {
    const act = st && st.lines[l] ? st.lines[l].action : 1;
    return `<div class="fg-row" role="row"><span class="n">${l + 1}</span>${line.map((c, i) =>
      `<button class="fg-cell${c ? '' : ' empty'}" role="gridcell" data-l="${l}" data-c="${i}" data-control="fx.${l + 1}.${i + 1}"
        title="fx.${l + 1}.${i + 1}${c ? ' : ' + lfoEsc(c.name) + (c.one_shot ? ' (ponctuel)' : '') : ' : case vide'}">${c ? (c.one_shot ? '⚡ ' : '') + lfoEsc(c.name || '(sans nom)') : '—'}</button>`).join('')}
      <div class="fg-act" role="gridcell" title="fx.${l + 1}.action : 0 = pas d'effet, 100 = plein effet"><label>Action</label>
        <input type="range" min="0" max="100" value="${Math.round(act * 100)}" data-act="${l}" data-control="fx.${l + 1}.action"><span class="v">${Math.round(act * 100)} %</span></div></div>`;
  }).join('');
  FG.key = '';
  paintFxGrid();
}
// What plays, from /api/frame and the pushed state.
function showFxGrid(st) {
  if (!st) return;
  FG.state = st;
  const key = JSON.stringify(st);
  if (key === FG.key) return;
  FG.key = key;
  document.querySelectorAll('#fgDest button').forEach(b => b.classList.toggle('active', ['master', 'cue'][+b.dataset.v] === st.dest));
  document.querySelectorAll('#fgClick button').forEach(b => b.classList.toggle('active', ['toggle', 'flash'][+b.dataset.v] === st.click));
  paintFxGrid();
}
function paintFxGrid() {
  const st = FG.state;
  if (!st) return;
  document.querySelectorAll('#fgGrid .fg-cell').forEach(b => {
    const line = st.lines[+b.dataset.l] || {}, on = line.active === +b.dataset.c;
    b.classList.toggle('on', on && !line.waiting);
    b.classList.toggle('wait', on && !!line.waiting);
    b.style.setProperty('--lvl', on ? (line.level ?? 1).toFixed(2) : 0);
    b.classList.toggle('sel', !!FG.sel && FG.sel[0] === +b.dataset.l && FG.sel[1] === +b.dataset.c);
  });
  document.querySelectorAll('#fgGrid [data-act]').forEach(r => {
    const line = st.lines[+r.dataset.act];
    if (!line || document.activeElement === r) return;
    r.value = Math.round(line.action * 100);
    r.nextElementSibling.textContent = Math.round(line.action * 100) + ' %';
  });
}
// A cell plays while the pointer is down (Flash) or toggles (Bascule);
// in « Modifier » mode it is picked for the editor instead.
const fgHeld = new Map();
function fgRelease(pointer) {
  const id = fgHeld.get(pointer);
  if (!id) return;
  fgHeld.delete(pointer);
  control(id, 0);
}
$('fgGrid').addEventListener('pointerdown', e => {
  const b = e.target.closest('.fg-cell');
  if (!b || e.button !== 0 || learnMode) return;
  if (FG.edit) { FG.sel = [+b.dataset.l, +b.dataset.c]; fgFillEditor(); paintFxGrid(); return; }
  try { b.setPointerCapture(e.pointerId); } catch { /* synthetic pointer */ }
  fgHeld.set(e.pointerId, b.dataset.control);
  control(b.dataset.control, 1);
});
['pointerup', 'pointercancel', 'lostpointercapture'].forEach(ev => $('fgGrid').addEventListener(ev, e => fgRelease(e.pointerId)));
$('fgGrid').addEventListener('input', e => {
  const r = e.target.closest('[data-act]');
  if (!r) return;
  r.nextElementSibling.textContent = r.value + ' %';
  control(r.dataset.control, r.value / 100);
});
$('fgStop').addEventListener('click', () => control('fx.stop', 1));
document.querySelectorAll('#fgDest button').forEach(b => b.addEventListener('click', () => control('fx.dest', +b.dataset.v)));
document.querySelectorAll('#fgClick button').forEach(b => b.addEventListener('click', () => control('fx.mode', +b.dataset.v)));
$('fgEditMode').addEventListener('click', () => {
  if (showMode) return flash(LOCKED_FR);
  FG.edit = !FG.edit;
  $('fgEditMode').setAttribute('aria-pressed', String(FG.edit));
  $('fgPanel').classList.toggle('editing', FG.edit);
  if (!FG.edit) { FG.sel = null; $('fgEditor').hidden = true; paintFxGrid(); }
});
$('fgEditClose').addEventListener('click', () => { FG.sel = null; $('fgEditor').hidden = true; paintFxGrid(); });
// The editor: the selected cell, created on the first change.
const fgCell = () => FG.sel && FG.cells[FG.sel[0]][FG.sel[1]];
function fgTouch(f) {
  if (!FG.sel) return;
  const [l, c] = FG.sel;
  const cell = FG.cells[l][c] || { name: `Effet ${l + 1}.${c + 1}`, modulators: [], modifiers: null, length: { beats: 4 }, one_shot: false };
  f(cell);
  FG.cells[l][c] = cell;
  saveFxGrid().then(fgFillEditor);
}
const fgMods = cell => cell.modifiers || (cell.modifiers = {});
function fgFillEditor() {
  const c = fgCell();
  $('fgEditor').hidden = !FG.sel;
  if (!FG.sel) return;
  $('fgEditPos').textContent = `${FG.sel[0] + 1}.${FG.sel[1] + 1}`;
  const m = (c && c.modifiers) || {};
  if (document.activeElement !== $('fgName')) $('fgName').value = c ? c.name : '';
  $('fgOneShot').checked = !!(c && c.one_shot);
  $('fgLength').value = String(c && c.length && c.length.beats !== undefined ? c.length.beats : 4);
  const size = Math.round((m.size ?? 1) * 100), bright = Math.round((m.brightness ?? 1) * 100);
  const rot = m.rot_speed ? m.rot_speed[2] : 0;
  $('fgSize').value = size; $('fgSizeV').textContent = size + ' %';
  $('fgBright').value = bright; $('fgBrightV').textContent = bright + ' %';
  $('fgRot').value = rot; $('fgRotV').textContent = `${rot} tour(s) / mesure`;
  $('fgStrobe').value = m.strobe && m.strobe.on && m.strobe.rate && m.strobe.rate.beats !== undefined ? String(m.strobe.rate.beats) : '0';
  $('fgMirror').value = m.mirror || 'none';
  $('fgPrism').value = String(m.prism || 1);
  $('fgColor').value = (m.color && m.color.kind) || 'normal';
  if (m.color_params && m.color_params.rgb) $('fgRgb').value = hex(m.color_params.rgb);
  $('fgRgb').hidden = $('fgColor').value !== 'fixed';
  const opts = (list, sel) => list.map(([v, t]) => `<option value="${v}"${String(v) === String(sel) ? ' selected' : ''}>${lfoEsc(t)}</option>`).join('');
  const mods = c ? c.modulators : [];
  $('fgMods').innerHTML = mods.map((md, i) => `<div class="fg-mod" data-i="${i}">
      <div class="row"><select data-f="target" title="Cible">${opts(FG.targets.map(t => [t.id, t.label]), md.target)}</select>
        <button class="del" data-del="${i}" title="Supprimer ce modulateur" aria-label="Supprimer ce modulateur">×</button></div>
      <div class="row"><select data-f="wave" title="Forme">${opts(LFO_WAVES, md.wave)}</select>
        <select data-f="period" title="Période">${opts(LFO_PERIODS, md.rate && md.rate.beats !== undefined ? md.rate.beats : 4)}</select></div>
      <label>Profondeur <span class="v">${Math.round(md.depth * 100)} %</span></label>
      <input type="range" data-f="depth" min="0" max="100" data-default="100" value="${Math.round(md.depth * 100)}"></div>`).join('');
  $('fgModAdd').disabled = mods.length >= FG.max;
}
$('fgName').addEventListener('change', () => fgTouch(c => { c.name = $('fgName').value.trim(); }));
$('fgOneShot').addEventListener('change', () => fgTouch(c => { c.one_shot = $('fgOneShot').checked; }));
$('fgLength').addEventListener('change', () => fgTouch(c => { c.length = { beats: +$('fgLength').value }; }));
$('fgSize').addEventListener('input', () => { $('fgSizeV').textContent = $('fgSize').value + ' %'; });
$('fgSize').addEventListener('change', () => fgTouch(c => { fgMods(c).size = $('fgSize').value / 100; }));
$('fgBright').addEventListener('input', () => { $('fgBrightV').textContent = $('fgBright').value + ' %'; });
$('fgBright').addEventListener('change', () => fgTouch(c => { fgMods(c).brightness = $('fgBright').value / 100; }));
$('fgRot').addEventListener('input', () => { $('fgRotV').textContent = `${$('fgRot').value} tour(s) / mesure`; });
$('fgRot').addEventListener('change', () => fgTouch(c => { const m = fgMods(c); m.rot_speed = [0, 0, +$('fgRot').value]; m.rot_sync = true; }));
$('fgStrobe').addEventListener('change', () => fgTouch(c => {
  const b = +$('fgStrobe').value;
  fgMods(c).strobe = b ? { on: true, hold: false, rate: { beats: b }, duty: 0.5 } : { on: false, hold: false, rate: { hz: 8 }, duty: 0.5 };
}));
$('fgMirror').addEventListener('change', () => fgTouch(c => { fgMods(c).mirror = $('fgMirror').value; }));
$('fgPrism').addEventListener('change', () => fgTouch(c => { fgMods(c).prism = +$('fgPrism').value; }));
function fgSetColor(c) {
  const m = fgMods(c), kind = $('fgColor').value, rgbv = rgb($('fgRgb').value);
  m.color_params = { ...(m.color_params || {}), rgb: rgbv, spread: 1, rainbow_rate: { beats: 4 }, chase_step: { beats: 0.25 } };
  m.color = { normal: { kind: 'normal' }, fixed: { kind: 'fixed', rgb: rgbv }, rainbow: { kind: 'rainbow', spread: 1, rate: { beats: 4 } },
    chase: { kind: 'chase', palette: 0, step: { beats: 0.25 }, spread: 'stroke' } }[kind];
}
$('fgColor').addEventListener('change', () => fgTouch(fgSetColor));
$('fgRgb').addEventListener('change', () => fgTouch(fgSetColor));
$('fgModAdd').addEventListener('click', () => fgTouch(c => {
  if (c.modulators.length < FG.max) c.modulators.push({ target: 'master.size', wave: 'sine', rate: { beats: 4 }, depth: 0.2, phase: 0, offset: 0, enabled: true });
}));
$('fgMods').addEventListener('click', e => {
  const del = e.target.dataset.del;
  if (del !== undefined) fgTouch(c => { c.modulators.splice(+del, 1); });
});
$('fgMods').addEventListener('input', e => {
  if (e.target.dataset.f === 'depth') e.target.previousElementSibling.querySelector('.v').textContent = e.target.value + ' %';
});
$('fgMods').addEventListener('change', e => {
  const f = e.target.dataset.f, box = e.target.closest('.fg-mod');
  if (!f || !box) return;
  fgTouch(c => {
    const md = c.modulators[+box.dataset.i];
    if (!md) return;
    if (f === 'target' || f === 'wave') md[f] = e.target.value;
    else if (f === 'period') md.rate = { beats: +e.target.value };
    else if (f === 'depth') md.depth = e.target.value / 100;
  });
});
holdToRun($('fgClear'), CONFIRM_HOLD_MS, () => {
  if (!FG.sel) return;
  FG.cells[FG.sel[0]][FG.sel[1]] = null;
  saveFxGrid().then(fgFillEditor);
}, () => flash('Maintenir pour vider la case'));
holdToRun($('fgStarter'), CONFIRM_HOLD_MS, () => {
  FG.cells = JSON.parse(JSON.stringify(FG.starter));
  saveFxGrid().then(fgFillEditor);
}, () => flash('Maintenir pour remettre les effets de départ'));

// ---------- audio routes (T-153) ----------
// A source (band, level, kick…) → gate, curve, attack/release → a control.
const AR_CURVES = [['linear', 'Linéaire'], ['square', 'Carré (plus de punch)'], ['sqrt', 'Racine (plus sensible)'], ['s_curve', 'En S (doux)']];
let aroutes = [], arMix = 0.5, arSources = [], arTargets = [], arMax = 16, arFlicker = [];
const arEvent = id => arSources.some(s => s.id === id && s.event);
async function loadRoutes() {
  const r = await apiJson('GET', '/api/audio/routes');
  aroutes = r.routes; arMix = r.mix; arSources = r.sources; arTargets = r.targets; arMax = r.max; arFlicker = r.flicker ?? [];
  $('musPreset').value = r.preset ?? '';
  renderRoutes();
}
async function saveRoutes() {
  // An edit by hand: the routes are no preset's any more (T-239).
  $('musPreset').value = ''; arFlicker = [];
  if (!await postLatest('routes', '/api/audio/routes', { mix: arMix, routes: aroutes })) await loadRoutes();
}
function arSpan(r, f, label, title) {
  const sp = r.shape[f], beats = sp.beats !== undefined, v = beats ? sp.beats : sp.ms;
  return `<label title="${title}">${label}</label><div class="row u-mt0">
    <input type="number" data-f="${f}" min="0" max="${beats ? 16 : 10000}" step="${beats ? 0.125 : 5}" value="${+v.toFixed(3)}" title="${title}">
    <select data-u="${f}" title="Unité"><option value="ms"${beats ? '' : ' selected'}>ms</option><option value="beats"${beats ? ' selected' : ''}>temps</option></select></div>`;
}
function renderRoutes() {
  const opts = (list, sel) => list.map(([v, t]) => `<option value="${v}"${String(v) === String(sel) ? ' selected' : ''}>${lfoEsc(t)}</option>`).join('');
  const srcOpts = sel => `<optgroup label="Continu">${opts(arSources.filter(s => !s.event).map(s => [s.id, s.label]), sel)}</optgroup>`
    + `<optgroup label="Évènements">${opts(arSources.filter(s => s.event).map(s => [s.id, s.label]), sel)}</optgroup>`;
  $('arMix').value = Math.round(arMix * 100);
  $('arMixV').textContent = Math.round(arMix * 100) + ' %';
  $('arList').innerHTML = aroutes.map((r, i) => {
    const ev = arEvent(r.source), amt = Math.round(r.shape.max * 100);
    return `<div class="lfo aroute" data-i="${i}">
      <div class="row"><div class="meters" title="Entrée / sortie"><div class="meter"><div data-in="${i}"></div></div><div class="meter out"><div data-out="${i}"></div></div></div>
        <select data-f="source" title="Source">${srcOpts(r.source)}</select>
        <button class="del" data-del="${i}" title="Supprimer ce lien audio">×</button></div>
      <div class="row"><select data-f="target" title="Cible">${opts(arTargets.map(t => [t.id, t.label]), r.target)}</select></div>
      <label>Quantité <span class="v">${amt} %</span></label>
      <input type="range" data-f="amount" min="-100" max="100" data-default="100" value="${amt}" title="Part de la plage du réglage (négatif : vers le bas)">
      <label>Seuil <span class="v">${Math.round(r.shape.gate * 100)} %</span></label>
      <input type="range" data-f="gate" min="0" max="100" data-default="5" value="${Math.round(r.shape.gate * 100)}" title="En dessous, le signal est ignoré (bruit)">
      <label>Courbe</label><select data-f="curve">${opts(AR_CURVES, r.shape.curve)}</select>
      ${arSpan(r, 'attack', 'Attaque', 'Temps de montée')}
      ${ev ? arSpan(r, 'decay', 'Déclin', 'Temps de retombée après un coup') : arSpan(r, 'release', 'Relâche', 'Temps de retombée')}
      <label class="check u-justify-start"><input type="checkbox" data-f="enabled"${r.enabled ? ' checked' : ''}> Actif</label>
      ${arFlicker[i] ? '<div class="route-flicker">Aigus sur la luminosité : risque de scintillement.</div>' : ''}
    </div>`;
  }).join('');
  $('arAdd').disabled = aroutes.length >= arMax;
  $('arHint').style.display = aroutes.length ? 'none' : '';
}
$('arHintAdd').onclick = () => $('arAdd').click();
$('arAdd').addEventListener('click', () => {
  if (aroutes.length >= arMax) return;
  aroutes.push({ source: 'bass', target: 'master.size', enabled: true, shape: { gate: 0.05, hysteresis: 0.02, gain: 1, curve: 'linear',
    attack: { ms: 10 }, release: { ms: 150 }, decay: { ms: 120 }, min: 0, max: 0.5 } });
  renderRoutes(); saveRoutes();
});
$('arList').addEventListener('click', e => {
  const del = e.target.dataset.del;
  if (del === undefined) return;
  aroutes.splice(+del, 1); renderRoutes(); saveRoutes();
});
$('arMix').addEventListener('input', () => {
  arMix = +$('arMix').value / 100;
  $('arMixV').textContent = Math.round(arMix * 100) + ' %';
  saveRoutes();
});
function arEdit(e, final) {
  const box = e.target.closest('.aroute');
  if (!box) return;
  const r = aroutes[+box.dataset.i], f = e.target.dataset.f, u = e.target.dataset.u, v = e.target.value;
  if (u) {
    const n = box.querySelector(`input[data-f="${u}"]`);
    r.shape[u] = v === 'beats' ? { beats: 0.25 } : { ms: 100 };
    if (n) n.value = v === 'beats' ? 0.25 : 100;
    renderRoutes();
  } else if (f === 'source' || f === 'target') {
    r[f] = v;
    if (final) renderRoutes();
  } else if (f === 'amount' || f === 'gate') {
    if (f === 'amount') r.shape.max = +v / 100; else r.shape.gate = +v / 100;
    e.target.previousElementSibling.querySelector('.v').textContent = v + ' %';
  } else if (f === 'curve') r.shape.curve = v;
  else if (f === 'attack' || f === 'release' || f === 'decay') {
    const beats = r.shape[f].beats !== undefined, n = Math.max(0, Math.min(beats ? 16 : 10000, +v || 0));
    r.shape[f] = beats ? { beats: n } : { ms: n };
  } else if (f === 'enabled') r.enabled = e.target.checked;
  else return;
  saveRoutes();
}
$('arList').addEventListener('input', e => { if (e.target.type === 'range' || e.target.type === 'number') arEdit(e, false); });
$('arList').addEventListener('change', e => { if (e.target.type !== 'range' && e.target.type !== 'number') arEdit(e, true); });
// Live meters: input (continuous sources) and shaped output, from /api/frame.
function showRoutes(meters) {
  if (!meters) return;
  meters.forEach((m, i) => {
    const inp = document.querySelector(`#arList [data-in="${i}"]`), out = document.querySelector(`#arList [data-out="${i}"]`);
    if (!inp || !out) return;
    inp.style.width = Math.round((m.input ?? Math.abs(m.value)) * 100) + '%';
    inp.parentElement.classList.toggle('shut', m.input !== null && !m.open);
    out.style.width = Math.round(Math.min(1, Math.abs(m.value)) * 100) + '%';
    out.dataset.v = m.value.toFixed(3);
  });
}

// ---------- audio presets (T-239) ----------
// One click replaces the audio routes by a ready-made set.
async function loadAudioPresets() {
  const r = await apiJson('GET', '/api/audio/presets');
  const sel = $('musPreset');
  sel.innerHTML = '<option value="">Personnalisé</option>' + r.presets.map(p => `<option value="${lfoEsc(p.id)}" title="${lfoEsc(p.hint)}">${lfoEsc(p.name)}</option>`).join('');
  sel.value = r.current ?? '';
}
$('musPreset').addEventListener('change', async () => {
  const id = $('musPreset').value, hint = $('musPresetHint');
  hint.textContent = ''; hint.classList.remove('warn');
  if (!id) return; // « Personnalisé » : rien à appliquer
  const r = await api('POST', '/api/audio/preset', { id }, { quiet: true });
  if (!r.ok) { const t = await r.text(); if (r.status === 423) flash(t); hint.textContent = t; hint.classList.add('warn'); }
  else {
    const v = await r.json();
    if (v.warnings?.length) { hint.textContent = v.warnings.join(' · '); hint.classList.add('warn'); }
    else hint.textContent = `Préréglage « ${$('musPreset').selectedOptions[0].textContent} » appliqué : ${v.routes.length} lien(s) audio.`;
  }
  await loadRoutes();
});

// ---------- audio triggers (T-240) ----------
// Rules « Quand … Alors … », the switch (audio.triggers.enabled), counts.
const TRG_EVENTS = [['kick', 'Kick'], ['snare', 'Caisse claire'], ['drop', 'Drop'], ['break', 'Break'], ['buildup_above', 'Montée ≥'], ['silence', 'Silence'], ['every_bars', 'Toutes les N mesures']];
const TRG_ACTIONS = [['launch_cue', 'Lancer un cue'], ['next_cue', 'Cue suivant de la page'], ['flash_cue', 'Flash d\'un cue'], ['black_hold', 'Noir bref']];
const TRG_QUANT = [['off', 'Tout de suite'], ['beat', 'Au prochain temps'], ['bar', 'À la prochaine mesure']];
let triggers = [], trgMax = 16, trgFired = [];
const trgKind = v => typeof v === 'string' ? v : Object.keys(v)[0];
async function loadTriggers() {
  const r = await apiJson('GET', '/api/audio/triggers');
  triggers = r.triggers; trgMax = r.max;
  renderTriggers(); showTriggers(r.status);
}
async function saveTriggers() {
  const r = await api('POST', '/api/audio/triggers', { triggers }, { quiet: true });
  if (!r.ok) { const t = await r.text(); flash(t); await loadTriggers(); return; }
  const v = await r.json();
  triggers = v.triggers; renderTriggers();
}
function renderTriggers() {
  const opts = (list, sel) => list.map(([v, t]) => `<option value="${lfoEsc(v)}"${String(v) === String(sel) ? ' selected' : ''}>${lfoEsc(t)}</option>`).join('');
  const cues = presets.map(p => [p.id, p.name]);
  $('trgList').innerHTML = triggers.map((t, i) => {
    const ev = trgKind(t.event), act = trgKind(t.action);
    const evVal = ev === 'buildup_above' ? `<input type="number" data-trg="evval" min="5" max="100" step="5" value="${Math.round(t.event.buildup_above * 100)}" title="Seuil de montée (%)">`
      : ev === 'every_bars' ? `<input type="number" data-trg="evval" min="1" max="64" step="1" value="${t.event.every_bars}" title="Nombre de mesures">` : '';
    const cue = act === 'launch_cue' ? t.action.launch_cue : act === 'flash_cue' ? t.action.flash_cue.id : null;
    const beats = act === 'flash_cue' ? t.action.flash_cue.beats : act === 'black_hold' ? t.action.black_hold.beats : null;
    const fired = trgFired[i]?.fired ?? 0;
    return `<div class="trg${t.enabled ? '' : ' off'}" data-i="${i}">
      <div class="row"><span>Quand</span><select data-trg="event" title="Évènement de la musique">${opts(TRG_EVENTS, ev)}</select>${evVal}
        <b class="count" data-count="${i}" title="Actions faites / limitées">${fired}</b>
        <button class="del lock-edit" data-del="${i}" title="Supprimer cette règle">×</button></div>
      <div class="row"><span>Alors</span><select data-trg="action" title="Action">${opts(TRG_ACTIONS, act)}</select>${beats !== null ? `<input type="number" data-trg="beats" min="0.25" max="${act === 'black_hold' ? 4 : 16}" step="0.25" value="${beats}" title="Durée en temps">` : ''}</div>
      ${cue !== null ? `<div class="row"><span>Cue</span><select data-trg="cue" title="Cue">${cue ? '' : '<option value="" selected>Choisir un cue…</option>'}${opts(cues, cue)}</select></div>` : ''}
      <div class="row"><span>Intervalle</span><input type="number" data-trg="interval" min="0.25" max="64" step="0.25" value="${t.min_interval_beats}" title="Intervalle min. : au plus une action par ce nombre de temps"><select data-trg="quantize" title="Quand agir">${opts(TRG_QUANT, t.quantize)}</select></div>
      <label class="check u-justify-start"><input type="checkbox" data-trg="enabled"${t.enabled ? ' checked' : ''}> Active</label>
    </div>`;
  }).join('');
  $('trgAdd').disabled = triggers.length >= trgMax;
}
function trgEvent(kind, old) {
  if (kind === 'buildup_above') return { buildup_above: old?.buildup_above ?? 0.6 };
  if (kind === 'every_bars') return { every_bars: old?.every_bars ?? 4 };
  return kind;
}
function trgAction(kind, cue, beats) {
  if (kind === 'launch_cue') return { launch_cue: cue ?? '' };
  if (kind === 'flash_cue') return { flash_cue: { id: cue ?? '', beats: beats ?? 1 } };
  if (kind === 'black_hold') return { black_hold: { beats: Math.min(4, beats ?? 0.5) } };
  return kind;
}
const trgCue = a => a.launch_cue ?? a.flash_cue?.id;
const trgBeats = a => a.flash_cue?.beats ?? a.black_hold?.beats;
$('trgAdd').addEventListener('click', () => {
  if (triggers.length >= trgMax) return;
  // A rule with a cue to choose is saved once the cue is chosen.
  triggers.push({ event: 'drop', action: 'next_cue', min_interval_beats: 1, quantize: 'off', enabled: true });
  renderTriggers(); saveTriggers();
});
$('trgList').addEventListener('click', e => {
  const del = e.target.dataset.del;
  if (del === undefined) return;
  triggers.splice(+del, 1); renderTriggers(); saveTriggers();
});
$('trgList').addEventListener('change', e => {
  const box = e.target.closest('.trg');
  if (!box) return;
  const t = triggers[+box.dataset.i], f = e.target.dataset.trg, v = e.target.value;
  if (f === 'event') t.event = trgEvent(v, null);
  else if (f === 'evval') t.event = typeof t.event === 'object' && 'buildup_above' in t.event ? { buildup_above: Math.max(5, Math.min(100, +v || 60)) / 100 } : { every_bars: Math.max(1, Math.min(64, Math.round(+v) || 4)) };
  else if (f === 'action') t.action = trgAction(v, trgCue(t.action), trgBeats(t.action));
  else if (f === 'cue') t.action = trgAction(trgKind(t.action), v, trgBeats(t.action));
  else if (f === 'beats') t.action = trgAction(trgKind(t.action), trgCue(t.action), Math.max(0.25, +v || 1));
  else if (f === 'interval') t.min_interval_beats = Math.max(0.25, Math.min(64, +v || 1));
  else if (f === 'quantize') t.quantize = v;
  else if (f === 'enabled') t.enabled = e.target.checked;
  else return;
  renderTriggers();
  // Not before its cue is chosen (the server refuses an empty one).
  if (trgCue(t.action) === '') return;
  saveTriggers();
});
// Frames fetched before the switch's answer don't undo the click.
let trgOnSentAt = 0, trgOnAnsweredAt = 0;
$('trgOn').addEventListener('change', async () => {
  const on = $('trgOn').checked;
  trgOnSentAt = performance.now();
  if (!await post('/api/control', { id: 'audio.triggers.enabled', value: on ? 1 : 0 })) {
    $('trgOn').checked = !on;
    flash('Automatisme audio impossible : arrêt d\'urgence enclenché ?');
  }
  trgOnAnsweredAt = performance.now();
});
// Every frame: the switch (it may move from MIDI or an e-stop), whether
// the rules run, the black, each rule's counts.
function showTriggers(st) {
  if (!st) return;
  if (trgOnAnsweredAt >= trgOnSentAt) $('trgOn').checked = st.enabled;
  const el = $('trgState');
  el.dataset.run = String(st.live); el.dataset.black = String(st.black);
  el.textContent = !st.enabled ? (triggers.length ? 'Automatisme coupé : aucune règle n\'agit.' : '')
    : st.black ? 'Noir bref en cours.'
    : st.live ? 'Automatisme actif : les règles suivent la musique.' : 'Automatisme en attente : pas d\'audio.';
  trgFired = st.rules ?? [];
  trgFired.forEach((r, i) => {
    const c = document.querySelector(`#trgList [data-count="${i}"]`);
    if (!c) return;
    c.textContent = r.limited ? `${r.fired} / ${r.limited}` : String(r.fired);
    c.classList.toggle('hit', r.ago_s !== null && r.ago_s < 0.3);
  });
}

export { LFO_PERIODS, LFO_WAVES, lfoEsc, loadAudioPresets, loadFxGrid, loadLfos, loadRoutes,
  loadTriggers, showFxGrid, showLfos, showRoutes, showTriggers, spreadGroups, spreadRead,
  spreadRender, spreadWire };
