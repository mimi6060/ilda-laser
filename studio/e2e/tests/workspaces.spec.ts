// Workspaces (T-295) and screen regions (T-270): LIVE, TIMELINE, CRÉATION,
// RÉGLAGES. Switching is display only; the top bar with the laser button,
// ARRÊT, tempo and the banners is on screen in every workspace; Space and
// Escape work everywhere, cue letters only in LIVE.
import { test, expect, useStudio, openUi, openWorkspace, reveal, focusPage, type Workspace } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
const WORKSPACES: Workspace[] = ['live', 'timeline', 'creation', 'settings'];
const activeCue = async () => (await studio.controlValues()).active_cue;

/** Where each existing section lives (T-295 « Répartition »). */
const SECTIONS: Record<Workspace, string[]> = {
  live: ['#cues', '#cueTabs', '#cueModes', '#cueMulti', '#layers', '#liveReset', '#rotPresets', '#mSize',
    '#colorModes', '#lfoAdd', '#lfoList', '#micBtn', '#aOn', '#sceneList', '#plStart'],
  timeline: ['#timeline', '#tlShow', '#tlPlay', '#tlBar'],
  creation: ['[data-kind="shape"]', '[data-kind="generator"]', '#shapes', '#text', '#gen', '#genPanel', '#evoPanel', '#color', '#scale', '#bright'],
  settings: ['#safetyPanel', '#safetyPanel summary', 'details:has(#cOx) > summary', '#midiPanel summary',
    '#sfHz', '#holdToRun', '#cOx', '#midiPanel', '#midiLearnMode', '#midiAllowArm'],
};
/**
 * Shown only for their kind of content, empty until filled, or inside a
 * collapsed section (Sécurité, Calibration, Contrôleur): placement checked, not visibility.
 */
const NOT_ALWAYS_SHOWN = new Set(['#lfoList', '#shapes', '#text', '#gen', '#genPanel', '#evoPanel',
  '#sfHz', '#holdToRun', '#cOx', '#midiLearnMode', '#midiAllowArm']);
/** Always on screen, whatever the workspace. */
const TOP_BAR = ['#projBtn', '#bpm', '#beats', '#midiStatus', '#armBtn', '#estopBtn', '#armInfo', '#wsTabs'];

/** The part of /api/state a display change must not touch (tempo phase and presence tick on their own). */
async function steadyState() {
  const st = await studio.state();
  const { tempo, presence, ...rest } = st;
  return { ...rest, bpm: tempo.bpm };
}
async function steadyFrame() {
  const f = await studio.frame();
  return { armed: f.armed, cue_page: f.cue_page, active_cue: f.active_cue, playlist: f.playlist, live: f.live, bpm: f.tempo.bpm };
}

async function shoot(page: Page, name: string) {
  await page.screenshot({ path: `test-results/workspaces-${name}.png` });
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await openUi(page, studio);
});

test('four workspace tabs, LIVE first, each section in its workspace', async ({ page }) => {
  const tabs = page.locator('#wsTabs [data-ws-tab]');
  await expect(tabs).toHaveCount(4);
  for (const [i, label] of ['LIVE', 'TIMELINE', 'CRÉATION', 'RÉGLAGES'].entries()) await expect(tabs.nth(i)).toContainText(label);
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveAttribute('aria-selected', 'true');

  for (const ws of WORKSPACES) {
    await openWorkspace(page, ws);
    for (const other of WORKSPACES) {
      for (const sel of SECTIONS[other]) {
        // In its own workspace and nowhere else.
        expect(await page.locator(sel).evaluate(el => (el.closest('[data-ws]') as HTMLElement).dataset.ws), sel).toBe(other);
        if (other !== ws) await expect(page.locator(sel), `${sel} hidden in ${ws}`).toBeHidden();
      }
    }
    for (const sel of SECTIONS[ws].filter(s => !NOT_ALWAYS_SHOWN.has(s))) {
      await reveal(page, sel); // at most one more click (a LIVE panel tab)
      await expect(page.locator(sel), `${sel} shown in ${ws}`).toBeVisible();
    }
    // The preview is on screen in LIVE, TIMELINE and CRÉATION (and RÉGLAGES, for calibration).
    await expect(page.locator('#preview')).toBeVisible();
    await expect(page.locator('#stPoints')).toBeVisible();
  }
});

test('top bar, laser button, ARRÊT and the banners are visible in every workspace', async ({ page }) => {
  // Force the MIDI-lost banner on (no controller in tests) to check where it sits.
  await page.locator('#midiLost').evaluate(el => el.classList.add('on'));
  for (const ws of WORKSPACES) {
    await openWorkspace(page, ws);
    for (const sel of TOP_BAR) await expect(page.locator(sel), `${sel} in ${ws}`).toBeVisible();
    await expect(page.locator('#midiLost'), `MIDI lost banner in ${ws}`).toBeVisible();
    for (const sel of ['#estopBtn', '#armBtn', '#midiLost']) {
      expect(await page.locator(sel).evaluate(el => !el.closest('[data-ws]') && !!el.closest('#top')), sel).toBe(true);
    }
  }
  await page.locator('#midiLost').evaluate(el => el.classList.remove('on'));

  // ARRÊT (the button) latches the stop from any workspace, and its banner shows there.
  for (const ws of WORKSPACES) {
    await openWorkspace(page, ws);
    await page.locator('#armBtn').click();
    await expect.poll(async () => (await studio.state()).armed).toBe(true);
    await page.locator('#estopBtn').click();
    await expect.poll(async () => (await studio.state()).estop).toBe(true);
    expect((await studio.state()).armed).toBe(false);
    await expect(page.locator('#estopBanner'), `stop banner in ${ws}`).toBeVisible();
    await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
    await page.locator('#estopReset').click();
    await expect(page.locator('#estopBanner')).toBeHidden();
    await expect.poll(async () => (await studio.state()).estop).toBe(false);
  }
});

test('Space arms and disarms, Escape stops, Shift+Escape disarms, in every workspace', async ({ page }) => {
  for (const ws of WORKSPACES) {
    await openWorkspace(page, ws);
    await focusPage(page);
    await page.keyboard.press('Space');
    await expect.poll(async () => (await studio.state()).armed, `Space arms in ${ws}`).toBe(true);
    await page.keyboard.press('Space');
    await expect.poll(async () => (await studio.state()).armed, `Space disarms in ${ws}`).toBe(false);

    await page.keyboard.press('Space');
    await expect.poll(async () => (await studio.state()).armed).toBe(true);
    await page.keyboard.press('Shift+Escape');
    await expect.poll(async () => (await studio.state()).armed, `Shift+Escape in ${ws}`).toBe(false);
    expect((await studio.state()).estop).toBe(false);

    await page.keyboard.press('Space');
    await expect.poll(async () => (await studio.state()).armed).toBe(true);
    await page.keyboard.press('Escape');
    await expect.poll(async () => (await studio.state()).estop, `Escape in ${ws}`).toBe(true);
    expect((await studio.state()).armed).toBe(false);
    await expect(page.locator('#estopBanner')).toBeVisible();
    await page.locator('#estopReset').click();
    await expect.poll(async () => (await studio.state()).estop).toBe(false);
  }
});

test('Escape stops even while typing in a CRÉATION text field', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(async () => (await studio.state()).armed).toBe(true);
  await reveal(page, '[data-kind="text"]');
  await page.locator('[data-kind="text"]').click();
  await page.locator('#text').click();
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await studio.state()).armed).toBe(false);
  expect((await studio.state()).estop).toBe(true);
  await page.locator('#estopReset').click();
});

test('cue letters play cues in LIVE only', async ({ page }) => {
  const catalog = await studio.presets();
  const cues = catalog.presets.filter(p => p.category === catalog.categories[0]);
  for (const ws of ['timeline', 'creation', 'settings'] as Workspace[]) {
    await openWorkspace(page, ws);
    await focusPage(page);
    await page.keyboard.press('z');
    await page.keyboard.press('Shift+E'); // flash
    await page.keyboard.press('<'); // master reverse: a LIVE key too
  }
  await new Promise(r => setTimeout(r, 300)); // proving nothing happened
  expect(await activeCue()).toBeNull();
  expect((await studio.live()).rot_reverse).toBe(false);

  await openWorkspace(page, 'live');
  await focusPage(page);
  await page.keyboard.press('z');
  await expect.poll(activeCue).toBe(cues[1].id);
  await expect(page.locator(`#cues .cue[data-id="${cues[1].id}"]`)).toHaveClass(/active/);
});

test('F1 to F4 switch workspaces without touching the laser or the cues', async ({ page }) => {
  await page.locator('#cues .cue').first().click();
  await expect.poll(activeCue).not.toBeNull();
  await page.locator('#bpm').fill('128');
  await page.locator('#bpm').press('Tab');
  await expect.poll(async () => (await studio.state()).tempo.bpm).toBeCloseTo(128, 3);
  await focusPage(page);
  const keys: [string, Workspace][] = [['F2', 'timeline'], ['F3', 'creation'], ['F4', 'settings'], ['F1', 'live']];
  for (const [key, ws] of keys) {
    const before = await steadyState();
    const frameBefore = await steadyFrame();
    await page.keyboard.press(key);
    await expect(page.locator(`#wsTabs [data-ws-tab="${ws}"]`)).toHaveClass(/active/);
    await expect(page.locator(SECTIONS[ws][0])).toBeVisible();
    expect(await steadyState(), `/api/state after ${key}`).toEqual(before);
    expect(await steadyFrame(), `/api/frame after ${key}`).toEqual(frameBefore);
  }
  // F-keys also work from a text field (they type nothing).
  await reveal(page, '#sceneName');
  await page.locator('#sceneName').click();
  await page.keyboard.press('F3');
  await expect(page.locator('#wsTabs [data-ws-tab="creation"]')).toHaveClass(/active/);
});

test('clicking through the tabs leaves /api/state and /api/frame unchanged', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(async () => (await studio.state()).armed).toBe(true);
  await page.locator('#cues .cue').nth(2).click();
  await expect.poll(activeCue).not.toBeNull();
  const before = await steadyState();
  const frameBefore = await steadyFrame();
  for (const ws of [...WORKSPACES, ...WORKSPACES.slice().reverse()]) {
    await openWorkspace(page, ws);
    if (ws === 'live') for (const t of ['lfo', 'music', 'scenes', 'direct']) await page.locator(`#liveTabs [data-panel-tab="${t}"]`).click();
  }
  for (const v of ['3d', 'both', '2d']) await page.locator(`#viewSeg [data-view="${v}"]`).click();
  expect(await steadyState()).toEqual(before);
  expect(await steadyFrame()).toEqual(frameBefore);
  await page.locator('#armBtn').click();
  await expect.poll(async () => (await studio.state()).armed).toBe(false);
});

test('the workspace, LIVE panel tab and stage tab come back after a reload', async ({ page }) => {
  await openWorkspace(page, 'creation');
  await page.locator('#viewSeg [data-view="both"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#preview')).toBeVisible();
  await page.reload();
  await expect(page.locator('#wsTabs [data-ws-tab="creation"]')).toHaveClass(/active/);
  await expect(page.locator('#gen')).toBeAttached();
  await expect(page.locator('#shapes')).toBeVisible();
  await expect(page.locator('#cues')).toBeHidden();
  await expect(page.locator('#viewSeg [data-view="both"]')).toHaveClass(/active/);
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#preview')).toBeVisible();

  await openWorkspace(page, 'live');
  await page.locator('#liveTabs [data-panel-tab="scenes"]').click();
  await page.locator('#viewSeg [data-view="2d"]').click();
  await page.reload();
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);
  await expect(page.locator('#liveTabs [data-panel-tab="scenes"]')).toHaveClass(/active/);
  await expect(page.locator('#sceneList')).toBeVisible();
  await expect(page.locator('#mSize')).toBeHidden();
  await expect(page.locator('#beams')).toBeHidden();
});

test('without localStorage the page starts on LIVE and the tabs still work', async ({ page }) => {
  await page.addInitScript(() => {
    const boom = () => { throw new Error('storage disabled'); };
    Storage.prototype.getItem = boom; Storage.prototype.setItem = boom;
  });
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(String(e)));
  await page.reload();
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await openWorkspace(page, 'settings');
  await expect(page.locator('#midiPanel')).toBeVisible();
  await focusPage(page);
  await page.keyboard.press('F2');
  await expect(page.locator('#timeline')).toBeVisible();
  expect(errors).toEqual([]);
});

test('right click opens the MIDI menu on controls of every workspace', async ({ page }) => {
  const targets: [Workspace, string, string][] = [
    ['live', '#mSize', 'master.size'], ['creation', '#bpm', 'tempo.bpm'], ['settings', '#estopBtn', 'safety.estop'],
  ];
  for (const [ws, sel, id] of targets) {
    await openWorkspace(page, ws);
    await page.locator(sel).click({ button: 'right' });
    await expect(page.locator('#midiMenu')).toBeVisible();
    await expect(page.locator('#midiMenuName')).toContainText(id);
    await page.locator('h1').click();
    await expect(page.locator('#midiMenu')).toBeHidden();
  }
});

test('the MIDI status in the top bar opens the controller settings', async ({ page }) => {
  await page.locator('#midiStatus').click();
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toHaveClass(/active/);
  await expect(page.locator('#midiLearnMode')).toBeVisible();
});

test('T-270 layout: at 1440×900 the preview and two rows of cues fit on screen', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const inView = async (sel: string) => {
    const b = (await page.locator(sel).boundingBox())!;
    return b.x >= 0 && b.y >= 0 && b.x + b.width <= 1440 && b.y + b.height <= 900;
  };
  await expect.poll(() => inView('#preview')).toBe(true);
  // The first two rows of the grid: cells 0 and the first cell of row 2.
  const cells = page.locator('#cues .cue');
  const tops = await cells.evaluateAll(els => els.map(e => e.getBoundingClientRect().top));
  const rows = [...new Set(tops.map(Math.round))].sort((a, b) => a - b);
  expect(rows.length).toBeGreaterThanOrEqual(2);
  const secondRow = tops.findIndex(t => Math.round(t) === rows[1]);
  expect(await inView('#cues .cue >> nth=0')).toBe(true);
  expect(await inView(`#cues .cue >> nth=${secondRow}`)).toBe(true);
  expect(await page.evaluate(() => document.scrollingElement!.scrollTop)).toBe(0);
  await shoot(page, 'live');
  for (const ws of ['timeline', 'creation', 'settings'] as Workspace[]) {
    await openWorkspace(page, ws);
    await expect.poll(() => inView('#preview')).toBe(true);
    await shoot(page, ws);
  }
  await openWorkspace(page, 'live');
  await page.locator('#viewSeg [data-view="both"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect.poll(() => inView('#beams')).toBe(true);
  await shoot(page, 'live-2d3d');
  await page.locator('#viewSeg [data-view="2d"]').click();
});

test('T-270 layout: no horizontal scroll at 1280×800, one column below 900 px', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  for (const ws of WORKSPACES) {
    await openWorkspace(page, ws);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), `1280 ${ws}`).toBe(true);
  }
  await page.setViewportSize({ width: 800, height: 900 });
  await openWorkspace(page, 'live');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  // Stage, then grid, then panel.
  const top = async (sel: string) => (await page.locator(sel).boundingBox())!.y;
  expect(await top('#preview')).toBeLessThan(await top('#cues'));
  expect(await top('#cues')).toBeLessThan(await top('#liveTabs'));
  await shoot(page, 'narrow');
});
