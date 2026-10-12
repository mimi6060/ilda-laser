// Laser Studio — zones.js (T-435).
// Sections (as in the former index.html script):
//   projection zones (T-012)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { postTestPattern, testPattern } from './cuelist.js';
import { $, apiJson, post, sendLatest } from './core.js';
import { syncFill } from './laser.js';
import { flash } from './workspaces.js';

// ---------- projection zones (T-012) ----------
// The server owns the zones: an edit sends the whole set (debounced) and
// takes back what the server kept (clamped values). The canvas shows the
// zone's geometry on a grid (same maths as projection.rs) and its warp
// corners, which can be dragged.
let proj = null, pzSel = 1, pzTimer = null, pzAlign = null, pzDrag = -1;
const PZ_SLIDERS = [
  ['size_x', 'Taille X', 0.05, 2, 0.01], ['size_y', 'Taille Y', 0.05, 2, 0.01],
  ['pos_x', 'Position X', -1, 1, 0.01], ['pos_y', 'Position Y', -1, 1, 0.01],
  ['rotation_deg', 'Rotation', -180, 180, 1], ['shear_x', 'Cisaillement X', -1, 1, 0.01],
  ['shear_y', 'Cisaillement Y', -1, 1, 0.01], ['keystone_x', 'Trapèze X', -0.9, 0.9, 0.01],
  ['keystone_y', 'Trapèze Y', -0.9, 0.9, 0.01], ['pincushion_x', 'Coussin X', -0.5, 0.5, 0.01],
  ['pincushion_y', 'Coussin Y', -0.5, 0.5, 0.01], ['bow_x', 'Arc X', -0.5, 0.5, 0.01],
  ['bow_y', 'Arc Y', -0.5, 0.5, 0.01],
];
const PZ_SQUARE = [[-1, -1], [1, -1], [1, 1], [-1, 1]];
const pzNeutral = () => ({ size_x: 1, size_y: 1, pos_x: 0, pos_y: 0, rotation_deg: 0, shear_x: 0, shear_y: 0, keystone_x: 0, keystone_y: 0,
  pincushion_x: 0, pincushion_y: 0, bow_x: 0, bow_y: 0, warp: { enabled: false, corners: PZ_SQUARE.map(c => c.slice()) } });
for (const [key, label, min, max, step] of PZ_SLIDERS) {
  const wrap = document.createElement('div');
  const l = document.createElement('label');
  l.htmlFor = 'pz_' + key;
  l.append(label + ' ');
  const v = document.createElement('span'); v.className = 'v'; v.id = 'pz_' + key + 'V';
  l.append(v);
  const r = document.createElement('input');
  Object.assign(r, { type: 'range', id: 'pz_' + key, min, max, step });
  r.addEventListener('input', () => { const z = pzZone(); if (!z) return; z.geometry[key] = +r.value; showProjection(); saveProjection(); });
  r.addEventListener('dblclick', () => { const z = pzZone(); if (!z) return; z.geometry[key] = pzNeutral()[key]; showProjection(); saveProjection(); });
  wrap.append(l, r);
  $('pzSliders').append(wrap);
}
const pzZone = () => proj && (proj.zones.find(z => z.id === pzSel) || proj.zones[0]);
// Same convexity rule as `projection::check_quad`.
function pzQuadOk(c) {
  let sign = 0;
  for (let i = 0; i < 4; i++) {
    const [a, b, d] = [c[i], c[(i + 1) % 4], c[(i + 2) % 4]];
    const cross = (b[0] - a[0]) * (d[1] - b[1]) - (b[1] - a[1]) * (d[0] - b[0]);
    if (Math.abs(cross) < 0.01 || (sign && Math.sign(cross) !== sign)) return false;
    sign = Math.sign(cross);
  }
  return true;
}
// `Placer::apply` of projection.rs, for the drawing only.
function pzPlacer(g) {
  const cl = v => Math.max(-1, Math.min(1, v));
  const [s, c] = [Math.sin(g.rotation_deg * Math.PI / 180), Math.cos(g.rotation_deg * Math.PI / 180)];
  let h = null;
  if (g.warp.enabled && pzQuadOk(g.warp.corners)) {
    const [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = g.warp.corners;
    const sx = x0 - x1 + x2 - x3, sy = y0 - y1 + y2 - y3;
    let gg = 0, hh = 0;
    if (Math.abs(sx) > 1e-12 || Math.abs(sy) > 1e-12) {
      const dx1 = x1 - x2, dx2 = x3 - x2, dy1 = y1 - y2, dy2 = y3 - y2, det = dx1 * dy2 - dx2 * dy1;
      gg = (sx * dy2 - dx2 * sy) / det; hh = (dx1 * sy - sx * dy1) / det;
    }
    h = { a: x1 - x0 + gg * x1, b: x3 - x0 + hh * x3, c: x0, d: y1 - y0 + gg * y1, e: y3 - y0 + hh * y3, f: y0, g: gg, h: hh };
  }
  return (x, y) => {
    [x, y] = [x + g.shear_x * y, y + g.shear_y * x];
    [x, y] = [x * g.size_x, y * g.size_y];
    [x, y] = [x * c - y * s, x * s + y * c];
    [x, y] = [x + g.pos_x, y + g.pos_y];
    [x, y] = [x * (1 + g.keystone_x * y), y * (1 + g.keystone_y * x)];
    [x, y] = [x * (1 + g.pincushion_x * y * y), y * (1 + g.pincushion_y * x * x)];
    [x, y] = [x + g.bow_x * y * y, y + g.bow_y * x * x];
    if (h) {
      const u = (cl(x) + 1) / 2, v = (cl(y) + 1) / 2, w = h.g * u + h.h * v + 1;
      [x, y] = [(h.a * u + h.b * v + h.c) / w, (h.d * u + h.e * v + h.f) / w];
    }
    return [cl(x), cl(y)];
  };
}
const PZ_M = 14;
const pzToPx = (cv, [x, y]) => [PZ_M + (x + 1) / 2 * (cv.width - 2 * PZ_M), PZ_M + (1 - (y + 1) / 2) * (cv.height - 2 * PZ_M)];
const pzFromPx = (cv, px, py) => [Math.max(-1, Math.min(1, (px - PZ_M) / (cv.width - 2 * PZ_M) * 2 - 1)), Math.max(-1, Math.min(1, 1 - (py - PZ_M) / (cv.height - 2 * PZ_M) * 2))];
function drawProjection() {
  const cv = $('pzCanvas'), ctx = cv.getContext('2d'), z = pzZone();
  ctx.clearRect(0, 0, cv.width, cv.height);
  if (!z) return;
  const css = getComputedStyle(document.documentElement), color = n => css.getPropertyValue(n).trim();
  ctx.strokeStyle = color('--line'); ctx.lineWidth = 1;
  const [a, b] = [pzToPx(cv, [-1, 1]), pzToPx(cv, [1, -1])];
  ctx.strokeRect(a[0], a[1], b[0] - a[0], b[1] - a[1]);
  // The field's grid through the zone geometry.
  const place = pzPlacer(z.geometry);
  ctx.strokeStyle = color('--accent'); ctx.globalAlpha = .8;
  for (let i = 0; i <= 8; i++) {
    for (const vertical of [false, true]) {
      ctx.beginPath();
      for (let j = 0; j <= 32; j++) {
        const t = -1 + i / 4, u = -1 + j / 16;
        const [px, py] = pzToPx(cv, place(vertical ? t : u, vertical ? u : t));
        if (j) ctx.lineTo(px, py); else ctx.moveTo(px, py);
      }
      ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;
  const corners = z.geometry.warp.corners;
  ctx.strokeStyle = color('--ws-settings'); ctx.fillStyle = color('--ws-settings');
  ctx.beginPath();
  corners.forEach((c, i) => { const [px, py] = pzToPx(cv, c); if (i) ctx.lineTo(px, py); else ctx.moveTo(px, py); });
  ctx.closePath(); ctx.setLineDash([4, 4]); ctx.stroke(); ctx.setLineDash([]);
  for (const c of corners) { const [px, py] = pzToPx(cv, c); ctx.beginPath(); ctx.arc(px, py, 6, 0, Math.PI * 2); ctx.fill(); }
  cv.classList.toggle('off', !z.geometry.warp.enabled);
}
function showProjection() {
  if (!proj) return;
  const z = pzZone();
  pzSel = z.id;
  const list = $('pzList');
  list.replaceChildren(...proj.zones.map(zone => {
    const b = document.createElement('button');
    b.dataset.zone = zone.id;
    b.setAttribute('role', 'tab');
    b.textContent = `${zone.id} · ${zone.name}`;
    if (proj.default_zones.includes(zone.id)) { const d = document.createElement('span'); d.className = 'def'; d.textContent = '★'; d.title = 'Zone par défaut'; b.append(d); }
    b.classList.toggle('active', zone.id === pzSel);
    b.classList.toggle('off', !zone.enabled);
    b.onclick = () => { pzSel = zone.id; showProjection(); };
    return b;
  }));
  if (document.activeElement !== $('pzName')) $('pzName').value = z.name;
  $('pzEnabled').checked = z.enabled;
  $('pzDefault').checked = proj.default_zones.includes(z.id);
  const sel = $('pzOutput');
  if (z.output && ![...sel.options].some(o => o.value === z.output)) sel.add(new Option(z.output, z.output));
  sel.value = z.output || '';
  for (const [key] of PZ_SLIDERS) {
    const r = $('pz_' + key), v = z.geometry[key];
    if (document.activeElement !== r) r.value = v;
    syncFill(r);
    $('pz_' + key + 'V').textContent = key === 'rotation_deg' ? Math.round(v) + '°' : (+v).toFixed(2);
  }
  $('pzWarp').checked = z.geometry.warp.enabled;
  $('pzCorners').textContent = ['BG', 'BD', 'HD', 'HG'].map((n, i) => `${n} ${z.geometry.warp.corners[i].map(c => c.toFixed(2)).join(' ; ')}`).join('   ');
  $('pzDelete').disabled = proj.zones.length <= 1;
  pzShowGroups();
  $('pzAdd').disabled = proj.zones.length >= proj.max_zones;
  const aligned = pzAlign === z.id;
  $('pzAlign').textContent = aligned ? 'Fin de l\'alignement' : 'Aligner avec la mire';
  $('pzAlignState').textContent = pzAlign ? `Mire à travers la zone ${pzAlign}` : '';
  $('pzAlignState').classList.toggle('on', !!pzAlign);
  drawProjection();
}
function takeProjection(v) {
  proj = { ...v.config, max_zones: v.max_zones };
  pzAlign = v.align;
  showProjection();
}
async function loadProjection() {
  try { takeProjection(await apiJson('GET', '/api/projection')); } catch (e) { console.warn('/api/projection', e); }
}
async function pzSend(path, body) {
  const r = await sendLatest('pz:' + path, 'POST', path, body, { quiet: true });
  if (!r) return true; // replaced by a newer edit (T-484)
  if (r.ok) { $('pzError').textContent = ''; takeProjection(await r.json()); return true; }
  const t = await r.text();
  if (r.status === 423) flash(t); else $('pzError').textContent = t;
  await loadProjection();
  return false;
}
function saveProjection(now = false) {
  clearTimeout(pzTimer);
  const send = () => { const { max_zones, ...cfg } = proj; return pzSend('/api/projection', cfg); };
  if (now) return send();
  pzTimer = setTimeout(send, 80);
}
$('pzName').addEventListener('change', () => { pzZone().name = $('pzName').value; saveProjection(true); });
$('pzEnabled').addEventListener('change', () => { pzZone().enabled = $('pzEnabled').checked; saveProjection(true); });
$('pzDefault').addEventListener('change', () => {
  const id = pzZone().id;
  proj.default_zones = proj.default_zones.filter(d => d !== id);
  if ($('pzDefault').checked) proj.default_zones.push(id);
  saveProjection(true);
});
$('pzOutput').addEventListener('change', () => { pzZone().output = $('pzOutput').value || null; saveProjection(true); });
$('pzWarp').addEventListener('change', () => {
  const g = pzZone().geometry;
  g.warp.enabled = $('pzWarp').checked;
  if (g.warp.enabled && !pzQuadOk(g.warp.corners)) g.warp.corners = PZ_SQUARE.map(c => c.slice());
  saveProjection(true);
});
$('pzResetCorners').addEventListener('click', () => { pzZone().geometry.warp.corners = PZ_SQUARE.map(c => c.slice()); saveProjection(true); });
$('pzResetGeo').addEventListener('click', () => { pzZone().geometry = pzNeutral(); saveProjection(true); });
// Zone groups (T-420): name and zones here; the master's level is only a
// reminder (it moves in LIVE, through `group.<n>.master`). Up to 32 groups
// (T-170), the first 16 with a master.
const PZ_MAX_GROUPS = 32, PZ_MASTER_CONTROLS = 16;
let pzGroupsKey = '';
function pzShowGroups() {
  const groups = proj.groups || (proj.groups = []);
  const key = JSON.stringify([groups, proj.zones.map(z => [z.id, z.name])]);
  $('pzGroupAdd').disabled = groups.length >= PZ_MAX_GROUPS;
  pzShowChasers();
  const typing = document.activeElement?.type === 'text' && $('pzGroups').contains(document.activeElement);
  if (key === pzGroupsKey || typing) return;
  pzGroupsKey = key;
  $('pzGroups').replaceChildren(...groups.map((g, i) => {
    const d = document.createElement('div');
    d.className = 'pz-group';
    d.dataset.group = i + 1;
    d.innerHTML = `<label for="pzGroupName${i}">Nom du groupe ${i + 1}</label>
      <input type="text" id="pzGroupName${i}" maxlength="40" autocomplete="off">
      <div class="gz"></div>
      <div class="row"><span class="gm-level"></span><button class="del">Supprimer le groupe</button></div>`;
    const name = d.querySelector('input');
    name.value = g.name;
    // `proj` is replaced by every server reply: always edit its current copy.
    name.addEventListener('change', () => { proj.groups[i].name = name.value; saveProjection(true); });
    d.querySelector('.gz').replaceChildren(...proj.zones.map(z => {
      const l = document.createElement('label');
      l.className = 'check';
      const c = document.createElement('input');
      c.type = 'checkbox';
      c.dataset.zone = z.id;
      c.checked = g.zones.includes(z.id);
      c.addEventListener('change', () => {
        const cur = proj.groups[i];
        cur.zones = cur.zones.filter(id => id !== z.id);
        if (c.checked) cur.zones.push(z.id);
        saveProjection(true);
      });
      l.append(c, ` ${z.id} · ${z.name}`);
      return l;
    }));
    d.querySelector('.gm-level').textContent = i < PZ_MASTER_CONTROLS ? `Maître : ${Math.round((g.master ?? 1) * 100)} % (LIVE)` : 'Sans maître (16 premiers groupes seulement)';
    d.querySelector('.del').addEventListener('click', () => {
      proj.groups.splice(i, 1);
      // The chasers' steps on it go, the later groups move up (T-170).
      for (const c of proj.chasers || []) c.steps = c.steps.filter(g => g !== i).map(g => g > i ? g - 1 : g);
      saveProjection(true);
    });
    return d;
  }));
}
$('pzGroupAdd').addEventListener('click', () => {
  const groups = proj.groups || (proj.groups = []);
  if (groups.length >= PZ_MAX_GROUPS) return;
  groups.push({ name: `Groupe ${groups.length + 1}`, zones: [], master: 1 });
  saveProjection(true);
});
// Zone chasers (T-170): name, steps (groups in order), step length and
// mode are edits of the zones (`POST /api/projection`: history, locked in
// show mode); running and the speed are playing values (`chaser.<n>.enabled`,
// `chaser.rate` through `/api/control`), allowed in show mode.
const ZC_STEPS = [0.25, 0.5, 1, 2, 4], ZC_STEP_FR = ['1/4 temps', '1/2 temps', '1 temps', '2 temps', '4 temps'];
const ZC_MODES = [['forward', 'Avant'], ['bounce', 'Aller-retour'], ['random', 'Aléatoire']];
const ZC_RATES_FR = ['×1/4', '×1/2', '×1', '×2', '×4'];
const ZC_MAX = 16, ZC_MAX_STEPS = 32;
let pzChasersKey = '';
function zcOptions(sel, pairs, value) {
  sel.replaceChildren(...pairs.map(([v, t]) => new Option(t, String(v))));
  sel.value = String(value);
}
function pzShowChasers() {
  const chasers = proj.chasers || (proj.chasers = []), groups = proj.groups || [];
  $('pzChaserAdd').disabled = chasers.length >= ZC_MAX || !groups.length;
  const rate = $('pzChaseRate');
  if (!rate.options.length) zcOptions(rate, ZC_RATES_FR.map((t, i) => [i, t]), 2);
  if (document.activeElement !== rate) rate.value = String(Math.max(0, [0.25, 0.5, 1, 2, 4].indexOf(proj.chase_rate ?? 1)));
  const key = JSON.stringify([chasers, groups.map(g => g.name)]);
  const typing = document.activeElement?.type === 'text' && $('pzChasers').contains(document.activeElement);
  if (key === pzChasersKey || typing) return;
  pzChasersKey = key;
  const groupOpts = groups.map((g, gi) => [gi, `${gi + 1} · ${g.name}`]);
  $('pzChasers').replaceChildren(...chasers.map((c, i) => {
    const d = document.createElement('div');
    d.className = 'pz-group pz-chaser';
    d.dataset.chaser = i + 1;
    d.innerHTML = `<label for="pzChName${i}">Nom du chenillard ${i + 1}</label>
      <input type="text" id="pzChName${i}" maxlength="40" autocomplete="off">
      <label class="check zc-run"><input type="checkbox" data-control="chaser.${i + 1}.enabled"> En marche (libre)</label>
      <div class="pz-grid">
        <div><label for="pzChStep${i}">Pas</label><select id="pzChStep${i}" class="zc-step"></select></div>
        <div><label for="pzChMode${i}">Mode</label><select id="pzChMode${i}" class="zc-mode"></select></div>
      </div>
      <div class="zc-steps" role="list" aria-label="Étapes du chenillard ${i + 1}"></div>
      <div class="row"><button class="zc-add">Ajouter une étape</button><span class="zc-now" aria-live="off"></span><button class="del">Supprimer le chenillard</button></div>`;
    const name = d.querySelector('input[type=text]');
    name.value = c.name;
    name.addEventListener('change', () => { proj.chasers[i].name = name.value; saveProjection(true); });
    const run = d.querySelector('.zc-run input');
    run.checked = !!c.enabled;
    run.addEventListener('change', () => post('/api/control', { id: `chaser.${i + 1}.enabled`, value: run.checked ? 1 : 0 }));
    const step = d.querySelector('.zc-step');
    zcOptions(step, ZC_STEPS.map((b, k) => [b, ZC_STEP_FR[k]]), c.step_beats ?? 1);
    step.addEventListener('change', () => { proj.chasers[i].step_beats = +step.value; saveProjection(true); });
    const mode = d.querySelector('.zc-mode');
    zcOptions(mode, ZC_MODES, c.mode || 'forward');
    mode.addEventListener('change', () => { proj.chasers[i].mode = mode.value; saveProjection(true); });
    d.querySelector('.zc-steps').replaceChildren(...c.steps.map((g, k) => {
      const row = document.createElement('div');
      row.className = 'zc-stepline';
      row.setAttribute('role', 'listitem');
      row.innerHTML = `<label for="pzChS${i}_${k}">Étape ${k + 1}</label><select id="pzChS${i}_${k}"></select><button class="zc-del" aria-label="Retirer l'étape ${k + 1}" title="Retirer l'étape">×</button>`;
      const sel = row.querySelector('select');
      zcOptions(sel, groupOpts, g);
      sel.addEventListener('change', () => { proj.chasers[i].steps[k] = +sel.value; saveProjection(true); });
      row.querySelector('.zc-del').addEventListener('click', () => { proj.chasers[i].steps.splice(k, 1); saveProjection(true); });
      return row;
    }));
    const add = d.querySelector('.zc-add');
    add.disabled = c.steps.length >= ZC_MAX_STEPS || !groups.length;
    add.addEventListener('click', () => {
      const st = proj.chasers[i].steps;
      st.push(st.length ? (st[st.length - 1] + 1) % groups.length : 0);
      saveProjection(true);
    });
    d.querySelector('.del').addEventListener('click', () => { proj.chasers.splice(i, 1); saveProjection(true); });
    return d;
  }));
}
$('pzChaserAdd').addEventListener('click', () => {
  const chasers = proj.chasers || (proj.chasers = []), groups = proj.groups || [];
  if (chasers.length >= ZC_MAX || !groups.length) return;
  chasers.push({ name: `Chenillard ${chasers.length + 1}`, steps: groups.map((_, gi) => gi).slice(0, ZC_MAX_STEPS), step_beats: 1, mode: 'forward', enabled: false });
  saveProjection(true);
});
$('pzChaseRate').addEventListener('change', () => post('/api/control', { id: 'chaser.rate', value: +$('pzChaseRate').value }));
$('pzAdd').addEventListener('click', () => {
  const id = [1, 2, 3, 4, 5, 6, 7, 8].find(n => !proj.zones.some(z => z.id === n));
  if (!id) return;
  proj.zones.push({ id, name: `Zone ${id}`, enabled: true, output: null, geometry: pzNeutral() });
  pzSel = id;
  saveProjection(true);
});
$('pzDelete').addEventListener('click', () => {
  if (proj.zones.length <= 1) return;
  const id = pzZone().id;
  proj.zones = proj.zones.filter(z => z.id !== id);
  proj.default_zones = proj.default_zones.filter(d => d !== id);
  pzSel = proj.zones[0].id;
  saveProjection(true);
});
$('pzAlign').addEventListener('click', async () => {
  const id = pzZone().id;
  if (pzAlign === id) return pzSend('/api/projection/align', { zone: null });
  if (await pzSend('/api/projection/align', { zone: id }) && !(testPattern && testPattern.active)) postTestPattern({ pattern: 'grid' });
});
// The output list, when the panel opens (no scan: what the studio knows).
$('pzPanel').addEventListener('toggle', async () => {
  if (!$('pzPanel').open) return;
  try {
    const j = await apiJson('GET', '/api/outputs'), sel = $('pzOutput');
    for (const d of j.devices || []) if (![...sel.options].some(o => o.value === d.id)) sel.add(new Option(`${d.name} (${d.id})`, d.id));
    showProjection();
  } catch { /* the select keeps what it has */ }
});
// Warp corners: drag a handle (the warp turns on).
function pzCanvasPos(e) { const cv = $('pzCanvas'), r = cv.getBoundingClientRect(); return [(e.clientX - r.left) * cv.width / r.width, (e.clientY - r.top) * cv.height / r.height]; }
$('pzCanvas').addEventListener('pointerdown', e => {
  const z = pzZone(); if (!z) return;
  const [px, py] = pzCanvasPos(e), cv = $('pzCanvas');
  let best = -1, bestD = 16;
  z.geometry.warp.corners.forEach((c, i) => { const [cx, cy] = pzToPx(cv, c), d = Math.hypot(cx - px, cy - py); if (d < bestD) { best = i; bestD = d; } });
  if (best < 0) return;
  pzDrag = best;
  cv.setPointerCapture(e.pointerId);
});
$('pzCanvas').addEventListener('pointermove', e => {
  if (pzDrag < 0) return;
  const g = pzZone().geometry, cv = $('pzCanvas'), [px, py] = pzCanvasPos(e);
  const corners = g.warp.corners.map(c => c.slice());
  corners[pzDrag] = pzFromPx(cv, px, py).map(v => Math.round(v * 100) / 100);
  if (!pzQuadOk(corners)) { $('pzError').textContent = 'Les 4 coins doivent former un quadrilatère convexe.'; return; }
  $('pzError').textContent = '';
  g.warp.corners = corners;
  g.warp.enabled = true;
  showProjection();
  saveProjection();
});
const pzDrop = () => { pzDrag = -1; };
$('pzCanvas').addEventListener('pointerup', pzDrop);
$('pzCanvas').addEventListener('pointercancel', pzDrop);

export { ZC_RATES_FR, loadProjection, proj, pzToPx, pzZone, showProjection, takeProjection,
  zcOptions };
