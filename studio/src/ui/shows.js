// Laser Studio — shows.js (T-435).
// Sections (as in the former index.html script):
//   timeline (T-160)
//   Lecteur de shows (T-444)
//   Agenda (T-455)
//   Timecode entrant (T-168)
//   Timecode sortant (T-417)
//   Enregistrer le jeu en direct (T-169)
//   timeline audio (T-161)
//   show from the song (T-167)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import './macros.js';
import { $, api, apiJson, post } from './core.js';
import { flash } from './workspaces.js';
import { TL, dropShowChanges, tlChange, tlHeads, tlMsg } from './timeline-editor.js';
import { esc } from './midi.js';

// ---------- timeline (T-160) ----------
// Load / play / pause / stop a saved show and follow its playhead. The
// editor (T-162) is further down.
let shows = [], timeline = null;
async function loadShows() {
  try { shows = await apiJson('GET', '/api/shows'); } catch { shows = []; }
  fillShowSelect($('tlShow'), 'Aucun show', timeline && timeline.name || '');
}
function fillShowSelect(sel, none, value) {
  sel.innerHTML = `<option value="">${none}</option>` + shows.map(s => `<option>${esc(s.name)}</option>`).join('');
  if (value && !shows.some(s => s.name === value)) sel.insertAdjacentHTML('beforeend', `<option>${esc(value)}</option>`);
  sel.value = value;
}
function fmtTime(s) { const m = Math.floor(s / 60); return m + ':' + (s - m * 60).toFixed(1).padStart(4, '0'); }
function showTimeline(t) {
  if (!t) return;
  timeline = t;
  if (document.activeElement !== $('tlShow') && $('tlShow').value !== (t.name || '')) fillShowSelect($('tlShow'), 'Aucun show', t.name || '');
  $('tlPlay').classList.toggle('active', t.playing);
  $('tlPause').classList.toggle('active', t.paused);
  $('tlLoop').classList.toggle('active', t.loop);
  showTake(t.take);
  const bar = `mesure ${Math.floor(t.bar) + 1}.${Math.floor((t.bar % 1) * t.beats_per_bar + 1e-6) + 1}`;
  const pos = !t.name ? '–' : t.time_base === 'beats'
    ? `temps ${t.position.toFixed(1)} / ${t.length.toFixed(0)} · ${bar}`
    : `${fmtTime(t.position)} / ${fmtTime(t.length)} · ${bar}`;
  $('tlPos').textContent = t.waiting ? 'attend la mesure suivante…' : pos;
  $('tlPos').classList.toggle('waiting', t.waiting);
  const pct = x => (t.length > 0 ? Math.min(100, 100 * x / t.length) : 0) + '%';
  $('tlHead').style.left = pct(t.position);
  const r = t.loop_region;
  $('tlRegion').hidden = !r;
  if (r) { $('tlRegion').style.left = pct(r[0]); $('tlRegion').style.width = `calc(${pct(r[1])} - ${pct(r[0])})`; }
}
// ---------- Lecteur de shows (T-444) ----------
// The list (GET /api/showlist, fetched again when its `rev` changes) and
// its state with every frame (`f.showlist`). Editing the list is an edit
// (locked in show mode, in the history); Lancer / Suivant / Précédent /
// Arrêter / Boucler are playback. Nothing here ever arms the laser.
const SL = { items: [], loop: false, rev: -1, sel: null, state: null, drag: null, loading: false };
const SL_TRANSPORT_FR = { stopped: 'Arrêté', playing: 'En cours', waiting: 'En attente du timecode', suspended: 'Suspendu' };
function slClock(s, approx) {
  if (s == null || !isFinite(s)) return '–';
  const t = Math.max(0, Math.round(s)), h = Math.floor(t / 3600), m = Math.floor(t / 60) % 60, sec = t % 60;
  const p = v => String(v).padStart(2, '0');
  return (approx ? '≈ ' : '') + (h ? `${h}:${p(m)}:${p(sec)}` : `${p(m)}:${p(sec)}`);
}
async function loadShowList() {
  if (SL.loading) return;
  SL.loading = true;
  try { takeShowList(await apiJson('GET', '/api/showlist', undefined, { quiet: true })); } catch { /* the next state tries again */ }
  finally { SL.loading = false; }
}
function takeShowList(v) {
  SL.items = v.items || [];
  SL.loop = !!v.loop_list;
  SL.rev = v.state ? v.state.rev : -1;
  if (SL.sel != null && SL.sel >= SL.items.length) SL.sel = null;
  renderShowList();
  showShowList(v.state);
}
function slBody(items) {
  return { items: items.map(i => ({ id: i.id, show: i.show, follow_system_tempo: i.follow_system_tempo, wait_timecode: i.wait_timecode })), loop_list: SL.loop };
}
async function saveShowList(items) {
  try { takeShowList(await apiJson('POST', '/api/showlist', slBody(items))); } catch { loadShowList(); }
}
function renderShowList() {
  const ol = $('slList');
  const cur = SL.state && SL.state.transport !== 'stopped' ? SL.state.index : null;
  ol.innerHTML = SL.items.map((it, i) => {
    const bad = it.problem ? ` <span class="sl-flag" title="${esc(it.problem.message || '')}">${esc(it.problem.kind)}</span>` : '';
    const sel = SL.sel === i;
    return `<li data-i="${i}" class="${i === cur ? 'current' : ''}${it.problem ? ' bad' : ''}">
      <button class="sl-grip lock-edit" draggable="true" data-i="${i}" aria-label="Déplacer « ${esc(it.show)} » (flèches haut / bas)" title="Glisser pour réordonner (ou flèches haut / bas)">⋮⋮</button>
      <button class="sl-name" data-i="${i}" aria-pressed="${sel}" title="Choisir : « Lancer » part de ce show">${i + 1}. ${esc(it.show)}</button>${bad}
      <span class="sl-dur" title="Durée du show${it.approx ? ' (show en temps : au tempo courant)' : ''}">${slClock(it.duration_s, it.approx)}</span>
      <label class="check lock-edit" title="Coché : le show joue sur l'horloge de tempo en cours. Décoché : le tempo passe au BPM du show (source Manuel) à son démarrage."><input type="checkbox" data-i="${i}" data-opt="follow_system_tempo"${it.follow_system_tempo ? ' checked' : ''}> Suivre le tempo du système</label>
      <label class="check lock-edit" title="Le show est chargé et attend que le timecode entrant (TC-IN) atteigne sa plage"><input type="checkbox" data-i="${i}" data-opt="wait_timecode"${it.wait_timecode ? ' checked' : ''}> Attendre le timecode</label>
      <button class="sl-x lock-edit" data-dup="${i}" aria-label="Dupliquer « ${esc(it.show)} »" title="Dupliquer">⧉</button>
      <button class="sl-x lock-edit" data-del="${i}" aria-label="Retirer « ${esc(it.show)} » de la liste" title="Retirer de la liste">✕</button>
    </li>`;
  }).join('');
  $('slEmpty').hidden = SL.items.length > 0;
  $('slLoop').setAttribute('aria-pressed', SL.loop);
}
function showShowList(st) {
  if (!st) return;
  SL.state = st;
  if (st.rev !== SL.rev) loadShowList();
  const n = SL.items.length, i = st.index;
  const it = i != null ? SL.items[i] : null;
  let now = SL_TRANSPORT_FR[st.transport] || st.transport;
  if (st.transport !== 'stopped' && it) {
    now = `${now} : ${i + 1}/${n} — ${it.show} · ${slClock(st.elapsed_s, st.approx)} écoulé · ${slClock(st.remaining_s, st.approx)} restant · liste : ${slClock(st.list_remaining_s, st.approx)}`;
  } else if (n) {
    now = `${now} · liste : ${slClock(st.list_remaining_s, st.approx)}`;
  }
  $('slNow').textContent = now;
  let label = '', cls = '';
  if (st.transport === 'suspended') { label = 'Suspendu : un show a été lancé à la main'; cls = 'warn'; }
  else if (st.preparing && st.transport !== 'stopped') label = 'Préparation…';
  else if (st.next_ready && st.transport !== 'stopped') { label = 'Prêt : ' + (SL.items[st.next_index] ? SL.items[st.next_index].show : 'suivant'); cls = 'ok'; }
  $('slState').textContent = label;
  $('slState').className = 'hint ' + cls;
  $('slResume').hidden = st.transport !== 'suspended';
  $('slMsg').textContent = st.problem || '';
  $('slMsg').className = 'hint' + (st.problem ? ' warn' : '');
  $('slPlay').classList.toggle('active', st.transport === 'playing' || st.transport === 'waiting');
  $('slLoop').setAttribute('aria-pressed', st.loop_list);
  document.querySelectorAll('#slList li').forEach(li => li.classList.toggle('current', st.transport !== 'stopped' && +li.dataset.i === i));
}
async function slAct(what, body) {
  try { takeShowList(await apiJson('POST', '/api/showlist/' + what, body || {}, { source: 'Lecteur de shows' })); } catch { /* said by api() */ }
}
$('slPlay').addEventListener('click', () => slAct('play', SL.sel != null ? { index: SL.sel } : {}));
$('slResume').addEventListener('click', () => slAct('play', {}));
$('slNext').addEventListener('click', () => slAct('next'));
$('slPrev').addEventListener('click', () => slAct('prev'));
$('slStop').addEventListener('click', () => slAct('stop'));
$('slLoop').addEventListener('click', () => slAct('loop', { on: !SL.loop }));
$('slAdd').addEventListener('focus', async () => {
  try { shows = await apiJson('GET', '/api/shows'); } catch { return; }
  fillShowSelect($('slAdd'), '+ Ajouter un show', '');
});
$('slAdd').addEventListener('change', () => {
  const name = $('slAdd').value;
  $('slAdd').value = '';
  if (name) saveShowList([...SL.items, { id: 0, show: name, follow_system_tempo: true, wait_timecode: false }]);
});
function slMove(from, to) {
  if (from === to || to < 0 || to >= SL.items.length) return;
  const items = SL.items.slice();
  const [it] = items.splice(from, 1);
  items.splice(to, 0, it);
  if (SL.sel === from) SL.sel = to;
  saveShowList(items);
}
$('slList').addEventListener('click', e => {
  const b = e.target.closest('button');
  if (!b) return;
  if (b.dataset.del != null) {
    const i = +b.dataset.del;
    if (SL.sel === i) SL.sel = null;
    saveShowList(SL.items.filter((_, k) => k !== i));
  } else if (b.dataset.dup != null) {
    const i = +b.dataset.dup;
    const items = SL.items.slice();
    items.splice(i + 1, 0, { ...SL.items[i], id: 0 });
    saveShowList(items);
  } else if (b.classList.contains('sl-name')) {
    const i = +b.dataset.i;
    SL.sel = SL.sel === i ? null : i;
    document.querySelectorAll('#slList .sl-name').forEach(x => x.setAttribute('aria-pressed', +x.dataset.i === SL.sel));
  }
});
$('slList').addEventListener('change', e => {
  const c = e.target.closest('input[data-opt]');
  if (!c) return;
  const items = SL.items.map((it, k) => k === +c.dataset.i ? { ...it, [c.dataset.opt]: c.checked } : it);
  saveShowList(items);
});
$('slList').addEventListener('keydown', e => {
  const g = e.target.closest('.sl-grip');
  if (!g || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') || e.metaKey || e.altKey || e.ctrlKey) return;
  e.preventDefault();
  slMove(+g.dataset.i, +g.dataset.i + (e.key === 'ArrowUp' ? -1 : 1));
});

// ---------- Agenda (T-455) ----------
// The entries (GET /api/agenda, fetched again when `rev` moves) and the
// summary with every frame (`f.agenda`). Every change posts the whole
// agenda: an edit (history, locked in show mode). The agenda starts a
// playback; it never arms. No shortcut, no control, no MIDI.
const AG = { agenda: { enabled: false, entries: [] }, rev: -1, state: null, loading: false };
const AG_DAYS = [['L', 'lundi'], ['M', 'mardi'], ['M', 'mercredi'], ['J', 'jeudi'], ['V', 'vendredi'], ['S', 'samedi'], ['D', 'dimanche']];
async function loadAgenda() {
  if (AG.loading) return;
  AG.loading = true;
  try {
    if (!shows.length) { try { shows = await apiJson('GET', '/api/shows', undefined, { quiet: true }); } catch { /* names only */ } }
    takeAgenda(await apiJson('GET', '/api/agenda', undefined, { quiet: true }));
  } catch { /* the next state tries again */ }
  finally { AG.loading = false; }
}
function takeAgenda(v) {
  AG.agenda = v.agenda || { enabled: false, entries: [] };
  AG.rev = v.state ? v.state.rev : -1;
  renderAgenda();
  showAgenda(v.state, true);
}
async function saveAgenda(agenda) {
  try { takeAgenda(await apiJson('POST', '/api/agenda', agenda, { source: 'Agenda' })); } catch { loadAgenda(); }
}
function agTargetValue(t) { return t.kind === 'show' ? 'show:' + t.name : t.kind; }
function agTarget(v) { return v.startsWith('show:') ? { kind: 'show', name: v.slice(5) } : { kind: v }; }
function renderAgenda() {
  const nextId = AG.state && AG.state.next ? AG.state.next.id : null;
  $('agList').innerHTML = AG.agenda.entries.map((e, i) => {
    const k = 'ag' + i;
    const opts = [['show_list', 'Lecteur de shows (liste)'], ...shows.map(s => ['show:' + s.name, 'Show : ' + s.name]), ['stop', 'Arrêt et noir (désarme)']];
    const cur = agTargetValue(e.target);
    if (!opts.some(o => o[0] === cur)) opts.push([cur, 'Show : ' + (e.target.name || '?') + ' (introuvable)']);
    const dated = e.date != null;
    const days = AG_DAYS.map(([l, n], d) => `<button class="ag-day" data-i="${i}" data-day="${d}" aria-pressed="${!!e.days[d]}" aria-label="${n}" title="${n}">${l}</button>`).join('');
    return `<li data-i="${i}" class="${e.enabled ? '' : 'off'}${e.id === nextId ? ' next' : ''}">
      <label for="${k}n">Nom</label><input type="text" id="${k}n" class="ag-name" data-i="${i}" data-f="name" maxlength="64" value="${esc(e.name)}">
      <label for="${k}t">Cible</label><select id="${k}t" class="ag-target" data-i="${i}" data-f="target">${opts.map(([v, l]) => `<option value="${esc(v)}"${v === cur ? ' selected' : ''}>${esc(l)}</option>`).join('')}</select>
      <label for="${k}h">Heure</label><input type="time" id="${k}h" class="ag-time" data-i="${i}" data-f="time" value="${esc(e.time)}" required>
      <label for="${k}m">Quand</label><select id="${k}m" data-i="${i}" data-f="mode"><option value="days"${dated ? '' : ' selected'}>Jours</option><option value="date"${dated ? ' selected' : ''}>Date</option></select>
      ${dated ? `<label for="${k}d">Date</label><input type="date" id="${k}d" data-i="${i}" data-f="date" value="${esc(e.date)}">`
        : `<span class="ag-days" role="group" aria-label="Jours de « ${esc(e.name)} »">${days}</span>`}
      <label for="${k}a">Active</label><input type="checkbox" id="${k}a" data-i="${i}" data-f="enabled"${e.enabled ? ' checked' : ''}>
      <button class="ag-x" data-del="${i}" aria-label="Supprimer « ${esc(e.name)} »" title="Supprimer l'entrée">✕</button>
    </li>`;
  }).join('');
  $('agEmpty').hidden = AG.agenda.entries.length > 0;
  $('agEnabled').setAttribute('aria-pressed', AG.agenda.enabled);
}
function showAgenda(st, full) {
  if (!st) return;
  AG.state = Object.assign({}, AG.state, st);
  if (!full && st.rev !== AG.rev) loadAgenda();
  const next = st.next;
  let text = 'Agenda inactif';
  if (st.enabled) text = next ? `Prochain : « ${next.name} » — ${next.when}` : 'Agenda actif — aucune échéance';
  if (st.playing) text += ` · en cours : « ${st.playing} »`;
  $('agNext').textContent = text;
  $('agSuspended').textContent = st.suspended ? `Agenda suspendu (${st.suspended}) : aucune lecture ne démarre avant le prochain armement du laser.` : '';
  $('agEnabled').setAttribute('aria-pressed', !!st.enabled);
  const pill = $('agendaPill');
  pill.hidden = !st.enabled;
  pill.textContent = st.suspended ? 'Agenda suspendu' : next ? `Agenda actif — prochain ${next.time}` : 'Agenda actif';
  pill.classList.toggle('warn', !!st.suspended);
  pill.title = (next ? `Prochaine échéance : « ${next.name} » — ${next.when}. ` : '') + 'Agenda actif : il lance des lectures à heure fixe (TIMELINE › Agenda). Il n\'arme jamais le laser.';
  if (full && st.last_runs) {
    $('agRuns').innerHTML = st.last_runs.length
      ? st.last_runs.map(r => `<li class="${/^(non lancé|manqué|interrompu)/.test(r.result) ? 'warn' : ''}">${esc(r.when)} — ${esc(r.entry)} — ${esc(r.result)}</li>`).join('')
      : '<li>Aucun déclenchement pour le moment.</li>';
  }
  document.querySelectorAll('#agList li').forEach(li => {
    const e = AG.agenda.entries[+li.dataset.i];
    li.classList.toggle('next', !!(e && next && e.id === next.id));
  });
}
function agEdit(i, patch) {
  const entries = AG.agenda.entries.map((e, k) => k === i ? { ...e, ...patch } : e);
  saveAgenda({ ...AG.agenda, entries });
}
$('agEnabled').addEventListener('click', () => saveAgenda({ ...AG.agenda, enabled: !AG.agenda.enabled }));
$('agAdd').addEventListener('click', () => {
  const n = AG.agenda.entries.length + 1;
  saveAgenda({ ...AG.agenda, entries: [...AG.agenda.entries, { id: 0, name: 'Entrée ' + n, enabled: true, target: { kind: 'show_list' }, time: '20:30', days: [true, true, true, true, true, true, true], date: null }] });
});
$('agList').addEventListener('click', e => {
  const b = e.target.closest('button');
  if (!b) return;
  if (b.dataset.del != null) {
    const i = +b.dataset.del;
    saveAgenda({ ...AG.agenda, entries: AG.agenda.entries.filter((_, k) => k !== i) });
  } else if (b.dataset.day != null) {
    const i = +b.dataset.i, d = +b.dataset.day;
    agEdit(i, { days: AG.agenda.entries[i].days.map((v, k) => k === d ? !v : v) });
  }
});
$('agList').addEventListener('focusin', async e => {
  const sel = e.target.closest('select.ag-target');
  if (!sel || sel.dataset.fresh) return;
  try { shows = await apiJson('GET', '/api/shows', undefined, { quiet: true }); } catch { return; }
  const cur = sel.value;
  const opts = [['show_list', 'Lecteur de shows (liste)'], ...shows.map(s => ['show:' + s.name, 'Show : ' + s.name]), ['stop', 'Arrêt et noir (désarme)']];
  if (!opts.some(o => o[0] === cur)) opts.push([cur, sel.selectedOptions[0] ? sel.selectedOptions[0].textContent : cur]);
  sel.innerHTML = opts.map(([v, l]) => `<option value="${esc(v)}"${v === cur ? ' selected' : ''}>${esc(l)}</option>`).join('');
  sel.dataset.fresh = '1';
});
$('agList').addEventListener('change', e => {
  const c = e.target.closest('[data-f]');
  if (!c) return;
  const i = +c.dataset.i, f = c.dataset.f;
  if (f === 'enabled') agEdit(i, { enabled: c.checked });
  else if (f === 'target') agEdit(i, { target: agTarget(c.value) });
  else if (f === 'mode') {
    const today = new Date();
    const iso = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, '0')}-${String(today.getDate()).padStart(2, '0')}`;
    agEdit(i, { date: c.value === 'date' ? iso : null });
  } else if (f === 'time') { if (c.value) agEdit(i, { time: c.value.slice(0, 5) }); }
  else if (f === 'date') { if (c.value) agEdit(i, { date: c.value }); }
  else if (f === 'name') agEdit(i, { name: c.value });
});
$('slList').addEventListener('dragstart', e => {
  const g = e.target.closest('.sl-grip');
  if (!g) return;
  SL.drag = +g.dataset.i;
  e.dataTransfer.effectAllowed = 'move';
  e.dataTransfer.setData('text/plain', String(SL.drag));
});
$('slList').addEventListener('dragover', e => {
  const li = e.target.closest('li');
  if (SL.drag == null || !li) return;
  e.preventDefault();
  document.querySelectorAll('#slList li.drop').forEach(x => x.classList.remove('drop'));
  li.classList.add('drop');
});
$('slList').addEventListener('drop', e => {
  const li = e.target.closest('li');
  document.querySelectorAll('#slList li.drop').forEach(x => x.classList.remove('drop'));
  if (SL.drag == null || !li) return;
  e.preventDefault();
  const from = SL.drag;
  SL.drag = null;
  slMove(from, +li.dataset.i);
});
$('slList').addEventListener('dragend', () => { SL.drag = null; document.querySelectorAll('#slList li.drop').forEach(x => x.classList.remove('drop')); });
loadShowList();

// ---------- Timecode entrant (T-168) ----------
// The summary comes with every frame (`f.timecode`); the settings, MIDI
// ports and audio channels from GET /api/timecode when they may have
// changed. TC-IN is a live switch (allowed in show mode); it never arms
// the laser.
let tcState = null, tcCfg = null, tcView = null, lastTimeline = null;
function assignLastTimeline(v) { return (lastTimeline = v); }
const TC_LED_FR = { off: 'Pas de source', no_signal: 'Pas de timecode', preroll: 'Pré-roll', locked: 'Suivi', afterroll: 'Post-roll', stopped: 'Timecode à l\'arrêt', lost: 'Timecode perdu' };
function showTimecode(t) {
  if (!t) return;
  tcState = t;
  const st = t.source === 'off' ? 'off' : t.status;
  $('tcArm').classList.toggle('on', t.armed);
  $('tcArm').setAttribute('aria-pressed', t.armed);
  $('tcLed').dataset.status = st;
  $('tcLed').setAttribute('aria-label', TC_LED_FR[st] || st);
  $('tcReadout').dataset.status = st;
  $('tcReadout').textContent = t.tc ? 'TC ' + t.tc : '--:--:--:--';
  const parts = [t.status_fr];
  if (t.rate_fr) parts.push(t.rate_fr);
  if (t.chasing) parts.push('pilote la timeline');
  else if (t.armed && t.blocked) parts.push(t.blocked);
  else if (!t.armed && t.source !== 'off') parts.push('TC-IN désarmé');
  $('tcStatus').textContent = parts.join(' · ');
  $('tcStatus').classList.toggle('warn', ['preroll', 'afterroll', 'lost'].includes(t.status) || (t.armed && !!t.blocked && !t.chasing));
  if (tcView && (tcView.source !== t.source || tcView.per_show !== t.per_show)) loadTimecode();
  showTcBig();
}
// Grand afficheur (T-409): the timecode received, else the timeline's
// position; green followed, orange pre-roll / post-roll, red lost.
let tcBigOn = false;
function fmtPosTc(secs) {
  const n = Math.floor(Math.max(0, secs) * 25 + 1e-6);
  const p = v => String(v).padStart(2, '0');
  return `${p(Math.floor(n / 90000) % 24)}:${p(Math.floor(n / 1500) % 60)}:${p(Math.floor(n / 25) % 60)}:${p(n % 25)}`;
}
function showTcBig() {
  if (!tcBigOn) return;
  const t = tcState, tl = lastTimeline;
  const on = t && t.source !== 'off';
  $('tcBigTc').textContent = on && t.tc ? t.tc : tl && tl.name && tl.time_base === 'seconds' ? fmtPosTc(tl.position) : '--:--:--:--';
  $('tcBig').dataset.state = on ? t.state : 'off';
  $('tcBigState').textContent = !on ? 'Position de la timeline' : t.status_fr + (t.rate_fr ? ' · ' + t.rate_fr : '');
  $('tcBigShow').textContent = tl && tl.name ? tl.name : 'Aucun show';
}
function toggleTcBig(on = !tcBigOn) {
  tcBigOn = on;
  $('tcBig').hidden = !on;
  showTcBig();
}
$('tcBigBtn').onclick = () => toggleTcBig(true);
$('tcBig').onclick = () => toggleTcBig(false);
function fillTimecode(v) {
  tcCfg = v.config;
  tcView = { source: v.source, per_show: v.per_show };
  const c = v.config;
  // The loaded show's own source, rate and offset, or the global ones.
  const e = v.show || c;
  const keep = id => document.activeElement === $(id);
  $('tcPerShow').checked = !!v.show;
  $('tcPerShow').disabled = !v.show_name;
  $('tcPerShow').parentElement.title = v.show_name ? `Source, images/s et décalage enregistrés dans « ${v.show_name} » (sinon : le réglage général)` : 'Chargez un show pour lui donner son propre timecode';
  if (!keep('tcSource')) $('tcSource').value = e.source;
  if (!keep('tcFps')) $('tcFps').value = e.fps;
  if (!keep('tcOffset')) $('tcOffset').value = +e.offset_s.toFixed(3);
  if (!keep('tcPreroll')) $('tcPreroll').value = c.pre_roll_frames;
  if (!keep('tcAfter')) $('tcAfter').value = c.after_roll_s ?? 1;
  $('tcAfter').disabled = c.after_roll_s === null;
  $('tcSmooth').checked = c.smooth;
  $('tcKeep').checked = c.after_roll_s === null;
  $('tcPortRow').hidden = v.source !== 'mtc';
  $('tcChanRow').hidden = v.source !== 'ltc';
  $('tcIfaceRow').hidden = v.source !== 'artnet';
  if (!keep('tcIface')) {
    const ifs = (v.artnet && v.artnet.interfaces) || [];
    const ips = [...new Set([...ifs.map(i => i.ip), ...(c.interface ? [c.interface] : [])])];
    $('tcIface').innerHTML = '<option value="">Aucune</option>' + ips.map(ip => { const i = ifs.find(x => x.ip === ip); return `<option value="${esc(ip)}">${esc(ip)}${i ? ' (' + esc(i.name) + ')' : ''}</option>`; }).join('');
    $('tcIface').value = c.interface || '';
  }
  if (!keep('tcPort')) {
    const ports = [...new Set([...(v.midi_ports || []), ...(c.midi_port ? [c.midi_port] : [])])];
    $('tcPort').innerHTML = '<option value="">Tous les ports</option>' + ports.map(p => `<option>${esc(p)}</option>`).join('');
    $('tcPort').value = c.midi_port || '';
  }
  if (!keep('tcChan')) {
    const n = Math.max(v.audio.channels || 0, c.ltc_channel, 2);
    $('tcChan').innerHTML = '<option value="0">Mixage de toutes les voies</option>' + Array.from({ length: n }, (_, i) => `<option value="${i + 1}">Voie ${i + 1}</option>`).join('');
    $('tcChan').value = String(c.ltc_channel);
  }
  $('tcMsg').textContent = v.source === 'ltc' && !v.audio.native
    ? 'LTC : choisissez l\'entrée audio native (Musique › Source) pour que le studio écoute l\'entrée.'
    : v.source === 'artnet' && !c.interface && !(v.artnet && v.artnet.chart)
    ? `Art-Net : choisissez l'interface réseau où écouter (port ${v.artnet ? v.artnet.port : 6454}).`
    : v.source !== 'off' && !v.armed ? 'Pour piloter la timeline : armer TC-IN, ouvrir les entrées externes (périmètre « Timeline » du timecode) et charger un show en secondes.' : '';
  showTimecode(v);
}
async function loadTimecode() {
  try { const r = await api('GET', '/api/timecode'); if (r.ok) fillTimecode(await r.json()); } catch { /* studio restarting */ }
}
async function setTimecode(patch) {
  const r = await api('POST', '/api/timecode', patch, { quiet: true });
  if (r.ok) { $('tcMsg').textContent = ''; fillTimecode(await r.json()); return; }
  const t = await r.text();
  // Reload first (it rewrites the hint), then say why it was refused.
  await loadTimecode();
  if (r.status === 423) flash(t); else $('tcMsg').textContent = t;
}
$('tcArm').onclick = async () => {
  try {
    const r = await api('POST', '/api/timecode/arm', { on: !(tcState && tcState.armed), source: 'ui' }, { quiet: true });
    const j = await r.json().catch(() => null);
    if (!r.ok) flash(j?.error || 'Refusé');
    else fillTimecode(j);
  } catch {}
};
// Source, rate and offset go to the show when it has its own.
function tcShowFields() {
  return { source: $('tcSource').value, fps: $('tcFps').value, offset_s: +$('tcOffset').value || 0 };
}
function setTcShowOr(key, value) {
  if (!$('tcPerShow').checked) return setTimecode({ [key]: value });
  setTimecode({ show: { ...tcShowFields(), [key]: value } });
}
$('tcPerShow').onchange = e => setTimecode({ show: e.target.checked ? tcShowFields() : null });
$('tcSource').onchange = e => setTcShowOr('source', e.target.value);
$('tcIface').onchange = e => setTimecode({ interface: e.target.value });
$('tcPort').onchange = e => setTimecode({ midi_port: e.target.value || null });
$('tcChan').onchange = e => setTimecode({ ltc_channel: +e.target.value });
$('tcFps').onchange = e => setTcShowOr('fps', e.target.value);
$('tcOffset').onchange = e => { const v = +e.target.value; if (Number.isFinite(v)) setTcShowOr('offset_s', v); };
$('tcPreroll').onchange = e => setTimecode({ pre_roll_frames: Math.round(+e.target.value) });
$('tcAfter').onchange = e => { const v = +e.target.value; if (Number.isFinite(v)) setTimecode({ after_roll_s: v }); };
$('tcSmooth').onchange = e => setTimecode({ smooth: e.target.checked });
$('tcKeep').onchange = e => setTimecode({ after_roll_s: e.target.checked ? null : (+$('tcAfter').value || 1) });
$('tcOffsetNow').onclick = () => {
  // hh:mm:ss:ff → seconds at the rate shown (drop frame: close enough
  // for a start mark; the field can be fine-tuned).
  const m = tcState && tcState.tc && tcState.tc.match(/^(\d+):(\d+):(\d+):(\d+)$/);
  if (!m) return flash('Aucun timecode reçu');
  const fps = { '24': 24, '25': 25, '29.97df': 30000 / 1001, '30': 30 }[tcState.rate] || 25;
  setTcShowOr('offset_s', Math.round(((+m[1] * 60 + +m[2]) * 60 + +m[3] + +m[4] / Math.round(fps)) * 1000) / 1000);
};
$('timelinePanel').addEventListener('focusin', () => { if (!tcCfg) loadTimecode(); });
setInterval(() => { if (!document.hidden && !$('timelinePanel').hidden) { loadTimecode(); loadTcOut(); } }, 2000);
loadTimecode();

// ---------- Timecode sortant (T-417) ----------
// TC-OUT sends the timeline's position (MTC on the MIDI port chosen,
// Art-Net from the interface chosen) while a show in seconds plays.
// Off by default; the summary comes with every frame
// (`timeline.timecode_out`), the settings from GET /api/timecode/out.
// Never arms the laser; not an external control (no shortcut).
let tcoCfg = null;
function showTcOut(o) {
  if (!o) return;
  $('tcoToggle').classList.toggle('on', o.enabled);
  $('tcoToggle').setAttribute('aria-pressed', o.enabled);
  const st = !o.enabled ? 'off' : o.error ? 'error' : o.sending ? 'sending' : o.blocked ? 'waiting' : 'idle';
  $('tcoState').dataset.status = st;
  $('tcoState').textContent = !o.enabled ? 'Émission : désactivée'
    : o.sending ? 'Émission : ' + (o.last || '…')
    : o.blocked ? 'Émission en attente : ' + o.blocked
    : 'Émission : à l\'arrêt (timeline arrêtée)';
  $('tcoState').title = o.error ? 'Dernière erreur d\'envoi : ' + o.error : '';
}
function fillTcOut(v) {
  const c = v.config;
  tcoCfg = c;
  const keep = id => document.activeElement === $(id);
  if (!keep('tcoPort')) {
    const ports = [...new Set([...(v.midi_ports || []), ...(c.mtc_port ? [c.mtc_port] : [])])];
    $('tcoPort').innerHTML = '<option value="">Aucun</option>' + ports.map(p => `<option>${esc(p)}</option>`).join('');
    $('tcoPort').value = c.mtc_port || '';
  }
  $('tcoArtnet').checked = !!c.artnet;
  document.querySelectorAll('#tcoBar .tco-net').forEach(el => { el.hidden = !c.artnet; });
  if (!keep('tcoIface')) {
    const ifs = v.interfaces || [];
    const cur = c.artnet ? c.artnet.interface : '';
    const ips = [...new Set([...ifs.map(i => i.ip), ...(cur ? [cur] : [])])];
    $('tcoIface').innerHTML = ips.map(ip => { const i = ifs.find(x => x.ip === ip); return `<option value="${esc(ip)}">${esc(ip)}${i ? ' (' + esc(i.name) + ')' : ''}</option>`; }).join('');
    $('tcoIface').value = cur || (ips[0] || '');
  }
  if (!keep('tcoTarget')) $('tcoTarget').value = c.artnet ? c.artnet.target : '';
  if (!keep('tcoFps')) $('tcoFps').value = c.fps;
  if (!keep('tcoOffset')) $('tcoOffset').value = +c.offset_s.toFixed(3);
  showTcOut(v.summary);
}
async function loadTcOut() {
  try { const r = await api('GET', '/api/timecode/out', undefined, { quiet: true }); if (r.ok) fillTcOut(await r.json()); } catch { /* studio restarting */ }
}
async function setTcOut(patch) {
  const r = await api('POST', '/api/timecode/out', patch, { quiet: true });
  if (r.ok) { $('tcoMsg').textContent = ''; fillTcOut(await r.json()); return; }
  const t = await r.text();
  if (r.status === 423) flash(t); else $('tcoMsg').textContent = t;
  loadTcOut();
}
function tcoNet() {
  return { interface: $('tcoIface').value, target: $('tcoTarget').value.trim() };
}
$('tcoToggle').onclick = () => setTcOut({ enabled: !(tcoCfg && tcoCfg.enabled) });
$('tcoPort').onchange = e => setTcOut({ mtc_port: e.target.value || null });
$('tcoArtnet').onchange = e => {
  if (!e.target.checked) return setTcOut({ artnet: null });
  // Loopback has no broadcast: start from a typed target there.
  const n = tcoNet();
  if (!n.interface) { e.target.checked = false; $('tcoMsg').textContent = 'Aucune interface réseau disponible.'; return; }
  setTcOut({ artnet: n });
};
$('tcoIface').onchange = () => setTcOut({ artnet: tcoNet() });
$('tcoTarget').onchange = () => setTcOut({ artnet: tcoNet() });
$('tcoFps').onchange = e => setTcOut({ fps: e.target.value });
$('tcoOffset').onchange = e => { const v = +e.target.value; if (Number.isFinite(v)) setTcOut({ offset_s: v }); };
loadTcOut();

$('tlShow').addEventListener('focus', loadShows);
$('tlShow').addEventListener('change', async () => {
  $('tlMsg').textContent = '';
  // The editor's unsaved changes (T-162) would be lost.
  if (!dropShowChanges()) { $('tlShow').value = (timeline && timeline.name) || ''; return; }
  if ($('tlShow').value) await post('/api/timeline/load', { name: $('tlShow').value });
  else await post('/api/timeline/stop');
});
$('tlPlay').onclick = async () => {
  const r = await api('POST', '/api/timeline/play', undefined, { quiet: true });
  $('tlMsg').textContent = r.ok ? '' : await r.text();
};
$('tlLoop').onclick = () => post('/api/timeline/loop', { on: !(timeline && timeline.loop) });

// --- Enregistrer le jeu en direct (T-169) ---
// The studio observes the cues and the master moves while the timeline
// plays (take.rs); it never arms, never changes what plays. The take is
// laid out by POST /api/timeline/record/take (changes nothing) and goes
// out through tlChange like any edit: one undo step, locked tracks and
// show mode refused by /api/timeline/edit.
let takeView = null;
function showTake(k) {
  if (!k) return;
  takeView = k;
  const b = $('tlTakeRec');
  const rec = k.state === 'recording', wait = k.state === 'waiting';
  b.classList.toggle('rec', rec); b.classList.toggle('wait', wait);
  b.setAttribute('aria-pressed', String(rec || wait));
  const label = rec ? `■ Arrêter l'enregistrement (${k.events} cue${k.events > 1 ? 's' : ''})` : wait ? '● En attente de la lecture…' : '● Enregistrer';
  if (b.textContent !== label) b.textContent = label;
  const t = k.take;
  $('tlTakeRow').hidden = !t;
  if (t) {
    const env = t.envelopes.length ? ` · ${t.envelopes.length} réglage(s) : ${t.envelopes.map(x => x.replace('master.', '')).join(', ')}` : '';
    const text = `Prise sur « ${t.show} » (${t.reason_fr}) : ${t.events} cue(s)${env}`;
    if ($('tlTakeInfo').textContent !== text) $('tlTakeInfo').textContent = text;
  }
}
$('tlTakeRec').onclick = async () => {
  const k = takeView;
  const busy = k && (k.state === 'recording' || k.state === 'waiting');
  const r = await api('POST', busy ? '/api/timeline/record/stop' : '/api/timeline/record/start', {}, { quiet: true });
  if (!r.ok) { tlMsg(await r.text(), true); return; }
  showTake(await r.json());
  if (!busy) tlMsg(timeline && timeline.playing ? 'Enregistrement en cours.' : 'Enregistrement prêt : il démarre avec la lecture.');
};
$('tlTakeKeep').onclick = async () => {
  if (!TL.show) { tlMsg('Ouvrez le show de la prise d\'abord.', true); return; }
  const r = await api('POST', '/api/timeline/record/take', { show: TL.show, quantize: $('tlTakeQ').value }, { quiet: true });
  if (!r.ok) { tlMsg(await r.text(), true); return; }
  const j = await r.json();
  if (!tlChange(() => { TL.show = j.show; })) { tlMsg('Rien n\'a été ajouté.', true); return; }
  TL.headsKey = ''; tlHeads();
  const d = await api('POST', '/api/timeline/record/discard', {}, { quiet: true });
  if (d.ok) showTake(await d.json());
  tlMsg(`Prise ${j.number} gardée : ${j.added} cue(s) sur ${j.tracks.join(', ')}${j.skipped ? ` (${j.skipped} cue(s) supprimé(s) depuis, ignoré(s))` : ''}. Annuler la retire.`);
};
$('tlTakeDrop').onclick = async () => {
  if (!confirm('Jeter cette prise ? Elle ne pourra pas être récupérée.')) return;
  const r = await api('POST', '/api/timeline/record/discard', {}, { quiet: true });
  if (r.ok) { showTake(await r.json()); tlMsg('Prise jetée.'); }
};
function seekFromClick(el, e) {
  if (!timeline || !timeline.length) return;
  const b = el.getBoundingClientRect();
  post('/api/timeline/seek', { position: timeline.length * Math.max(0, Math.min(1, (e.clientX - b.left) / b.width)) });
}
$('tlBar').addEventListener('click', e => seekFromClick($('tlBar'), e));
loadShows();

// ---------- timeline audio (T-161) ----------
// The show's song: import, pick, offset, volume, and its waveform (peaks
// computed and cached by the studio). The studio plays it itself on the
// Mac's output and the timeline follows the song's clock; the page only
// shows it.
let songs = [], waveKey = '', waveBusy = false;
async function loadSongs() {
  try { songs = (await apiJson('GET', '/api/media/audio')).files; } catch { songs = []; }
  fillSongSelect();
  fillAutoSongs();
}
function fillSongSelect() {
  const cur = timeline && timeline.audio ? timeline.audio.file : '';
  $('tlSong').innerHTML = '<option value="">Aucun morceau</option>' + songs.map(s => `<option>${esc(s.file)}</option>`).join('');
  if (cur && !songs.some(s => s.file === cur)) $('tlSong').insertAdjacentHTML('beforeend', `<option>${esc(cur)}</option>`);
  $('tlSong').value = cur;
}
const SONG_STATE = {
  disabled: () => 'lecture audio désactivée (--no-audio) : horloge système',
  idle: () => '',
  loading: () => 'chargement du morceau…',
  ready: st => `lecture sur ${st.device || 'la sortie par défaut'} · horloge ${st.clock === 'audio' ? 'du morceau' : 'système'}`,
  error: st => 'morceau illisible : ' + st.message,
  no_device: st => 'pas de sortie audio (' + st.message + ') : horloge système',
};
function showSong(t, st) {
  if (!t) return;
  const a = t.audio, seconds = !!t.name && t.time_base === 'seconds';
  $('tlSong').disabled = !seconds;
  $('tlImport').title = seconds ? 'WAV, MP3, AIFF ou FLAC : copié dans studio-data/media/audio, jamais ailleurs' : 'Importer dans studio-data/media/audio (chargez un show en secondes pour l\'y associer)';
  $('tlOffset').disabled = $('tlGain').disabled = !a;
  if (document.activeElement !== $('tlSong') && $('tlSong').value !== (a ? a.file : '')) fillSongSelect();
  if (document.activeElement !== $('tlOffset')) $('tlOffset').value = a ? Math.round(a.offset_s * 1000) : 0;
  if (document.activeElement !== $('tlGain')) $('tlGain').value = a ? Math.round(a.gain * 100) : 100;
  $('tlOffsetV').textContent = `${$('tlOffset').value > 0 ? '+' : ''}${$('tlOffset').value} ms`;
  $('tlGainV').textContent = `${$('tlGain').value} %`;
  const text = a && st ? (SONG_STATE[st.state] || (() => st.state))(st) : '';
  $('tlSongStatus').textContent = text;
  $('tlSongStatus').classList.toggle('bad', !!st && (st.state === 'error' || st.state === 'no_device'));
  $('tlWaveHead').style.left = (t.length > 0 ? Math.min(100, 100 * t.position / t.length) : 0) + '%';
  $('tlWaveEmpty').hidden = !!a;
  if (!$('tlWave').clientWidth) return; // workspace hidden: drawn when shown
  const w = Math.round($('tlWave').clientWidth * (window.devicePixelRatio || 1));
  const key = a ? `${a.file}|${a.offset_s}|${t.length}|${w}` : '';
  if (key !== waveKey && !waveBusy) drawWave(key, a, t.length, w);
}
async function drawWave(key, a, length, w) {
  const c = $('tlWave'), h = Math.max(1, Math.round(c.clientHeight * (window.devicePixelRatio || 1)));
  c.width = w; c.height = h;
  const g = c.getContext('2d');
  g.clearRect(0, 0, w, h);
  waveKey = key;
  if (!a || !(length > 0)) return;
  waveBusy = true;
  try {
    const r = await api('GET', `/api/timeline/waveform?from=0&to=${length}&px=${w}`);
    if (!r.ok) return; // file missing or unreadable: not asked again until something changes
    const wave = await r.json();
    g.strokeStyle = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() || '#3ddc84';
    g.globalAlpha = 0.75;
    g.beginPath();
    for (let x = 0; x < wave.max.length; x++) {
      const top = h / 2 - wave.max[x] * h / 2, bottom = h / 2 - wave.min[x] * h / 2;
      g.moveTo(x + 0.5, top);
      g.lineTo(x + 0.5, Math.max(bottom, top + 1));
    }
    g.stroke();
    c.dataset.file = wave.file;
  } catch { waveKey = ''; } finally { waveBusy = false; }
}
async function setSong(patch) {
  const r = await api('POST', '/api/timeline/audio', patch, { quiet: true });
  $('tlMsg').textContent = r.ok ? '' : await r.text();
  return r.ok;
}
$('tlSong').addEventListener('focus', loadSongs);
$('tlSong').addEventListener('change', () => setSong({ file: $('tlSong').value || null }));
$('tlOffset').addEventListener('input', () => { $('tlOffsetV').textContent = `${$('tlOffset').value > 0 ? '+' : ''}${$('tlOffset').value} ms`; });
$('tlOffset').addEventListener('change', () => setSong({ offset_s: $('tlOffset').value / 1000 }));
$('tlGain').addEventListener('input', () => { $('tlGainV').textContent = `${$('tlGain').value} %`; });
$('tlGain').addEventListener('change', () => setSong({ gain: $('tlGain').value / 100 }));
$('tlImport').onclick = () => $('tlImportFile').click();
$('tlImportFile').addEventListener('change', async () => {
  const f = $('tlImportFile').files[0];
  $('tlImportFile').value = '';
  if (!f) return;
  $('tlMsg').textContent = `Import de « ${f.name} »…`;
  let r;
  try { r = await api('POST', '/api/media/audio?name=' + encodeURIComponent(f.name), f, { quiet: true }); }
  catch { $('tlMsg').textContent = 'Import impossible : studio injoignable'; return; }
  if (!r.ok) { $('tlMsg').textContent = 'Import refusé : ' + await r.text(); return; }
  const info = await r.json();
  await loadSongs();
  if (timeline && timeline.name && timeline.time_base === 'seconds') {
    if (await setSong({ file: info.file })) $('tlMsg').textContent = `Morceau « ${info.file} » (${fmtTime(info.duration_s)}) ajouté au show.`;
  } else {
    $('tlMsg').textContent = `Morceau « ${info.file} » importé : chargez un show en secondes pour l'y associer.`;
  }
});
$('tlWaveBox').addEventListener('click', e => seekFromClick($('tlWaveBox'), e));
loadSongs();

// ---------- show from the song (T-167) ----------
// The studio decodes and analyses the song on its own thread (tempo,
// beats, sections) and opens a proposed show, unsaved; the page starts,
// follows and cancels the job. Same song = same proposal; « Autre
// proposition » asks for another seed.
let autoPoll = null, autoSeed = null;
const AUTO_KIND = { intro: 'intro', verse: 'couplet', buildup: 'montée', drop: 'drop', break: 'pause', outro: 'fin' };
function fillAutoSongs() {
  const sel = $('tlAutoSong');
  const cur = sel.value || (timeline && timeline.audio ? timeline.audio.file : '') || (songs[0] ? songs[0].file : '');
  sel.innerHTML = '<option value="">Aucun morceau</option>' + songs.map(s => `<option>${esc(s.file)}</option>`).join('');
  sel.value = songs.some(s => s.file === cur) ? cur : '';
  autoButtons();
}
function autoButtons(running = !!autoPoll) {
  $('tlAuto').disabled = running || !$('tlAutoSong').value;
  $('tlAutoSong').disabled = running;
  $('tlAutoCancel').hidden = !running;
  $('tlAutoProg').hidden = !running;
  if (running) $('tlAutoAgain').hidden = true;
}
function autoMsg(html, err = false) { $('tlAutoMsg').innerHTML = html; $('tlAutoMsg').classList.toggle('err', err); }
function showAutoJob(j) {
  if (j.state === 'running') {
    $('tlAutoProg').value = j.step === 'analyse' ? j.progress : 0;
    autoMsg(`${esc(j.step)}${j.step === 'analyse' ? ' · ' + Math.round(j.progress * 100) + ' %' : ''}…`);
    return;
  }
  clearInterval(autoPoll); autoPoll = null; autoButtons(false);
  if (j.state === 'done') {
    const a = j.analysis, n = {};
    a.sections.forEach(s => { n[s.kind] = (n[s.kind] || 0) + 1; });
    const parts = Object.keys(AUTO_KIND).filter(k => n[k]).map(k => `${n[k]} ${AUTO_KIND[k]}`).join(', ');
    autoSeed = j.seed;
    autoMsg(`Show <b>« ${esc(j.show)} »</b> proposé, non enregistré : ${a.bpm.toFixed(1).replace('.', ',')} BPM${a.confidence < 0.2 ? ' (tempo incertain)' : ''} · ${parts} · graine ${j.seed}. Retouchez-le puis Enregistrer.`);
    $('tlAutoAgain').hidden = false;
    loadShows();
  } else if (j.state === 'failed') autoMsg('Génération impossible : ' + esc(j.message), true);
  else if (j.state === 'cancelled') autoMsg('Génération annulée : rien n\'a été ouvert.');
}
async function autoTick() {
  try { showAutoJob(await apiJson('GET', '/api/timeline/autoshow')); } catch { /* next tick */ }
}
async function autoStart(seed) {
  const file = $('tlAutoSong').value;
  if (!file) return;
  if (!dropShowChanges('le show proposé le remplacera')) return;
  const body = seed == null ? { file } : { file, seed };
  let r;
  try { r = await api('POST', '/api/timeline/autoshow', body, { quiet: true }); }
  catch { autoMsg('Studio injoignable', true); return; }
  if (!r.ok) { autoMsg(esc(await r.text()), true); return; }
  autoPoll = setInterval(autoTick, 250);
  autoButtons(true);
  showAutoJob(await r.json());
}
$('tlAutoSong').addEventListener('focus', loadSongs);
$('tlAutoSong').addEventListener('change', () => { $('tlAutoAgain').hidden = true; autoButtons(); });
$('tlAuto').onclick = () => autoStart(null);
$('tlAutoAgain').onclick = () => autoStart(((autoSeed || 0) + 1 + Math.floor(Math.random() * 1e6)) % 4294967296);
$('tlAutoCancel').onclick = () => post('/api/timeline/autoshow/cancel');
// A job started earlier (another tab, a reload) is followed too.
apiJson('GET', '/api/timeline/autoshow').then(j => {
  if (j.state === 'running') { autoPoll = setInterval(autoTick, 250); autoButtons(true); }
}).catch(() => {});

export { SL, assignLastTimeline, fillShowSelect, fmtTime, lastTimeline, loadShows, loadSongs,
  showAgenda, showShowList, showSong, showTcOut, showTimecode, showTimeline, shows, slAct, timeline,
  toggleTcBig };
