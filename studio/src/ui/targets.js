// Laser Studio — targets.js (T-435).
// Sections (as in the former index.html script):
//   Cibles miroir (T-109)
//   Suivi de cible PosiStageNet (T-457)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { outdoorState, safety, sfDrawing, sfGradPick } from './safety.js';
import { $, api, calibration, editLook, genEdit, settings } from './core.js';
import { draw, g } from './presence.js';
import { lastPoints, lastShown, show2d } from './view3d.js';

// ---------- Cibles miroir (T-109) ----------
// The machine's targets (GET /api/targets): each one's output point and
// what the safety stage does there. Adding or moving one asks for the
// confirmation (409 → « Confirmer »); aiming = the next click on the 2D
// preview moves it (through the inverse of the calibration).
let mtView = null, mtPending = null, mtAiming = null;
async function reloadTargets() {
  const r = await api('GET', '/api/targets', undefined, { quiet: true });
  if (!r.ok) return;
  mtView = await r.json();
  showTargets();
}
function mtItems() { return mtView ? mtView.items.map(t => ({ ...t })) : []; }
/** The rows' values; a coordinate that is not a number keeps the stored one. */
function mtFromControls() {
  const items = mtItems();
  $('mtList').querySelectorAll('.mtrow').forEach(row => {
    const t = items[+row.dataset.i];
    if (!t) return;
    const num = (cls, old) => { const v = parseFloat(row.querySelector(cls).value); return Number.isFinite(v) ? v : old; };
    t.name = row.querySelector('.mtname').value.trim() || t.name;
    t.x = num('.mtx', t.x); t.y = num('.mty', t.y);
  });
  return items;
}
function showTargets() {
  const v = mtView;
  if (!v) return;
  const el = (tag, props = {}) => Object.assign(document.createElement(tag), props);
  $('mtAdd').disabled = v.items.length >= v.max;
  if (!v.items.length) $('mtList').replaceChildren(el('div', { className: 'hint', textContent: 'Aucune cible. « Ajouter une cible », puis « Viser » dans l’aperçu.' }));
  else $('mtList').replaceChildren(...v.items.map((t, i) => {
    const row = el('div', { className: 'mtrow' + (mtAiming === i ? ' aiming' : '') });
    row.dataset.i = i;
    const field = (label, input) => {
      const f = el('div', { className: 'mtfield' });
      f.append(el('label', { htmlFor: input.id, textContent: label }), input);
      return f;
    };
    const st = v.status[i] || { reasons: [], level: 1 };
    const cut = st.level <= 0, dimmed = st.level < 1 || st.guard != null;
    const msg = st.reasons.length ? st.reasons.join(' · ') : 'Rien sur le trajet : allumée si un cue la vise.';
    const guard = st.guard != null ? ` · garde anti-point fixe : ${Math.round(st.guard * 100)} %` : '';
    row.append(
      el('span', { className: 'mtnum', textContent: String(i + 1) }),
      field('Nom', el('input', { type: 'text', id: `mtName${i}`, className: 'mtname', maxLength: 40, value: t.name, autocomplete: 'off' })),
      field('X', el('input', { type: 'number', id: `mtX${i}`, className: 'mtx', min: -1, max: 1, step: 0.01, value: t.x.toFixed(3) })),
      field('Y', el('input', { type: 'number', id: `mtY${i}`, className: 'mty', min: -1, max: 1, step: 0.01, value: t.y.toFixed(3) })),
      el('button', { className: 'mtaim', textContent: mtAiming === i ? 'Visée…' : 'Viser', ariaLabel: `Viser la cible ${i + 1} dans l’aperçu` }),
      el('button', { className: 'mtdel', textContent: 'Supprimer', ariaLabel: `Supprimer la cible ${i + 1}` }),
      el('div', { className: 'hint mtstatus' + (cut ? ' sf-error' : dimmed ? ' sf-warn' : ''), textContent: (cut ? 'Coupée : ' : dimmed ? 'Atténuée : ' : '') + msg + guard }),
    );
    return row;
  }));
  $('mtAimHint').hidden = mtAiming == null;
  if (settings && settings.content && settings.content.generator === 'mirror_targets') showTargetPicker(settings.content.params.a);
  if (lastPoints && show2d) draw(lastShown);
}
/** The look's target checkboxes (bit i = target i + 1). */
function showTargetPicker(a) {
  const mask = Number.isFinite(+a) && a > 0 ? Math.round(a) : 0;
  if (document.activeElement && document.activeElement.closest && document.activeElement.closest('#gTargets')) return;
  const el = (tag, props = {}) => Object.assign(document.createElement(tag), props);
  // The mobile targets (T-457) come after the fixed ones (16 bits at most).
  const all = mtView ? [...mtView.items.map(t => ({ name: t.name })), ...(mtView.mobile || []).map(m => ({ name: `${m.name} (mobile)` }))].slice(0, 16) : [];
  if (!all.length) { $('gTargets').replaceChildren(el('div', { className: 'hint', textContent: 'Aucune cible réglée (RÉGLAGES › Cibles miroir).' })); return; }
  $('gTargets').replaceChildren(...all.map((t, i) => {
    const label = el('label', { className: 'check' });
    const box = el('input', { type: 'checkbox', checked: !!(mask & (1 << i)) });
    box.dataset.bit = i;
    label.append(box, el('span', { textContent: `${i + 1}. ${t.name}` }));
    return label;
  }));
}
$('gTargets').addEventListener('change', e => {
  if (!e.target.matches('input[type=checkbox]')) return;
  let mask = 0;
  $('gTargets').querySelectorAll('input[type=checkbox]').forEach(b => { if (b.checked) mask |= 1 << +b.dataset.bit; });
  editLook(genEdit(c => { c.params.a = mask; }));
});
async function mtSave(items, confirm = false) {
  const body = confirm ? { items, confirm_loosen: true } : { items };
  const r = await api('POST', '/api/targets', body, { quiet: true, ignore: [409] });
  if (r.ok) {
    mtView = await r.json(); mtPending = null;
    $('mtLoosen').hidden = true; $('mtError').textContent = '';
    showTargets();
    return;
  }
  const text = await r.text();
  let j = null;
  try { j = JSON.parse(text); } catch { /* plain text */ }
  if (r.status === 409 && j && !j.armed) {
    mtPending = items;
    $('mtLoosenText').textContent = 'Assouplir la sécurité ? ' + j.loosen.join(' · ');
    $('mtLoosen').hidden = false;
    return;
  }
  await reloadTargets();
  $('mtError').textContent = (j && j.error) || text;
}
$('mtPanel').addEventListener('toggle', () => { if ($('mtPanel').open) reloadTargets(); });
$('mtList').addEventListener('change', e => { if (e.target.matches('input')) mtSave(mtFromControls()); });
$('mtList').addEventListener('click', e => {
  const row = e.target.closest('.mtrow');
  if (!row) return;
  const i = +row.dataset.i;
  if (e.target.matches('.mtdel')) {
    const items = mtFromControls();
    items.splice(i, 1);
    if (mtAiming != null) mtAiming = null;
    mtSave(items);
  } else if (e.target.matches('.mtaim')) {
    mtAiming = mtAiming === i ? null : i;
    showTargets();
  }
});
$('mtAdd').addEventListener('click', () => {
  const items = mtFromControls();
  items.push({ name: `Cible ${items.length + 1}`, x: 0, y: 0.5 });
  mtSave(items);
});
$('mtAimCancel').addEventListener('click', () => { mtAiming = null; showTargets(); });
$('mtLoosenOk').addEventListener('click', () => { if (mtPending) mtSave(mtPending, true); });
$('mtLoosenCancel').addEventListener('click', () => { mtPending = null; $('mtLoosen').hidden = true; reloadTargets(); });
/** A preview point (output coordinates) back through the calibration. */
function uncalibrate(x, y) {
  const c = calibration || {};
  const sx = c.scale_x || 1, sy = c.scale_y || 1, a = -(c.rotation_deg || 0) * Math.PI / 180;
  const u = (x - (c.offset_x || 0)) / sx, v = (y - (c.offset_y || 0)) / sy;
  const clamp = n => Math.round(Math.min(1, Math.max(-1, n)) * 1000) / 1000;
  return [clamp(u * Math.cos(a) - v * Math.sin(a)), clamp(u * Math.sin(a) + v * Math.cos(a))];
}
$('preview').addEventListener('click', e => {
  if (mtAiming == null || sfDrawing || sfGradPick) return;
  const r = $('preview').getBoundingClientRect();
  const [x, y] = uncalibrate((e.clientX - r.left) / r.width * 2 - 1, 1 - (e.clientY - r.top) / r.height * 2);
  const items = mtFromControls(), i = mtAiming;
  mtAiming = null;
  if (!items[i]) return showTargets();
  items[i].x = x; items[i].y = y;
  mtSave(items);
});
/** The targets on the 2D preview, while their panel is open, one is being
 * aimed or the look lights them: a ring and the number, red when cut. */
function drawTargetsOverlay(px) {
  const lookOn = settings && settings.content && settings.content.generator === 'mirror_targets';
  if (!mtView || !mtView.items.length || !($('mtPanel').open || mtAiming != null || lookOn)) return;
  g.save(); g.lineWidth = 2; g.font = '600 22px system-ui, sans-serif'; g.textBaseline = 'bottom';
  mtView.items.forEach((t, i) => {
    const st = mtView.status[i];
    if (!st) return;
    const [x, y] = px(st.out), cut = st.level <= 0;
    g.strokeStyle = g.fillStyle = cut ? 'rgba(255,77,79,0.95)' : mtAiming === i ? '#ffb020' : 'rgba(61,220,132,0.95)';
    g.beginPath(); g.arc(x, y, 14, 0, Math.PI * 2); g.stroke();
    g.beginPath(); g.moveTo(x - 20, y); g.lineTo(x + 20, y); g.moveTo(x, y - 20); g.lineTo(x, y + 20); g.stroke();
    g.fillText(String(i + 1), x + 16, y - 10);
  });
  g.restore();
}
// ---------- Suivi de cible PosiStageNet (T-457) ----------
// GET /api/psn: settings, links and their calibration; GET /api/psn/trackers
// (polled while the panel is open or the look lights targets): the trackers
// seen and each mobile target's state. Every change posts the whole
// settings; a loosening asks for the confirmation (409 → « Confirmer »).
// Calibrating a link: for each reference point the operator places the
// tracker, then clicks the preview where the laser must go; the tracker's
// position at that moment and the click make one point.
let psnView = null, psnStatus = null, psnPending = null, psnCal = null;
const psnStates = { 'suivi': 'Suivi', 'perdu': 'Perdu', 'hors zone': 'Hors zone', 'non calibré': 'Non calibré', 'saut': 'Saut', 'entrées fermées': 'Entrées fermées', 'non affecté': 'Non affecté', 'invalide': 'Invalide' };
const psnPct = v => (Math.round(v * 10) / 10).toFixed(1).replace('.', ',');
async function reloadPsn() {
  const r = await api('GET', '/api/psn', undefined, { quiet: true });
  if (!r.ok) return;
  psnView = await r.json();
  psnStatus = psnView.status;
  showPsn();
}
async function pollPsn() {
  const r = await api('GET', '/api/psn/trackers', undefined, { quiet: true });
  if (!r.ok) return;
  psnStatus = await r.json();
  showPsnTrackers();
  showPsnLinks(false);
  if (lastPoints && show2d) draw(lastShown);
}
let psnLoaded = false;
setInterval(() => {
  const lookOn = settings && settings.content && settings.content.generator === 'mirror_targets';
  if (document.hidden || !($('psnPanel').open || lookOn)) return;
  // The look lights mobile targets: the settings once, then the states.
  if (!psnLoaded) { psnLoaded = true; reloadPsn(); return; }
  if (psnView && psnView.settings.enabled) pollPsn();
}, 250);
function psnSettings() { return psnView ? JSON.parse(JSON.stringify(psnView.settings)) : null; }
/** The settings with the panel's fields; a field that is not a number keeps the stored value. */
function psnFromControls() {
  const s = psnSettings();
  if (!s) return null;
  const num = (id, old) => { const v = parseInt($(id).value, 10); return Number.isFinite(v) ? v : old; };
  s.enabled = $('psnEnabled').checked;
  s.interface = $('psnIface').value.trim() || null;
  s.group = $('psnGroup').value.trim() || s.group;
  s.port = num('psnPort', s.port);
  s.smoothing_ms = num('psnSmooth', s.smoothing_ms);
  s.lost_after_ms = num('psnLost', s.lost_after_ms);
  $('psnLinks').querySelectorAll('.psnrow').forEach(row => {
    const l = s.links[+row.dataset.i];
    if (!l) return;
    l.target = row.querySelector('.psnname').value.trim() || l.target;
    const id = parseInt(row.querySelector('.psnid').value, 10);
    if (Number.isFinite(id) && id >= 0 && id <= 65535) l.tracker_id = id;
    l.axes = [row.querySelector('.psnax0').value, row.querySelector('.psnax1').value];
    const sp = parseFloat(row.querySelector('.psnspeed').value);
    if (Number.isFinite(sp)) l.max_speed = sp;
  });
  return s;
}
function showPsn() {
  const v = psnView;
  if (!v) return;
  const s = v.settings, focus = document.activeElement;
  const put = (id, val) => { if (focus !== $(id)) $(id).value = val; };
  $('psnEnabled').checked = s.enabled;
  put('psnIface', s.interface || '');
  put('psnGroup', s.group);
  put('psnPort', s.port);
  put('psnSmooth', s.smoothing_ms);
  put('psnLost', s.lost_after_ms);
  showPsnTrackers();
  showPsnLinks(true);
}
function showPsnTrackers() {
  const st = psnStatus, el = (tag, props = {}) => Object.assign(document.createElement(tag), props);
  if (!st || !psnView) return;
  const l = st.listen || {};
  $('psnListen').textContent = !st.enabled ? 'Suivi désactivé : aucune écoute.'
    : l.error ? `Erreur : ${l.error}`
    : (l.local ? `Écoute sur ${l.local}` : 'Ouverture de l’écoute…') + ` · ${l.packets || 0} paquet(s), ${l.rejected || 0} rejeté(s)` + (st.gate ? ` · ${st.gate}` : '');
  const fmt = n => (Math.round(n * 100) / 100).toFixed(2);
  const rows = (st.trackers || []).map(t => {
    const tr = el('tr');
    tr.dataset.id = t.id;
    const linked = psnView.settings.links.some(k => k.tracker_id === t.id);
    const btn = el('button', { className: 'psnassign', textContent: 'Affecter à une cible…', disabled: linked || psnView.settings.links.length >= psnView.max_links, ariaLabel: `Affecter le traceur ${t.id} à une cible` });
    tr.append(el('td', { textContent: String(t.id) }), el('td', { textContent: t.name || '—' }), el('td', { textContent: `${fmt(t.pos[0])} ; ${fmt(t.pos[1])} ; ${fmt(t.pos[2])} m · ${t.age_ms} ms` }),
      el('td', { textContent: psnStates[t.state] || t.state }), el('td'));
    tr.lastChild.append(btn);
    return tr;
  });
  if (!rows.length) { const tr = el('tr'); tr.append(el('td', { colSpan: 5, className: 'hint', textContent: st.enabled ? 'Aucun traceur reçu.' : 'Activez le suivi pour recevoir les traceurs.' })); rows.push(tr); }
  $('psnTrackerRows').replaceChildren(...rows);
}
/** The links (mobile targets). `full`: rebuild the inputs too (not while typing). */
function showPsnLinks(full) {
  const v = psnView, el = (tag, props = {}) => Object.assign(document.createElement(tag), props);
  if (!v) return;
  const links = v.settings.links, statuses = (psnStatus && psnStatus.links) || [];
  const stateText = i => {
    const st = statuses[i], cal = v.calibration[i] || {};
    const calText = cal.ok ? `Erreur moyenne : ${psnPct(cal.error_pct)} %` : `Non calibré : ${cal.why || ''}`;
    if (!st) return [calText, !cal.ok];
    const state = psnStates[st.state] || st.state;
    const why = st.reasons && st.reasons.length ? ` (${st.reasons.join(' · ')})` : '';
    return [`${state}${why} · ${calText}`, !st.lit];
  };
  if (!full && $('psnLinks').contains(document.activeElement)) return;
  if (!full) {
    $('psnLinks').querySelectorAll('.psnrow').forEach(row => {
      const [text, dark] = stateText(+row.dataset.i), box = row.querySelector('.psnstate');
      box.textContent = text; box.className = 'hint psnstate mtstatus' + (dark ? ' sf-error' : '');
    });
    return;
  }
  const axes = [['x', 'X'], ['y', 'Y'], ['z', 'Z'], ['neg_x', '−X'], ['neg_y', '−Y'], ['neg_z', '−Z']];
  const sel = (cls, id, val, label) => {
    const s = el('select', { className: cls, id });
    axes.forEach(([k, t]) => s.append(el('option', { value: k, textContent: t, selected: k === val })));
    s.setAttribute('aria-label', label);
    return s;
  };
  if (!links.length) { $('psnLinks').replaceChildren(el('div', { className: 'hint', textContent: 'Aucune cible mobile. « Affecter à une cible… » sur un traceur, puis « Calibrer ».' })); return; }
  $('psnLinks').replaceChildren(...links.map((l, i) => {
    const row = el('div', { className: 'mtrow psnrow' + (psnCal && psnCal.i === i ? ' calibrating' : '') });
    row.dataset.i = i;
    const field = (label, input) => { const f = el('div', { className: 'mtfield' }); f.append(el('label', { htmlFor: input.id, textContent: label }), input); return f; };
    const [text, dark] = stateText(i);
    row.append(
      el('span', { className: 'mtnum', textContent: `M${i + 1}` }),
      field('Cible mobile', el('input', { type: 'text', id: `psnName${i}`, className: 'psnname', maxLength: 40, value: l.target, autocomplete: 'off' })),
      field('Traceur', el('input', { type: 'number', id: `psnId${i}`, className: 'psnid', min: 0, max: 65535, step: 1, value: l.tracker_id })),
      field('Axe horizontal', sel('psnax0', `psnAx0${i}`, l.axes[0], 'Axe horizontal')),
      field('Axe vertical', sel('psnax1', `psnAx1${i}`, l.axes[1], 'Axe vertical')),
      field('Vitesse max (/s)', el('input', { type: 'number', id: `psnSpeed${i}`, className: 'psnspeed', min: 0.1, max: 20, step: 0.1, value: l.max_speed })),
      el('button', { className: 'psncal', textContent: psnCal && psnCal.i === i ? 'Calibrage…' : 'Calibrer', ariaLabel: `Calibrer la cible mobile ${i + 1}` }),
      el('button', { className: 'psndel', textContent: 'Supprimer', ariaLabel: `Supprimer la cible mobile ${i + 1}` }),
      el('div', { className: 'hint psnstate mtstatus' + (dark ? ' sf-error' : ''), textContent: text }),
    );
    return row;
  }));
  showPsnCal();
}
function showPsnCal() {
  $('psnCalHint').hidden = !psnCal;
  if (!psnCal) return;
  const n = psnCal.refs.length;
  $('psnCalText').textContent = `Placez le traceur au point ${n + 1} puis visez-le dans l’aperçu 2D (${n} point(s) relevé(s), 4 au moins).`;
  $('psnCalDone').disabled = n < 4;
}
async function psnSave(settingsBody, confirm = false) {
  const body = confirm ? { ...settingsBody, confirm_loosen: true } : settingsBody;
  const r = await api('POST', '/api/psn', body, { quiet: true, ignore: [409] });
  if (r.ok) {
    psnView = await r.json(); psnStatus = psnView.status; psnPending = null;
    $('psnLoosen').hidden = true; $('psnError').textContent = '';
    showPsn();
    reloadTargets();
    return true;
  }
  const text = await r.text();
  let j = null;
  try { j = JSON.parse(text); } catch { /* plain text */ }
  if (r.status === 409 && j && !j.armed) {
    psnPending = settingsBody;
    $('psnLoosenText').textContent = 'Assouplir la sécurité ? ' + j.loosen.join(' · ');
    $('psnLoosen').hidden = false;
    return false;
  }
  await reloadPsn();
  $('psnError').textContent = (j && j.error) || text;
  return false;
}
$('psnPanel').addEventListener('toggle', () => { if ($('psnPanel').open) reloadPsn(); });
['psnEnabled', 'psnIface', 'psnGroup', 'psnPort', 'psnSmooth', 'psnLost'].forEach(id => $(id).addEventListener('change', () => { const s = psnFromControls(); if (s) psnSave(s); }));
$('psnLinks').addEventListener('change', e => { if (e.target.matches('input, select')) { const s = psnFromControls(); if (s) psnSave(s); } });
$('psnLinks').addEventListener('click', e => {
  const row = e.target.closest('.psnrow');
  if (!row) return;
  const i = +row.dataset.i;
  if (e.target.matches('.psndel')) {
    const s = psnFromControls();
    s.links.splice(i, 1);
    psnCal = null;
    psnSave(s);
  } else if (e.target.matches('.psncal')) {
    psnCal = psnCal && psnCal.i === i ? null : { i, refs: [] };
    showPsnLinks(true);
  }
});
$('psnTrackerRows').addEventListener('click', e => {
  if (!e.target.matches('.psnassign')) return;
  const id = +e.target.closest('tr').dataset.id, s = psnFromControls();
  const t = ((psnStatus && psnStatus.trackers) || []).find(k => k.id === id);
  let name = (t && t.name) || `Cible mobile ${s.links.length + 1}`;
  while (s.links.some(l => l.target === name)) name += ' bis';
  s.links.push({ tracker_id: id, target: name, axes: ['x', 'y'], refs: [], max_speed: 4 });
  psnSave(s);
});
$('psnCalCancel').addEventListener('click', () => { psnCal = null; showPsnLinks(true); });
$('psnCalDone').addEventListener('click', async () => {
  if (!psnCal || psnCal.refs.length < 4) return;
  const s = psnFromControls(), i = psnCal.i;
  if (!s.links[i]) { psnCal = null; return showPsnLinks(true); }
  s.links[i].refs = psnCal.refs;
  psnCal = null;
  psnSave(s);
});
$('psnLoosenOk').addEventListener('click', () => { if (psnPending) psnSave(psnPending, true); });
$('psnLoosenCancel').addEventListener('click', () => { psnPending = null; $('psnLoosen').hidden = true; reloadPsn(); });
// A calibration point: the tracker's current position and the click.
$('preview').addEventListener('click', e => {
  if (!psnCal || mtAiming != null || sfDrawing || sfGradPick || !psnView) return;
  const link = psnView.settings.links[psnCal.i];
  const t = link && ((psnStatus && psnStatus.trackers) || []).find(k => k.id === link.tracker_id);
  if (!t || !t.valid) { $('psnError').textContent = 'Aucune position valide reçue pour ce traceur : placez-le, puis visez.'; return; }
  const r = $('preview').getBoundingClientRect();
  const [x, y] = uncalibrate((e.clientX - r.left) / r.width * 2 - 1, 1 - (e.clientY - r.top) / r.height * 2);
  psnCal.refs.push({ tracker: [...t.pos], output: [x, y] });
  $('psnError').textContent = '';
  showPsnCal();
});
/** A cross per mobile target on the 2D preview (output coordinates), red
 * when the tracking or the safety stage keeps it dark. */
function drawPsnOverlay(px) {
  const lookOn = settings && settings.content && settings.content.generator === 'mirror_targets';
  if (!psnStatus || !psnStatus.enabled || !($('psnPanel').open || lookOn)) return;
  g.save(); g.lineWidth = 2; g.font = '600 20px system-ui, sans-serif'; g.textBaseline = 'bottom';
  (psnStatus.links || []).forEach((l, i) => {
    if (!l.out) return;
    const [x, y] = px(l.out);
    g.strokeStyle = g.fillStyle = l.lit ? 'rgba(61,220,132,0.95)' : 'rgba(255,77,79,0.95)';
    g.beginPath(); g.moveTo(x - 14, y - 14); g.lineTo(x + 14, y + 14); g.moveTo(x - 14, y + 14); g.lineTo(x + 14, y - 14); g.stroke();
    g.fillText(`M${i + 1}`, x + 16, y - 10);
  });
  g.restore();
}
/** Zones and horizon over the 2D preview (translucent red), under the laser. */
function drawSafetyOverlay(W, H, px) {
  if (!safety || !$('sfOverlay').checked) return;
  const h = safety.horizon, yh = (1 - (h.y + 1) / 2) * H;
  if (h.lines) {
    g.fillStyle = 'rgba(255,60,60,0.16)'; g.fillRect(0, yh, W, H - yh);
    const top = (1 - (Math.min(1, h.y + h.ramp) + 1) / 2) * H;
    if (yh > top) {
      const grad = g.createLinearGradient(0, yh, 0, top);
      grad.addColorStop(0, 'rgba(255,60,60,0.16)'); grad.addColorStop(1, 'rgba(255,60,60,0)');
      g.fillStyle = grad; g.fillRect(0, top, W, yh - top);
    }
  }
  g.save(); g.setLineDash([10, 8]); g.strokeStyle = 'rgba(255,77,79,0.55)'; g.lineWidth = 2;
  g.beginPath(); g.moveTo(0, yh); g.lineTo(W, yh); g.stroke(); g.restore();
  const poly = (pts, fill, stroke) => {
    if (pts.length < 2) return;
    g.beginPath(); pts.forEach((p, i) => { const [x, y] = px(p); i ? g.lineTo(x, y) : g.moveTo(x, y); }); g.closePath();
    if (fill) { g.fillStyle = fill; g.fill(); }
    g.strokeStyle = stroke; g.lineWidth = 2; g.stroke();
  };
  // Public zones (T-255) in orange: blanked unless audience scanning is on.
  const scanning = !!(audienceMode && audienceMode.scanning);
  for (const z of safety.zones) {
    const dim = z.kind === 'dim' && (!z.audience || scanning);
    poly(z.points, dim ? 'rgba(255,120,60,0.16)' : 'rgba(255,40,40,0.28)', z.audience ? 'rgba(255,176,32,0.95)' : 'rgba(255,77,79,0.9)');
    if (dim) drawSoftZone(z, px);
  }
  // Outdoor mode (T-262): what the declared angles and « Ciel coupé »
  // cut, hatched in blue; the rest of the field tinted green.
  const cut = outdoorState && outdoorState.cut;
  if (cut && (outdoorState.enabled || outdoorState.sky_off)) {
    if (outdoorState.enabled) { g.fillStyle = 'rgba(60,220,120,0.08)'; g.fillRect(0, 0, W, H); }
    for (const r of cut) {
      const [x0, y0] = px(r[3]), [x1, y1] = px(r[1]);
      g.save(); g.beginPath(); g.rect(x0, y0, x1 - x0, y1 - y0); g.clip();
      g.fillStyle = 'rgba(47,123,255,0.14)'; g.fillRect(x0, y0, x1 - x0, y1 - y0);
      g.strokeStyle = 'rgba(47,123,255,0.5)'; g.lineWidth = 1; g.beginPath();
      for (let d = -(y1 - y0); d < x1 - x0; d += 10) { g.moveTo(x0 + d, y1); g.lineTo(x0 + d + (y1 - y0), y0); }
      g.stroke(); g.restore();
    }
  }
  // Cells the static beam guard is dimming (orange, stronger = darker).
  if (dwellStatus && dwellStatus.cells.length) {
    const step = 2 / dwellStatus.grid;
    for (const [cx, cy, k] of dwellStatus.cells) {
      const [x0, y0] = px([-1 + cx * step, -1 + (cy + 1) * step]), [x1, y1] = px([-1 + (cx + 1) * step, -1 + cy * step]);
      g.fillStyle = `rgba(255,150,40,${(0.25 + 0.5 * (1 - k)).toFixed(2)})`;
      g.fillRect(x0, y0, x1 - x0, y1 - y0);
    }
  }
  if (sfDrawing) {
    poly(sfDrawing.points, null, '#ffb020');
    g.fillStyle = '#ffb020';
    for (const p of sfDrawing.points) { const [x, y] = px(p); g.fillRect(x - 4, y - 4, 8, 8); }
  }
}
/** A Dim zone's soft edge (a faint band and a dotted outer contour) and its
 * gradient (an arrow from « Départ » to « Arrivée » with both levels), T-446. */
function drawSoftZone(z, px) {
  const pts = z.points;
  if (z.feather > 0 && pts.length >= 3) {
    const w = Math.abs(px([z.feather, 0])[0] - px([0, 0])[0]);
    const path = () => { g.beginPath(); pts.forEach((p, i) => { const [x, y] = px(p); i ? g.lineTo(x, y) : g.moveTo(x, y); }); g.closePath(); };
    g.save();
    // The band: a wide stroke, clipped to outside the polygon.
    g.beginPath(); g.rect(-1e4, -1e4, 2e4, 2e4);
    pts.forEach((p, i) => { const [x, y] = px(p); i ? g.lineTo(x, y) : g.moveTo(x, y); }); g.closePath();
    g.clip('evenodd');
    path(); g.lineJoin = 'round'; g.lineWidth = 2 * w; g.strokeStyle = 'rgba(255,120,60,0.10)'; g.stroke();
    g.restore();
    // The outer limit: each vertex pushed out along its corner's bisector.
    const area = pts.reduce((s, p, i) => { const q = pts[(i + 1) % pts.length]; return s + p[0] * q[1] - q[0] * p[1]; }, 0);
    const out = area > 0 ? 1 : -1;
    const norm = (a, b) => { const dx = b[0] - a[0], dy = b[1] - a[1], l = Math.hypot(dx, dy) || 1; return [out * dy / l, -out * dx / l]; };
    const outer = pts.map((p, i) => {
      const n1 = norm(pts[(i + pts.length - 1) % pts.length], p), n2 = norm(p, pts[(i + 1) % pts.length]);
      // Miter: (n1 + n2) / (1 + n1·n2) is at distance 1 from both edges;
      // capped at 3 for sharp corners (the drawing only).
      const d = 1 + n1[0] * n2[0] + n1[1] * n2[1];
      let mx = (n1[0] + n2[0]) / Math.max(d, 1e-6), my = (n1[1] + n2[1]) / Math.max(d, 1e-6);
      const m = Math.hypot(mx, my);
      if (m > 3) { mx *= 3 / m; my *= 3 / m; }
      return [p[0] + mx * z.feather, p[1] + my * z.feather];
    });
    g.save(); g.setLineDash([4, 5]); g.strokeStyle = 'rgba(255,150,80,0.8)'; g.lineWidth = 1.5;
    g.beginPath(); outer.forEach((p, i) => { const [x, y] = px(p); i ? g.lineTo(x, y) : g.moveTo(x, y); }); g.closePath(); g.stroke();
    g.restore();
  }
  const gr = z.gradient;
  if (gr) {
    const [x0, y0] = px(gr.from), [x1, y1] = px(gr.to);
    const a = Math.atan2(y1 - y0, x1 - x0);
    g.save(); g.strokeStyle = g.fillStyle = 'rgba(255,200,120,0.95)'; g.lineWidth = 2;
    g.beginPath(); g.moveTo(x0, y0); g.lineTo(x1, y1); g.stroke();
    g.beginPath(); g.moveTo(x1, y1);
    g.lineTo(x1 - 10 * Math.cos(a - 0.4), y1 - 10 * Math.sin(a - 0.4));
    g.lineTo(x1 - 10 * Math.cos(a + 0.4), y1 - 10 * Math.sin(a + 0.4)); g.closePath(); g.fill();
    g.beginPath(); g.arc(x0, y0, 4, 0, Math.PI * 2); g.fill();
    g.font = '12px system-ui, sans-serif';
    // Labels a little inside the arrow, so they stay on the canvas.
    const [ux, uy] = [Math.cos(a) * 18, Math.sin(a) * 18];
    g.fillText(Math.round(z.level * 100) + ' %', x0 + ux + 6, y0 + uy + 4);
    g.fillText(Math.round(gr.level_to * 100) + ' %', x1 - ux + 6, y1 - uy + 4);
    g.restore();
  }
}
/** The static beam guard (T-256) on the last frame (the overlay draws its cells). */
let dwellStatus = null;
function assignDwellStatus(v) { return (dwellStatus = v); }
// The audience mode (T-255) from /api/frame: Public zones drawn blanked unless scanning.
let audienceMode = null;
function assignAudienceMode(v) { return (audienceMode = v); }
function showDwell(d) {
  if (!d) return;
  $('dwellLed').classList.toggle('on', d.active);
  $('dwellInfo').textContent = !d.active ? ''
    : `Garde active : ${d.points_dimmed} point(s) atténué(s)` + (d.extent_factor < 1 ? ` · figure trop petite (${(d.extent * 100).toFixed(1)} %)` : '')
      + (d.cells.length ? ` · ${d.cells.length} case(s) trop exposée(s)` : '');
}
function showStrobe(st) {
  if (!st) return;
  $('strobeLed').classList.toggle('on', st.active);
  $('strobeInfo').textContent = st.active ? 'Limiteur actif : strobe trop long, sortie maintenue allumée.'
    : st.fast ? `Strobe rapide (${st.rate_hz.toFixed(1)} Hz) depuis ${st.burst_s.toFixed(1)} s`
    : [st.beams_blanked ? `${st.beams_blanked} faisceau(x) sous l'horizon éteint(s)` : '',
       st.points_masked ? `${st.points_masked} point(s) masqué(s) par les zones ou l'horizon` : ''].filter(Boolean).join(' · ');
}

export { assignAudienceMode, assignDwellStatus, audienceMode, drawPsnOverlay, drawSafetyOverlay,
  drawTargetsOverlay, mtView, reloadTargets, showDwell, showStrobe, showTargetPicker };
