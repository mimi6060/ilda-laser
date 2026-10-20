// Laser Studio — simple.js (T-530): the Mode Simple view's own wiring.
//
// Mode Simple is CSS-first (styles.css hides the Pro density and lays out
// the big preview beside this clean panel); this module only wires the
// panel's few controls. It is the last module of the page order, so it
// imports the one before it (command.js) and app.js imports this one.
//
// Nothing here loosens anything: the looks write the same `content` as
// CRÉATION (through editLook → /api/settings, never a raw fetch, T-432),
// the brightness slider is the ordinary master.brightness one wired by
// direct.js, and the LASER / Noir / ARRÊT buttons stay the top bar's.
import './command.js';
import { $, editLook, settings, showSettings } from './core.js';
import { setUiMode, setWorkspace } from './workspaces.js';

// The default generator parameters, the same ones the CRÉATION « Effet »
// button uses, so showSettings reads a complete look.
const GEN_PARAMS = { count: 8, a: 3, b: 2, speed: 1, color_mode: 'rainbow', color2: [0, 0, 255] };

// One look per big button: a shape from patterns.rs, or a simple generator.
function pickShape(shape) {
  editLook(s => { s.content = { kind: 'shape', shape }; });
  showSettings();
}
function pickGenerator(generator) {
  editLook(s => { s.content = { kind: 'generator', generator, params: structuredClone(GEN_PARAMS) }; });
  showSettings();
}
$('simpleLooks').querySelectorAll('[data-simple-shape]').forEach(b =>
  b.addEventListener('click', () => pickShape(b.dataset.simpleShape)));
$('simpleLooks').querySelectorAll('[data-simple-gen]').forEach(b =>
  b.addEventListener('click', () => pickGenerator(b.dataset.simpleGen)));

// The active look, kept in step with the real settings (CRÉATION, cues,
// MIDI… all end in showSettings, which fires `ls:look`).
function syncLooks() {
  const c = settings && settings.content;
  if (!c) return;
  $('simpleLooks').querySelectorAll('[data-simple-shape]').forEach(b =>
    b.classList.toggle('active', c.kind === 'shape' && c.shape === b.dataset.simpleShape));
  $('simpleLooks').querySelectorAll('[data-simple-gen]').forEach(b =>
    b.classList.toggle('active', c.kind === 'generator' && c.generator === b.dataset.simpleGen));
}
document.addEventListener('ls:look', syncLooks);
syncLooks();

// « Réglages avancés » : switch to Pro and open RÉGLAGES (the only place
// that leaves the clean view on purpose).
$('simpleAdvanced').addEventListener('click', () => { setUiMode('pro'); setWorkspace('settings'); });
