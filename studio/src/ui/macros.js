// Laser Studio — macros.js (T-435).
// Sections (as in the former index.html script):
//   macros (T-425)
//   scripts (T-458)
//
// Moved as is from index.html. Only the import lines below and the
// export list at the end were added; a top-level variable another module
// assigns is written through its assign<Name>() function, declared next
// to it. The first import is the module before this one: every module
// imports its predecessor and app.js imports the last one, so the ES
// evaluation order is the page order the script had.
import { assignMenuCue, menuCue, menuSlot } from './presets.js';
import { $, api, apiJson, rgb } from './core.js';
import { IS_MAC, MACRO_LETTERS } from './laser.js';
import { LOCKED_FR, activeWs } from './workspaces.js';
import { gridPage, gridPost, loadPresets, presetById } from './cues.js';

// ---------- macros (T-425) ----------
// LIVE › Macros: one pad per macro (click = launch, again = from the
// start, ■ = stop, Go for the ones waiting). CRÉATION › Macros: the list
// edited as a draft, then « Enregistrer » sends it whole (POST
// /api/macros): every line is checked by the studio, a refused one (arming,
// safety) or an unknown word is shown under its step. The frame carries
// the list's revision and what runs (`f.macros`).
const MAC = { list: [], draft: [], sel: 0, dirty: false, rev: -1, running: [], runKey: '', err: null, cmds: [] };
const macClone = v => JSON.parse(JSON.stringify(v));
function macroForKey(letter) { return MAC.list.find(m => m.enabled && m.key === letter); }
function macWaitText(w) {
  if (!w) return '';
  if (w.kind === 'go') return 'attend Go';
  return `encore ${String(w.left).replace('.', ',')} ${w.kind === 'beats' ? 'temps' : 's'}`;
}
async function loadMacros(force) {
  const j = await apiJson('GET', '/api/macros', undefined, { cache: 'no-store', quiet: true }).catch(() => null);
  if (!j) return;
  MAC.list = j.macros; MAC.rev = j.rev;
  if (!MAC.dirty || force === true && !MAC.draft.length) { MAC.draft = macClone(j.macros); MAC.dirty = false; MAC.sel = Math.min(MAC.sel, Math.max(0, MAC.draft.length - 1)); }
  if (j.warnings && j.warnings.length) $('macState').textContent = j.warnings.join(' · ');
  renderMacButtons();
  renderMacEdit();
}
function showMacros(m) {
  if (!m) return;
  if (m.rev !== MAC.rev) { MAC.rev = m.rev; loadMacros(); }
  const key = JSON.stringify(m.running);
  if (key === MAC.runKey) return;
  MAC.runKey = key; MAC.running = m.running;
  renderMacButtons();
}
function renderMacButtons() {
  const box = $('macButtons');
  const pads = MAC.list.map(m => {
    const run = MAC.running.find(r => r.id === m.id);
    const pad = document.createElement('div');
    pad.className = 'mac-pad';
    const b = document.createElement('button');
    b.className = 'mac-btn' + (run ? ' on' : '') + (run && run.go ? ' wait' : '');
    b.dataset.mac = m.id;
    b.dataset.control = `macro.${m.id}.go`;
    b.disabled = !m.enabled;
    b.title = `macro.${m.id}.go : ${run ? 'relancer depuis la première étape' : 'lancer'}${m.enabled ? '' : ' (désactivée)'}`;
    const n = document.createElement('span'); n.className = 'n'; n.textContent = m.name; b.appendChild(n);
    if (m.key) { const k = document.createElement('kbd'); k.textContent = (IS_MAC ? '⌥' : 'Alt+') + m.key; b.appendChild(k); }
    if (run) {
      const pr = document.createElement('span'); pr.className = 'prog';
      pr.style.width = (m.steps.length ? Math.min(100, run.step / m.steps.length * 100) : 100) + '%';
      b.appendChild(pr);
    }
    b.onclick = () => macRun(m.id, 'ui');
    pad.appendChild(b);
    if (run) {
      const st = document.createElement('button');
      st.className = 'mac-stop'; st.textContent = '■'; st.setAttribute('aria-label', `Arrêter la macro ${m.name}`);
      st.onclick = () => macStop(m.id);
      pad.appendChild(st);
    }
    return pad;
  });
  if (!pads.length) { const e = document.createElement('div'); e.className = 'mac-empty'; e.textContent = 'Aucune macro : les créer dans CRÉATION › Macros'; pads.push(e); }
  box.replaceChildren(...pads);
  $('macGo').hidden = !MAC.running.some(r => r.go);
}
async function macRun(id, source) {
  const r = await api('POST', '/api/macros/run', { id, source }, { quiet: true });
  $('macMsg').textContent = r.ok ? '' : await r.text();
}
async function macStop(id) {
  await api('POST', '/api/macros/stop', id ? { id } : {}, { quiet: true });
}
$('macGo').onclick = () => api('POST', '/api/macros/go', {}, { quiet: true });
$('macStopAll').onclick = () => macStop(null);
// The editor.
const MAC_WAITS = [['none', 'Aucune'], ['seconds', 'Secondes'], ['beats', 'Temps'], ['go', 'Attendre Go']];
const waitKind = w => typeof w === 'string' ? w : Object.keys(w || {})[0] || 'none';
const waitVal = w => typeof w === 'object' && w ? Object.values(w)[0] : '';
async function macLoadCommands() {
  const j = await apiJson('GET', '/api/command/actions', undefined, { cache: 'no-store', quiet: true }).catch(() => null);
  if (!j) return;
  MAC.cmds = [...new Set(j.actions.map(a => a.command))];
  macFillCommands();
}
function macFillCommands() {
  const names = MAC.draft.map(m => `macro "${m.name}" go`);
  $('macCmdList').replaceChildren(...[...MAC.cmds, ...names].map(c => { const o = document.createElement('option'); o.value = c; return o; }));
}
function macTouched() {
  MAC.dirty = true;
  $('macState').textContent = 'Modifications non enregistrées : « Enregistrer » pour les garder';
}
function macCur() { return MAC.draft[MAC.sel]; }
function renderMacEdit() {
  const sel = $('macSel');
  sel.replaceChildren(...MAC.draft.map((m, i) => { const o = document.createElement('option'); o.value = i; o.textContent = (m.name || '(sans nom)') + (m.enabled ? '' : ' (désactivée)'); return o; }));
  if (!MAC.draft.length) { const o = document.createElement('option'); o.textContent = 'Aucune macro'; o.value = ''; sel.appendChild(o); }
  sel.value = MAC.draft.length ? String(MAC.sel) : '';
  const m = macCur();
  $('macFields').hidden = !m;
  $('macDup').disabled = $('macDel').disabled = !m;
  const err = MAC.err;
  const errOf = m && err && err.macro_id === m.id;
  $('macErr').hidden = !(err && !(errOf && err.step));
  $('macErr').textContent = err && !(errOf && err.step) ? err.message_fr || String(err) : '';
  if (!m) return;
  $('macName').value = m.name;
  $('macKey').replaceChildren(...[['', 'Aucune'], ...MACRO_LETTERS.map(l => [l, l])].map(([v, t]) => { const o = document.createElement('option'); o.value = v; o.textContent = t; return o; }));
  $('macKey').value = m.key && MACRO_LETTERS.includes(m.key) ? m.key : '';
  $('macOn').checked = m.enabled !== false;
  $('macSteps').replaceChildren(...m.steps.map((st, i) => macStepRow(m, st, i, errOf && err.step === i + 1 ? err : null)));
  macFillCommands();
}
function macStepRow(m, st, i, err) {
  const row = document.createElement('div');
  row.className = 'mac-step' + (st.enabled === false ? ' off' : '');
  row.dataset.step = i + 1;
  const kind = waitKind(st.wait);
  row.innerHTML = `<span class="mac-n">${i + 1}</span>
    <div class="row"><label for="macCmd${i}">Commande</label><input type="text" id="macCmd${i}" list="macCmdList" maxlength="200" autocomplete="off" spellcheck="false" placeholder="master.size 80"></div>
    <div class="row"><label for="macWait${i}">Attente</label><select id="macWait${i}">${MAC_WAITS.map(([v, t]) => `<option value="${v}">${t}</option>`).join('')}</select>
      <label for="macVal${i}">Valeur</label><input type="number" id="macVal${i}" min="0.05" max="600" step="0.25">
      <label class="check"><input type="checkbox" id="macStepOn${i}"><span>Active</span></label></div>
    <div class="row"><button data-act="up" aria-label="Monter l'étape ${i + 1}" title="Monter">↑</button><button data-act="down" aria-label="Descendre l'étape ${i + 1}" title="Descendre">↓</button><button data-act="del" aria-label="Supprimer l'étape ${i + 1}" title="Supprimer l'étape">✕</button></div>`;
  const cmd = row.querySelector('input[type=text]'), wait = row.querySelector('select'), val = row.querySelector('input[type=number]'), on = row.querySelector('input[type=checkbox]');
  cmd.value = st.command || '';
  wait.value = kind;
  val.value = waitVal(st.wait);
  val.hidden = val.previousElementSibling.hidden = kind === 'none' || kind === 'go';
  on.checked = st.enabled !== false;
  const setWait = () => {
    const k = wait.value, v = parseFloat(String(val.value).replace(',', '.'));
    if (k === 'seconds' || k === 'beats') {
      const n = Number.isFinite(v) && v > 0 ? v : 1;
      st.wait = { [k]: n };
      if (!(Number.isFinite(v) && v > 0)) val.value = n;
    } else st.wait = k;
    val.hidden = val.previousElementSibling.hidden = k === 'none' || k === 'go';
  };
  cmd.addEventListener('input', () => { st.command = cmd.value; macTouched(); });
  wait.addEventListener('change', () => { setWait(); macTouched(); });
  val.addEventListener('change', () => { setWait(); macTouched(); });
  on.addEventListener('change', () => { st.enabled = on.checked; row.classList.toggle('off', !on.checked); macTouched(); });
  row.querySelectorAll('[data-act]').forEach(b => b.onclick = () => {
    const to = b.dataset.act === 'up' ? i - 1 : i + 1;
    if (b.dataset.act === 'del') m.steps.splice(i, 1);
    else if (to >= 0 && to < m.steps.length) [m.steps[i], m.steps[to]] = [m.steps[to], m.steps[i]];
    else return;
    MAC.err = null; macTouched(); renderMacEdit();
  });
  if (err) {
    const e = document.createElement('div');
    e.className = 'mac-err'; e.setAttribute('role', 'alert');
    e.textContent = `Étape ${err.step} : ` + String(err.message_fr || '').replace(/^Macro « .* », étape \d+ : /, '');
    row.appendChild(e);
  }
  return row;
}
$('macSel').addEventListener('change', () => { if ($('macSel').value !== '') { MAC.sel = +$('macSel').value; renderMacEdit(); } });
// The list's entry follows the name; the steps are left alone (a full
// render here would replace the field the focus is moving to).
$('macName').addEventListener('input', () => {
  const m = macCur(); if (!m) return;
  m.name = $('macName').value; macTouched();
  const o = $('macSel').selectedOptions[0]; if (o) o.textContent = (m.name || '(sans nom)') + (m.enabled ? '' : ' (désactivée)');
});
$('macKey').addEventListener('change', () => { const m = macCur(); if (m) { m.key = $('macKey').value || null; macTouched(); } });
$('macOn').addEventListener('change', () => { const m = macCur(); if (m) { m.enabled = $('macOn').checked; macTouched(); } });
// A new macro's id, chosen here so an error names it (`m` + number).
function macNewId() {
  const taken = new Set([...MAC.draft, ...MAC.list].map(m => m.id));
  for (let n = 1; ; n++) if (!taken.has('m' + n)) return 'm' + n;
}
function macFreeName(base) {
  const taken = new Set(MAC.draft.map(m => m.name.toLowerCase()));
  for (let n = 1; ; n++) { const name = (n === 1 ? base : `${base} ${n}`).slice(0, 32); if (!taken.has(name.toLowerCase())) return name; }
}
$('macAdd').onclick = () => {
  MAC.draft.push({ id: macNewId(), name: macFreeName('Macro'), steps: [{ command: '', wait: 'none', enabled: true }], key: null, enabled: true });
  MAC.sel = MAC.draft.length - 1; MAC.err = null; macTouched(); renderMacEdit();
  $('macName').focus();
};
$('macDup').onclick = () => {
  const m = macCur(); if (!m) return;
  MAC.draft.push({ ...macClone(m), id: macNewId(), key: null, name: macFreeName(m.name.slice(0, 24) + ' (copie)') });
  MAC.sel = MAC.draft.length - 1; MAC.err = null; macTouched(); renderMacEdit();
};
$('macDel').onclick = () => {
  if (!macCur()) return;
  MAC.draft.splice(MAC.sel, 1);
  MAC.sel = Math.max(0, Math.min(MAC.sel, MAC.draft.length - 1)); MAC.err = null; macTouched(); renderMacEdit();
};
$('macAddStep').onclick = () => {
  const m = macCur(); if (!m) return;
  if (m.steps.length >= 64) { $('macState').textContent = '64 étapes au plus'; return; }
  m.steps.push({ command: '', wait: 'none', enabled: true });
  macTouched(); renderMacEdit();
  $('macCmd' + (m.steps.length - 1))?.focus();
};
$('macSave').onclick = async () => {
  const r = await api('POST', '/api/macros', { macros: MAC.draft }, { quiet: true });
  if (r.ok) {
    const j = await r.json();
    MAC.list = j.macros; MAC.rev = j.rev; MAC.draft = macClone(j.macros); MAC.dirty = false; MAC.err = null;
    $('macState').textContent = 'Macros enregistrées';
  } else {
    const text = await r.text();
    let e; try { e = JSON.parse(text); } catch { e = { message_fr: r.status === 423 ? LOCKED_FR : text }; }
    MAC.err = e;
    const at = e.macro_id ? MAC.draft.findIndex(m => m.id === e.macro_id) : -1;
    if (at >= 0) MAC.sel = at;
    $('macState').textContent = 'Non enregistré';
  }
  renderMacButtons();
  renderMacEdit();
};
// ---------- scripts (T-458) ----------
// CRÉATION › Scripts: the list edited as a draft, « Enregistrer » sends it
// whole (POST /api/scripts; a refused line is never saved). « Lancer »
// saves first, then asks for one run; the studio runs it in its own thread
// and applies its commands at a later frame. The frame carries the list's
// revision, what runs and the trust banner (`f.scripts`).
const SCR = { list: [], draft: [], status: [], sel: 0, dirty: false, rev: -1, trusted: true, halted: false, examples: [], err: null, note: '', polled: 0 };
const SCR_TRIG = { manual: 'À la demande', every_beat: 'À chaque temps', every_bar: 'À chaque mesure' };
const scrCur = () => SCR.draft[SCR.sel] || null;
const scrStatus = id => SCR.status.find(s => s.id === id) || null;
async function loadScripts(force) {
  SCR.polled = Date.now();
  const j = await apiJson('GET', '/api/scripts', undefined, { cache: 'no-store', quiet: true }).catch(() => null);
  if (!j) return;
  SCR.list = j.scripts; SCR.rev = j.rev; SCR.status = j.status; SCR.trusted = j.trusted; SCR.halted = j.halted;
  if (!SCR.examples.length && j.examples) {
    SCR.examples = j.examples;
    $('scrExample').replaceChildren(...[['', 'Exemple…'], ...j.examples.map((x, i) => [String(i), x.name])].map(([v, n]) => { const o = document.createElement('option'); o.value = v; o.textContent = n; return o; }));
  }
  if (!SCR.dirty || force === true && !SCR.draft.length) { SCR.draft = JSON.parse(JSON.stringify(j.scripts)); SCR.dirty = false; }
  SCR.sel = Math.min(SCR.sel, Math.max(0, SCR.draft.length - 1));
  if (j.warnings && j.warnings.length) SCR.note = j.warnings.join(' · ');
  renderScripts();
}
function showScripts(f) {
  if (!f) return;
  $('scrTrustBanner').hidden = !f.ask_trust;
  if (f.ask_trust) $('scrTrustText').textContent = `Ce projet contient ${f.count} script${f.count > 1 ? 's' : ''}. Ils sont désactivés.`;
  const shown = activeWs === 'creation' && !$('scrEdit').hidden;
  if (f.rev !== SCR.rev || (shown && Date.now() - SCR.polled > 700) || f.halted !== SCR.halted) loadScripts();
}
function renderScripts() {
  const rows = SCR.draft.map((sc, i) => {
    const b = document.createElement('button');
    const st = sc.id ? scrStatus(sc.id) : null;
    const bad = st && (st.kind === 'error' || st.kind === 'budget');
    b.className = 'scr-item' + (i === SCR.sel ? ' sel' : '') + (bad ? ' bad' : '');
    b.dataset.scriptId = sc.id;
    const n = document.createElement('span'); n.className = 'n'; n.textContent = sc.name || 'Sans nom';
    const d = document.createElement('span'); d.className = 'd';
    d.textContent = [SCR_TRIG[sc.trigger] || '', sc.trigger !== 'manual' && !sc.enabled ? 'inactif' : '', st ? st.state : 'non enregistré'].filter(Boolean).join(' · ');
    b.append(n, d);
    b.onclick = () => { SCR.sel = i; SCR.err = null; renderScripts(); };
    return b;
  });
  if (!rows.length) { const e = document.createElement('div'); e.className = 'mac-empty'; e.textContent = 'Aucun script : « + Script » ou un exemple'; rows.push(e); }
  $('scrList').replaceChildren(...rows);
  const sc = scrCur();
  $('scrFields').hidden = !sc;
  if (sc) {
    const put = (id, v, prop = 'value') => { const el = $(id); if (document.activeElement !== el && el[prop] !== v) el[prop] = v; };
    put('scrName', sc.name); put('scrTrigger', sc.trigger); put('scrOn', !!sc.enabled, 'checked'); put('scrSource', sc.source);
  }
  const st = sc && sc.id ? scrStatus(sc.id) : null;
  const lines = st ? st.console.slice() : [];
  $('scrConsole').textContent = lines.join('\n') || (st ? `${st.state}${st.runs ? ` · ${st.runs} exécution${st.runs > 1 ? 's' : ''}, ${st.last_ms.toFixed(1).replace('.', ',')} ms` : ''}` : '');
  $('scrConsole').scrollTop = $('scrConsole').scrollHeight;
  const err = SCR.err || (st && st.last_error ? st.last_error : '');
  $('scrErr').hidden = !err;
  $('scrErr').textContent = err;
  $('scrTrust').hidden = SCR.trusted || !SCR.list.length;
  $('scrResume').hidden = !SCR.halted;
  $('scrState').textContent = [SCR.dirty ? 'Modifications non enregistrées' : SCR.note, !SCR.trusted && SCR.list.length ? 'Scripts de ce projet désactivés' : '', SCR.halted ? 'Déclencheurs suspendus' : ''].filter(Boolean).join(' · ');
}
function scrTouched() { SCR.dirty = true; SCR.note = ''; markDirtyScripts(); }
function markDirtyScripts() { $('scrState').textContent = 'Modifications non enregistrées'; }
function scrFreeName(base) {
  const taken = new Set(SCR.draft.map(s => s.name.toLowerCase()));
  for (let n = 1; ; n++) { const name = (n === 1 ? base : `${base} ${n}`).slice(0, 32); if (!taken.has(name.toLowerCase())) return name; }
}
function scrAdd(sc) {
  if (SCR.draft.length >= 32) { SCR.err = '32 scripts au plus'; renderScripts(); return; }
  SCR.draft.push(sc); SCR.sel = SCR.draft.length - 1; SCR.err = null; scrTouched(); renderScripts();
}
$('scrAdd').onclick = () => { scrAdd({ id: 0, name: scrFreeName('Script'), source: '', trigger: 'manual', enabled: false }); $('scrName').focus(); };
$('scrExample').onchange = () => {
  const x = SCR.examples[+$('scrExample').value];
  $('scrExample').value = '';
  if (x) scrAdd({ id: 0, name: scrFreeName(x.name), source: x.source, trigger: x.trigger, enabled: false });
};
$('scrDel').onclick = () => {
  if (!scrCur()) return;
  SCR.draft.splice(SCR.sel, 1);
  SCR.sel = Math.max(0, Math.min(SCR.sel, SCR.draft.length - 1)); SCR.err = null; scrTouched(); renderScripts();
};
$('scrName').oninput = () => { const sc = scrCur(); if (sc) { sc.name = $('scrName').value; scrTouched(); } };
$('scrName').onchange = renderScripts;
$('scrTrigger').onchange = () => { const sc = scrCur(); if (sc) { sc.trigger = $('scrTrigger').value; scrTouched(); renderScripts(); } };
$('scrOn').onchange = () => { const sc = scrCur(); if (sc) { sc.enabled = $('scrOn').checked; scrTouched(); renderScripts(); } };
$('scrSource').oninput = () => { const sc = scrCur(); if (sc) { sc.source = $('scrSource').value; scrTouched(); } };
async function scrSave() {
  const r = await api('POST', '/api/scripts', { scripts: SCR.draft }, { quiet: true });
  if (r.ok) {
    const j = await r.json();
    SCR.dirty = false; SCR.err = null; SCR.draft = JSON.parse(JSON.stringify(j.scripts)); SCR.note = 'Scripts enregistrés';
    SCR.list = j.scripts; SCR.rev = j.rev; SCR.status = j.status; SCR.trusted = j.trusted; SCR.halted = j.halted;
    renderScripts();
    return true;
  }
  const text = await r.text();
  let e; try { e = JSON.parse(text); } catch { e = { message_fr: r.status === 423 ? LOCKED_FR : text }; }
  const at = e.id ? SCR.draft.findIndex(s => s.id === e.id) : -1;
  if (at >= 0) SCR.sel = at;
  SCR.err = e.message_fr || 'Non enregistré';
  renderScripts();
  $('scrState').textContent = 'Non enregistré';
  return false;
}
$('scrSave').onclick = scrSave;
async function scrRun() {
  if (SCR.dirty && !await scrSave()) return;
  const sc = scrCur();
  if (!sc || !sc.id) return;
  SCR.err = null;
  const r = await api('POST', '/api/scripts/run', { id: sc.id, source: 'ui' }, { quiet: true });
  if (!r.ok) { SCR.err = await r.text(); renderScripts(); return; }
  for (const ms of [120, 400, 1000]) setTimeout(loadScripts, ms);
}
$('scrRun').onclick = scrRun;
$('scrStop').onclick = async () => {
  const sc = scrCur();
  if (!sc || !sc.id) return;
  await api('POST', '/api/scripts/stop', { id: sc.id }, { quiet: true });
  loadScripts();
};
$('scrCheck').onclick = async () => {
  const sc = scrCur();
  if (!sc) return;
  const j = await apiJson('POST', '/api/scripts/check', { source: sc.source }, { quiet: true }).catch(() => null);
  if (!j) return;
  SCR.err = j.ok ? null : j.error.message_fr;
  renderScripts();
  if (j.ok) $('scrState').textContent = 'Aucune erreur';
};
async function scrApprove(yes) {
  await api('POST', '/api/scripts/trust', { approve: yes }, { quiet: true });
  $('scrTrustBanner').hidden = true;
  loadScripts();
}
$('scrTrustYes').onclick = () => scrApprove(true);
$('scrTrustNo').onclick = () => scrApprove(false);
$('scrTrust').onclick = () => scrApprove(true);
$('scrResume').onclick = async () => {
  const r = await api('POST', '/api/scripts/run', {}, { quiet: true });
  if (!r.ok) SCR.err = await r.text();
  loadScripts();
};
$('cueMenuClose').onclick = () => { $('cueMenu').hidden = true; assignMenuCue(null); };
$('cueMenuRemove').onclick = async () => {
  $('cueMenu').hidden = true; assignMenuCue(null);
  await gridPost('/api/grid/clear', { page: gridPage, slot: menuSlot });
};
document.addEventListener('pointerdown', e => { if (!$('cueMenu').hidden && !$('cueMenu').contains(e.target)) $('cueMenu').hidden = true; });
// T-405: the cell's own cue, by its place on the page.
const cellEdit = async body => { if (await gridPost('/api/grid/edit', { from: [gridPage, menuSlot], ...body })) await loadPresets(); };
$('cueMenuNameInput').addEventListener('change', () => { const name = $('cueMenuNameInput').value.trim(); if (name) cellEdit({ op: 'rename', name }); });
$('cueMenuNameInput').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); $('cueMenuNameInput').blur(); } });
$('cueMenuColor').addEventListener('change', () => cellEdit({ op: 'color', color: rgb($('cueMenuColor').value) }));
$('cueMenuCopy').onclick = () => { $('cueMenu').hidden = true; cellEdit({ op: 'copy', to: [gridPage, null] }); };
$('cueMenuToUser').onclick = () => { $('cueMenu').hidden = true; cellEdit({ op: 'copy_to_user' }); };
$('cueMenuDelete').onclick = () => {
  const p = presetById.get(menuCue);
  $('cueMenu').hidden = true;
  if (confirm(`Supprimer le cue « ${p ? p.name : ''} » ?`)) cellEdit({ op: 'delete' });
};

export { MAC, loadMacros, loadScripts, macLoadCommands, macRun, macWaitText, macroForKey, scrRun,
  showMacros, showScripts };
