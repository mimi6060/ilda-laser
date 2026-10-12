// Laser Studio — cuelist.js (T-435).
// Sections (as in the former index.html script):
//   scenes
//   Liste de cues (T-410)
//   calibration
//   test patterns (T-404)
//   zone trace (T-447)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './music.js';
import { $, api, apiJson, armed, calibration, post, postLatest, sendLatest } from './core.js';
import { clientId } from './presence.js';
import { CONFIRM_HOLD_MS, LOCKED_FR, flash, holdToRun, showMode } from './workspaces.js';
import { safety } from './safety.js';
import { cueCategories, deck, presetById, presets, refreshLook } from './cues.js';
import { loadState } from './venue.js';
import { esc } from './midi.js';
import { emptyState } from './command.js';

// ---------- scenes ----------
let scenes = [];
function assignScenes(v) { return (scenes = v); }
function renderScenes() {
  $('sceneList').replaceChildren();
  // T-437: what a scene is for, and the first one in one click.
  if (!scenes.length) $('sceneList').appendChild(emptyState('Une scène garde le look courant pour le rejouer. Réglez un look, donnez-lui un nom (ou pas), puis sauvez-le.',
    [['Sauver le look courant', () => saveScene($('sceneName').value.trim() || 'Scène 1')]]));
  scenes.forEach((sc, i) => {
    const row = document.createElement('div');
    row.className = 'scene'; row.dataset.index = i;
    row.innerHTML = `<span class="n"></span><span class="d">${sc.duration_secs} s</span><button>▶</button><button class="hold-confirm lock-edit" title="Supprimer (maintenir pour confirmer)">✕</button>`;
    row.querySelector('.n').textContent = sc.name;
    const [play, del] = row.querySelectorAll('button');
    play.onclick = async () => { await post('/api/scenes/play', { name: sc.name }); await loadState(); };
    // Destructive (T-283): hold to confirm, never a single click.
    holdToRun(del, CONFIRM_HOLD_MS, async () => { await post('/api/scenes/delete', { name: sc.name }); await loadState(); },
      () => flash('Maintenir pour confirmer'));
    $('sceneList').appendChild(row);
  });
}
function highlightPlaylist(index) {
  document.querySelectorAll('.scene').forEach(r => r.classList.toggle('playing', index !== null && +r.dataset.index === index));
}
async function saveScene(name) {
  await post('/api/scenes/save', { name, duration_secs: +$('sceneDur').value || 8 });
  $('sceneName').value = '';
  await loadState();
}
$('sceneSave').onclick = async () => {
  const name = $('sceneName').value.trim();
  if (!name) { $('sceneName').focus(); return; }
  await post('/api/scenes/save', { name, duration_secs: +$('sceneDur').value || 8 });
  $('sceneName').value = '';
  await loadState();
};
$('plStart').onclick = () => post('/api/playlist/start');

// ---------- Liste de cues (T-410) ----------
// GET /api/cuelists for the saved lists; where the loaded one is comes
// with every state (`cuelist`). Edits are saved whole (POST /api/cuelists),
// a moment after the last change; show mode hides them (Go stays).
let cqLists = [], cqPlayer = null, cqEdit = null, cqSaveTimer = 0, cqKey = '', cqControls = [];
const CQ_TRIGGERS = [['go', 'Go'], ['after', 'Après n s'], ['follow', 'À la suite'], ['beats', 'Sur le temps (tous les n temps)'], ['audio', 'Sur événement audio']];
const CQ_EVENTS = { kick: 'Grosse caisse', snare: 'Caisse claire', hat: 'Charleston', drop: 'Drop', beat: 'Temps', onset: 'Attaque' };
const CQ_KINDS = [['cut', 'Coupe'], ['fade', 'Fondu'], ['fade_through_black', 'Fondu au noir'], ['morph', 'Morph']];
const cqOpts = (list, v) => list.map(([k, l]) => `<option value="${k}"${k === v ? ' selected' : ''}>${esc(l)}</option>`).join('');
async function loadCueLists() {
  try {
    const v = await apiJson('GET', '/api/cuelists', undefined, { cache: 'no-store' });
    cqLists = v.lists || [];
    if (!cqControls.length) {
      const all = await apiJson('GET', '/api/controls');
      cqControls = (all.controls || all).filter(c => c.external && c.id !== 'transport.arm' && !c.id.startsWith('list.'));
    }
    cqPlayer = v.player;
  } catch { return; }
  cqEdit = cqPlayer ? structuredClone(cqLists.find(l => l.name === cqPlayer.name) || null) : null;
  renderCueList();
}
function cqSave() {
  clearTimeout(cqSaveTimer);
  cqSaveTimer = setTimeout(async () => {
    if (!cqEdit) return;
    const r = await api('POST', '/api/cuelists', cqEdit, { quiet: true });
    if (r.ok) {
      const v = await r.json();
      cqLists = v.lists; cqPlayer = v.player; $('cqMsg').textContent = '';
      // The server numbers new steps: take its copy.
      const saved = cqLists.find(l => l.name === cqEdit.name);
      if (saved && saved.steps.length === cqEdit.steps.length && saved.steps.some((st, i) => st.id !== cqEdit.steps[i].id)) { cqEdit = structuredClone(saved); renderCueList(); }
    } else {
      $('cqMsg').textContent = await r.text();
      if (r.status === 423) flash(LOCKED_FR);
    }
  }, 250);
}
function cqChanged(rerender) {
  if (rerender) renderCueList();
  cqSave();
}
async function cqAct(action, source, body = {}) {
  const r = await api('POST', '/api/cuelist/' + action, { source, ...body }, { quiet: true });
  if (!r.ok) { const t = await r.text(); $('cqMsg').textContent = t; flash(t); return false; }
  $('cqMsg').textContent = '';
  showCueList(await r.json());
  refreshLook();
  return true;
}
function cqSourceOptions(src) {
  const v = src.type === 'cue' ? 'cue:' + src.id : src.type === 'scene' ? 'scene:' + src.name : 'black';
  let html = `<option value="black">Noir</option>`;
  let known = v === 'black';
  for (const cat of cueCategories.length ? cueCategories : [...new Set(presets.map(p => p.category))]) {
    const items = presets.filter(p => p.category === cat);
    if (!items.length) continue;
    html += `<optgroup label="${esc(cat)}">` + items.map(p => { const k = 'cue:' + p.id; known ||= k === v; return `<option value="${esc(k)}"${k === v ? ' selected' : ''}>${esc(p.name)}</option>`; }).join('') + '</optgroup>';
  }
  if (scenes.length) html += '<optgroup label="Scènes">' + scenes.map(sc => { const k = 'scene:' + sc.name; known ||= k === v; return `<option value="${esc(k)}"${k === v ? ' selected' : ''}>${esc(sc.name)}</option>`; }).join('') + '</optgroup>';
  if (!known) html += `<option value="${esc(v)}" selected>${esc(v.replace(/^\w+:/, ''))} (introuvable)</option>`;
  return html;
}
function cqStepSourceName(src) {
  if (src.type === 'cue') return presetById.get(src.id)?.name || src.id;
  if (src.type === 'scene') return 'Scène ' + src.name;
  return 'Noir';
}
function cqStepRow(st, i) {
  const row = document.createElement('div');
  row.className = 'cq-step'; row.dataset.index = i;
  const ro = showMode ? ' disabled' : '';
  const trig = st.trigger || { type: 'go' };
  const n = trig.type === 'after' ? trig.s : trig.type === 'beats' ? trig.n : '';
  const sp = st.split;
  row.innerHTML = `<div class="cq-line">
      <button class="cq-n cq-b" title="Aller à ce pas">${i + 1}</button>
      <input class="cq-label" placeholder="Libellé" maxlength="80"${ro}>
      <select class="cq-src" title="Ce que le pas montre"${ro}>${cqSourceOptions(st.source || { type: 'black' })}</select>
    </div>
    <div class="cq-line">
      <select class="cq-trig" title="Déclencheur"${ro}>${cqOpts(CQ_TRIGGERS, trig.type)}</select>
      <input type="number" class="cq-trig-n" min="${trig.type === 'beats' ? 1 : 0}" step="${trig.type === 'beats' ? 1 : 0.1}" title="${trig.type === 'beats' ? 'Tous les n temps' : 'Secondes après le départ du pas précédent'}"${ro}${trig.type === 'after' || trig.type === 'beats' ? '' : ' hidden'}>
      <select class="cq-trig-ev" title="Événement audio"${ro}${trig.type === 'audio' ? '' : ' hidden'}>${cqOpts(Object.entries(CQ_EVENTS), trig.event || 'kick')}</select>
      <span class="cq-t">délai</span><input type="number" class="cq-delay" min="0" step="0.1" title="Délai (s) entre le déclencheur et le départ"${ro}>
    </div>
    <div class="cq-line">
      <span class="cq-t">entrée</span><input type="number" class="cq-in" min="0" step="0.1" title="Fondu d'entrée (s)"${ro}>
      <span class="cq-t">sortie</span><input type="number" class="cq-out" min="0" step="0.1" title="Fondu de sortie (s), quand le pas suivant arrive"${ro}>
      <select class="cq-kind" title="Transition depuis le pas précédent (depuis le noir : toujours un fondu)"${ro}>${cqOpts(CQ_KINDS, st.transition || 'fade')}</select>
      <button class="cq-b cq-more-btn" title="Temps séparés, couleur, contrôle déclenché">⋯</button>
      <button class="cq-b cq-dup lock-edit" title="Dupliquer">⧉</button>
      <button class="cq-b cq-del lock-edit" title="Supprimer ce pas">✕</button>
    </div>
    <div class="cq-more" hidden>
      <div class="cq-line">
        <label class="chk"><input type="checkbox" class="cq-split"${sp ? ' checked' : ''}${ro}> Temps séparés</label>
        <span class="cq-t">géom.</span><input type="number" class="cq-sg" min="0" step="0.1"${ro}>
        <span class="cq-t">coul.</span><input type="number" class="cq-sc" min="0" step="0.1"${ro}>
        <span class="cq-t">lum.</span><input type="number" class="cq-sb" min="0" step="0.1"${ro}>
      </div>
      <div class="cq-line">
        <label class="chk"><input type="checkbox" class="cq-keep"${st.keep_color ? ' checked' : ''}${ro}> Garder la couleur du pas précédent</label>
      </div>
      <div class="cq-line">
        <span class="cq-t">au départ</span>
        <select class="cq-fire" title="Contrôle déclenché au départ du pas (jamais l'armement)"${ro}><option value="">Aucun contrôle</option>${cqControls.map(c => `<option value="${esc(c.id)}"${st.fire && st.fire.control === c.id ? ' selected' : ''}>${esc(c.label_fr)}</option>`).join('')}</select>
        <input type="number" class="cq-fire-v" step="0.1" title="Valeur"${ro}>
      </div>
    </div>`;
  const q = c => row.querySelector(c);
  q('.cq-label').value = st.label || '';
  if (n !== '') q('.cq-trig-n').value = n;
  q('.cq-delay').value = st.delay_s || 0;
  q('.cq-in').value = st.fade_in_s || 0;
  q('.cq-out').value = st.fade_out_s || 0;
  q('.cq-sg').value = sp ? sp.geometry_s : st.fade_in_s || 0;
  q('.cq-sc').value = sp ? sp.color_s : st.fade_in_s || 0;
  q('.cq-sb').value = sp ? sp.brightness_s : st.fade_in_s || 0;
  ['.cq-sg', '.cq-sc', '.cq-sb'].forEach(c => { q(c).disabled = showMode || !sp; });
  q('.cq-fire-v').value = st.fire ? st.fire.value : 1;
  q('.cq-n').onclick = () => cqAct('goto', 'ui', { step: i });
  q('.cq-more-btn').onclick = () => { q('.cq-more').hidden = !q('.cq-more').hidden; };
  if (showMode) return row;
  const num = c => Math.max(0, +q(c).value || 0);
  q('.cq-label').onchange = () => { st.label = q('.cq-label').value.trim(); cqChanged(false); };
  q('.cq-src').onchange = () => {
    const v = q('.cq-src').value;
    st.source = v === 'black' ? { type: 'black' } : v.startsWith('scene:') ? { type: 'scene', name: v.slice(6) } : { type: 'cue', id: v.slice(4) };
    cqChanged(false);
  };
  q('.cq-trig').onchange = () => {
    const t = q('.cq-trig').value;
    st.trigger = t === 'after' ? { type: 'after', s: 2 } : t === 'beats' ? { type: 'beats', n: 4 } : t === 'audio' ? { type: 'audio', event: 'kick' } : { type: t };
    cqChanged(true);
  };
  q('.cq-trig-n').onchange = () => {
    if (st.trigger.type === 'after') st.trigger.s = num('.cq-trig-n');
    else if (st.trigger.type === 'beats') st.trigger.n = Math.max(1, Math.round(num('.cq-trig-n')));
    cqChanged(false);
  };
  q('.cq-trig-ev').onchange = () => { st.trigger.event = q('.cq-trig-ev').value; cqChanged(false); };
  q('.cq-delay').onchange = () => { st.delay_s = num('.cq-delay'); cqChanged(false); };
  q('.cq-in').onchange = () => { st.fade_in_s = num('.cq-in'); cqChanged(false); };
  q('.cq-out').onchange = () => { st.fade_out_s = num('.cq-out'); cqChanged(false); };
  q('.cq-kind').onchange = () => { st.transition = q('.cq-kind').value; cqChanged(false); };
  const split = () => { st.split = q('.cq-split').checked ? { geometry_s: num('.cq-sg'), color_s: num('.cq-sc'), brightness_s: num('.cq-sb') } : null; if (!st.split) delete st.split; };
  q('.cq-split').onchange = () => { split(); ['.cq-sg', '.cq-sc', '.cq-sb'].forEach(c => { q(c).disabled = !st.split; }); cqChanged(false); };
  ['.cq-sg', '.cq-sc', '.cq-sb'].forEach(c => { q(c).onchange = () => { split(); cqChanged(false); }; });
  q('.cq-keep').onchange = () => { st.keep_color = q('.cq-keep').checked; cqChanged(false); };
  const fire = () => { const c = q('.cq-fire').value; if (c) st.fire = { control: c, value: +q('.cq-fire-v').value || 0 }; else delete st.fire; cqChanged(false); };
  q('.cq-fire').onchange = fire; q('.cq-fire-v').onchange = fire;
  q('.cq-dup').onclick = () => { cqEdit.steps.splice(i + 1, 0, { ...structuredClone(st), id: 0 }); cqChanged(true); };
  q('.cq-del').onclick = () => { cqEdit.steps.splice(i, 1); cqChanged(true); };
  // Drag to reorder.
  row.draggable = true;
  row.addEventListener('dragstart', e => { if (e.target.closest('input, select')) { e.preventDefault(); return; } e.dataTransfer.setData('text/x-cq-step', String(i)); });
  row.addEventListener('dragover', e => { if (e.dataTransfer.types.includes('text/x-cq-step')) { e.preventDefault(); row.classList.add('drag-over'); } });
  row.addEventListener('dragleave', () => row.classList.remove('drag-over'));
  row.addEventListener('drop', e => {
    e.preventDefault(); row.classList.remove('drag-over');
    const from = +e.dataTransfer.getData('text/x-cq-step');
    if (Number.isNaN(from) || from === i) return;
    const [moved] = cqEdit.steps.splice(from, 1);
    cqEdit.steps.splice(i, 0, moved);
    cqChanged(true);
  });
  return row;
}
function renderCueList() {
  const sel = $('cqSelect');
  const name = cqPlayer ? cqPlayer.name : '';
  sel.innerHTML = `<option value="">${cqLists.length ? '— choisir une liste —' : 'Aucune liste'}</option>` + cqLists.map(l => `<option${l.name === name ? ' selected' : ''}>${esc(l.name)}</option>`).join('');
  $('cqLayer').value = String(cqEdit ? cqEdit.layer : 1);
  $('cqWrap').checked = !!(cqEdit && cqEdit.wrap);
  const box = $('cqSteps');
  if (!cqEdit) {
    box.innerHTML = '<div class="hint">' + (cqLists.length ? 'Choisissez une liste pour la jouer.' : 'Créez une liste (« + Liste »), ajoutez des pas, puis GO.') + '</div>';
  } else if (!cqEdit.steps.length) {
    box.innerHTML = '<div class="hint">Liste vide : « + Ajouter un pas ».</div>';
  } else {
    box.replaceChildren(...cqEdit.steps.map(cqStepRow));
  }
  cqKey = '';
  showCueList(cqPlayer);
}
function showCueList(v) {
  cqPlayer = v || null;
  const key = JSON.stringify(v);
  if (key === cqKey) return;
  cqKey = key;
  if (!$('cqPanel') || $('cqPanel').hidden) return;
  if ((v ? v.name : null) !== (cqEdit ? cqEdit.name : null)) {
    // Loaded from elsewhere (another page, a project): follow it.
    cqEdit = v ? structuredClone(cqLists.find(l => l.name === v.name) || null) : null;
    if (v && !cqEdit) { loadCueLists(); return; }
    renderCueList();
    return;
  }
  const st = $('cqStatus');
  if (!v) { st.textContent = 'Aucune liste chargée'; $('cqProgress').style.width = '0'; return; }
  const ph = v.phase || {};
  const where = v.index == null ? 'Début' : `Pas <b>${v.index + 1}</b>/${v.steps}${v.label ? ' · ' + esc(v.label) : ''}`;
  const state = v.paused ? '<span class="paused">PAUSE</span>' : ph.phase === 'fading' ? 'fondu' : ph.phase === 'delay' ? `délai ${ph.left_s.toFixed(1)} s` : '';
  const next = v.next != null ? `suivant : ${v.next + 1}${v.next_label ? ' ' + esc(v.next_label) : ''}${v.next_trigger && v.next_trigger.type !== 'go' ? ' (auto)' : ''}` : v.waiting != null ? '' : 'fin de liste';
  st.innerHTML = [where, state, next].filter(Boolean).join(' · ');
  $('cqProgress').style.width = (v.progress != null ? Math.round(v.progress * 100) : 0) + '%';
  $('cqPause').classList.toggle('on', !!v.paused);
  document.querySelectorAll('#cqSteps .cq-step').forEach(r => {
    const i = +r.dataset.index;
    r.classList.toggle('current', i === v.index);
    r.classList.toggle('waiting', i === v.waiting);
    r.classList.toggle('next', i === v.next && i !== v.index);
  });
}
$('cqSelect').onchange = async () => {
  const name = $('cqSelect').value;
  if (!name) return;
  if (await post('/api/cuelist/load', { name })) await loadCueLists();
};
$('cqNew').onclick = async () => {
  const name = (prompt('Nom de la nouvelle liste de cues', 'Liste ' + (cqLists.length + 1)) || '').trim();
  if (!name) return;
  const r = await api('POST', '/api/cuelists', { name, layer: 1, wrap: false, steps: [] }, { quiet: true });
  if (!r.ok) { $('cqMsg').textContent = await r.text(); return; }
  await post('/api/cuelist/load', { name });
  await loadCueLists();
};
holdToRun($('cqDelete'), CONFIRM_HOLD_MS, async () => {
  if (!cqEdit) return;
  await post('/api/cuelists/delete', { name: cqEdit.name });
  await loadCueLists();
}, () => flash('Maintenir pour confirmer'));
$('cqLayer').onchange = () => { if (cqEdit) { cqEdit.layer = +$('cqLayer').value; cqChanged(false); } };
$('cqWrap').onchange = () => { if (cqEdit) { cqEdit.wrap = $('cqWrap').checked; cqChanged(false); } };
$('cqAdd').onclick = () => { if (!cqEdit) return flash('Choisissez ou créez une liste'); cqEdit.steps.push({ id: 0, label: '', source: { type: 'black' }, trigger: { type: 'go' }, delay_s: 0, fade_in_s: 1, fade_out_s: 1, transition: 'fade', keep_color: false }); cqChanged(true); };
$('cqAddCue').onclick = () => {
  if (!cqEdit) return flash('Choisissez ou créez une liste');
  const id = deck && deck.active && deck.active.length ? deck.active[deck.active.length - 1].cue : null;
  if (!id) return flash('Aucun cue ne joue');
  cqEdit.steps.push({ id: 0, label: presetById.get(id)?.name || '', source: { type: 'cue', id }, trigger: { type: 'go' }, delay_s: 0, fade_in_s: 1, fade_out_s: 1, transition: 'fade', keep_color: false });
  cqChanged(true);
};
$('cqGo').onclick = () => cqAct('go', 'ui');
$('cqBack').onclick = () => cqAct('back', 'ui');
$('cqPause').onclick = () => cqAct('pause', 'ui');
$('cqTop').onclick = () => cqAct('top', 'ui');
$('cqGoto').onclick = () => cqAct('goto', 'ui', { step: Math.max(0, (+$('cqGotoN').value || 1) - 1) });
$('plStop').onclick = () => post('/api/playlist/stop');

// ---------- calibration ----------
const calMap = { cOx: ['offset_x', 100], cOy: ['offset_y', 100], cSx: ['scale_x', 100], cSy: ['scale_y', 100], cRot: ['rotation_deg', 1] };
function showCalibration() {
  for (const [id, [key, k]] of Object.entries(calMap)) {
    $(id).value = Math.round(calibration[key] * k);
    $(id + 'V').textContent = key === 'rotation_deg' ? $(id).value + '°' : (calibration[key]).toFixed(2);
  }
}
let calTimer = null;
Object.keys(calMap).forEach(id => $(id).addEventListener('input', () => {
  for (const [cid, [key, k]] of Object.entries(calMap)) calibration[key] = $(cid).value / k;
  showCalibration();
  clearTimeout(calTimer); calTimer = setTimeout(() => postLatest('calibration', '/api/calibration', calibration), 60);
}));

// ---------- test patterns (T-404) ----------
// The server owns the mode; the frame poll reports it (`f.test_pattern`).
let testPattern = null, tpTimer = null;
async function loadTestPatterns() {
  try {
    const r = await apiJson('GET', '/api/testpattern');
    const sel = $('tpSelect');
    for (const p of r.patterns) sel.add(new Option(p.label, p.id));
    showTestPattern(r.state);
  } catch (e) { console.warn('/api/testpattern', e); }
}
function showTestPattern(tp) {
  if (!tp) return;
  testPattern = tp;
  const on = !!tp.active;
  $('tpBanner').hidden = !on;
  $('tpExit').disabled = !on;
  if (document.activeElement !== $('tpSelect')) $('tpSelect').value = tp.active || '';
  if (document.activeElement !== $('tpBright')) $('tpBright').value = Math.round(tp.brightness * 100);
  $('tpBrightV').textContent = $('tpBright').value + ' %';
}
// `latest`: the brightness slider, latest wins (T-484). Leaving the
// pattern posts directly.
async function postTestPattern(body, latest = false) {
  const r = latest ? await sendLatest('testpattern', 'POST', '/api/testpattern', body) : await api('POST', '/api/testpattern', body);
  if (!r) return;
  if (r.ok) showTestPattern((await r.json()).state); else console.warn('/api/testpattern', await r.text());
}
$('tpSelect').addEventListener('change', () => postTestPattern({ pattern: $('tpSelect').value || null }));
$('tpBright').addEventListener('input', () => {
  $('tpBrightV').textContent = $('tpBright').value + ' %';
  clearTimeout(tpTimer); tpTimer = setTimeout(() => postTestPattern({ brightness: $('tpBright').value / 100 }, true), 60);
});
$('tpExit').addEventListener('click', () => postTestPattern({ pattern: null }));
$('tpBannerExit').addEventListener('click', () => postTestPattern({ pattern: null }));

// ---------- zone trace (T-447) ----------
// The server owns the mode (60 s, ends on Échap, a disarm or this page
// gone); the frame poll reports it (`f.zone_trace`). A mouse action only:
// no shortcut, no control id.
let zoneTrace = null;
function showZoneTrace(zt, isArmed) {
  if (!zt) return;
  zoneTrace = zt;
  const on = !!zt.on;
  $('ztBanner').hidden = !on;
  $('ztBannerText').textContent = `Vérification des zones — fin dans ${zt.remaining_s} s`;
  $('ztBtn').textContent = on ? 'Arrêter le tracé' : 'Tracer les zones au laser';
  $('ztBtn').setAttribute('aria-pressed', String(on));
  $('ztArmHint').hidden = !!isArmed;
  if (on && document.activeElement !== $('ztOnly')) $('ztOnly').value = zt.only == null ? '' : String(zt.only);
}
/** The menu: every zone of the safety settings, by name. */
function renderZoneTraceMenu() {
  const sel = $('ztOnly');
  const keep = sel.value;
  sel.replaceChildren(new Option('Toutes les zones', ''), ...(safety?.zones || []).map((z, i) => new Option(z.name || `Zone ${i + 1}`, String(z.id))));
  sel.value = [...sel.options].some(o => o.value === keep) ? keep : '';
}
async function postZoneTrace(on) {
  $('ztError').textContent = '';
  const body = { on, client_id: clientId };
  if (on && $('ztOnly').value !== '') body.only = Number($('ztOnly').value);
  const r = await api('POST', '/api/safety/trace', body, { quiet: true });
  if (r.ok) showZoneTrace(await r.json(), armed);
  else $('ztError').textContent = await r.text();
}
$('ztBtn').addEventListener('click', () => postZoneTrace(!(zoneTrace && zoneTrace.on)));
$('ztBannerExit').addEventListener('click', () => postZoneTrace(false));

export { assignScenes, cqAct, cqPlayer, highlightPlaylist, loadCueLists, loadTestPatterns,
  postTestPattern, renderCueList, renderScenes, renderZoneTraceMenu, scenes, showCalibration,
  showCueList, showTestPattern, showZoneTrace, testPattern };
