// Laser Studio — app.js (T-435): the entry point of the page's modules.
//
// The modules run in page order: each one imports the one before it
// first, and this file imports the last one, so the ES module graph is
// evaluated exactly as the former single script was read, top to
// bottom. Chain:
//   core.js → laser.js → presence.js → state.js → view3d.js → workspaces.js →
//   music.js → cuelist.js → zones.js → safety.js → targets.js → direct.js →
//   tempo.js → cues.js → presets.js → macros.js → shows.js → timeline-editor.js →
//   timeline-markers.js → timeline-canvas.js → layers.js → desk.js → modulators.js → outputs.js →
//   venue.js → midi.js → apc40.js → project.js → figures.js → media.js →
//   program.js → help.js → command.js
import './command.js';
import { api, armed, controlLatest, sendLatest } from './core.js';
import { arm } from './laser.js';
import { LINK } from './state.js';
import { lastPoints, lastShown, view3d } from './view3d.js';
import { reloadSafety } from './safety.js';
import { sliderLabel } from './direct.js';
import { askBeforeQuit } from './project.js';
import { pg } from './program.js';

// What the e2e tests reach by name, as they did when the page was one
// classic script (its top-level names were globals). Functions are set;
// variables are read through getters, so the current value is seen.
Object.assign(window, { api, sendLatest, controlLatest, askBeforeQuit, reloadSafety, sliderLabel });
for (const [name, get] of Object.entries({
  view3d: () => view3d,
  lastPoints: () => lastPoints,
  lastShown: () => lastShown,
  pg: () => pg,
  arm: () => arm,
  armed: () => armed,
  LINK: () => LINK,
})) Object.defineProperty(window, name, { get, configurable: true });

// Tells the inline guard of index.html that the keyboard handler of
// laser.js is installed: Échap is handled there from now on.
document.documentElement.dataset.uiReady = '';
