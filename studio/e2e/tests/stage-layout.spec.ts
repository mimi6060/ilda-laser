// Layout of the workspaces' stage (T-503, T-506, T-509): at 1024×768,
// 1280×800, 1440×900 and 1920×1080, in 2D, 3D and 2D+3D, every workspace
// shows a usable view (each visible canvas at least 160 px), the 3D
// settings drawer and the safety badge stay inside the stage, the
// toolbars under the stage stay clickable (nothing from the stage covers
// them), the cue grid keeps rows on a small screen, the TIMELINE panel
// shows its transport and timecode without scrolling, the markers' labels
// never overlap and the tracks stay on screen in show mode, read only.
//
// Preview only: the fake test output, the laser never armed; everything
// here is display, measured with getBoundingClientRect / elementFromPoint.
import { test, expect, useStudio, openUi, openWorkspace, open3dSettings, openCueMore, type Workspace } from '../studio';
import type { Locator, Page } from '@playwright/test';

const studio = useStudio();
const SIZES = [{ width: 1024, height: 768 }, { width: 1280, height: 800 }, { width: 1440, height: 900 }, { width: 1920, height: 1080 }] as const;
const WORKSPACES: Workspace[] = ['live', 'timeline', 'creation', 'settings'];
const MODES = ['2d', '3d', 'both'] as const;
const MIN_VIEW = 160;

type Box = { x: number; y: number; width: number; height: number };
const bottom = (b: Box) => b.y + b.height;
const right = (b: Box) => b.x + b.width;
const inside = (a: Box, b: Box, tol = 0.5) => a.x >= b.x - tol && a.y >= b.y - tol && right(a) <= right(b) + tol && bottom(a) <= bottom(b) + tol;

async function box(loc: Locator): Promise<Box> {
  const b = await loc.boundingBox();
  expect(b, `${loc} has a box`).not.toBeNull();
  return b!;
}

/** The view mode, like the operator: the 2D / 3D / 2D+3D buttons. */
async function setMode(page: Page, mode: typeof MODES[number]) {
  await page.locator(`#viewSeg [data-view="${mode}"]`).click();
  await expect(page.locator(`#viewSeg [data-view="${mode}"]`)).toHaveClass(/active/);
  if (mode !== '2d') await expect(page.locator('#beams')).toBeVisible();
  if (mode !== '3d') await expect(page.locator('#preview')).toBeVisible();
  // One layout pass after the switch.
  await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
}

/** elementFromPoint at the centre of every visible button of `selector` is the button itself. */
async function clickable(page: Page, selector: string) {
  const hits = await page.evaluate(sel => {
    const out: string[] = [];
    for (const b of document.querySelectorAll<HTMLElement>(sel)) {
      if (b.hidden || b.closest('[hidden]') || !b.offsetParent) continue;
      const r = b.getBoundingClientRect();
      if (!r.width || !r.height) continue;
      if (r.bottom > innerHeight || r.right > innerWidth || r.top < 0) { out.push(`${b.id || b.textContent?.trim()} off screen (${Math.round(r.top)}…${Math.round(r.bottom)})`); continue; }
      const el = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
      if (el !== b && !b.contains(el)) out.push(`${b.id || b.textContent?.trim()} covered by ${el?.tagName}#${(el as HTMLElement | null)?.id}.${el?.className}`);
    }
    return out;
  }, selector);
  expect(hits, `${selector} clickable`).toEqual([]);
}

/** The canvases on screen measure at least MIN_VIEW and sit inside the stage. */
async function viewUsable(page: Page, mode: typeof MODES[number], label: string) {
  const stage = await box(page.locator('#stage'));
  const vp = page.viewportSize()!;
  for (const [id, shown] of [['preview', mode !== '3d'], ['beams', mode !== '2d']] as const) {
    const c = page.locator('#' + id);
    if (!shown) { await expect(c, `${label} #${id}`).toBeHidden(); continue; }
    const b = await box(c);
    expect(b.height, `${label} #${id} height`).toBeGreaterThanOrEqual(MIN_VIEW);
    expect(b.width, `${label} #${id} width`).toBeGreaterThanOrEqual(MIN_VIEW);
    expect(inside(b, stage), `${label} #${id} inside the stage`).toBe(true);
    expect(inside(b, { x: 0, y: 0, width: vp.width, height: vp.height }), `${label} #${id} on screen`).toBe(true);
  }
  // The stats line under the view (points, « Faisceaux dans le public ») is on screen.
  const st = await box(page.locator('#stPoints'));
  expect(inside(st, stage), `${label} stats inside the stage`).toBe(true);
  // No vertical page scroll: the workspace fits the window.
  expect(await page.evaluate(() => document.documentElement.scrollHeight <= innerHeight + 1), `${label} no page scroll`).toBe(true);
}

const TOOLBARS: Record<Workspace, string[]> = {
  live: ['#viewSeg button', '#cueModes button', '#cueMoreBtn', '#cuePageAdd'],
  timeline: ['#viewSeg button', '#tlEditor .tl-tools button', '#tlPlay', '#tlStop'],
  creation: ['#viewSeg button', '#creationTabs button'],
  settings: ['#viewSeg button'],
  desk: ['#viewSeg button'],
};

test.beforeEach(async ({ page }) => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  await page.goto(studio.url + '/');
});

test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  if ((await studio.get('/api/lock')).show_mode) await studio.post('/api/lock', { show_mode: false });
});

for (const size of SIZES) {
  test.describe(`${size.width}×${size.height}`, () => {
    for (const ws of WORKSPACES) {
      test(`${ws}: the view keeps ${MIN_VIEW} px, the toolbars stay clickable, the 3D safety badge is on screen`, async ({ page }) => {
        await page.setViewportSize(size);
        await openUi(page, studio);
        await openWorkspace(page, ws);
        for (const mode of MODES) {
          const label = `${ws} ${mode}`;
          await setMode(page, mode);
          await viewUsable(page, mode, label);
          for (const sel of TOOLBARS[ws]) await clickable(page, sel);
          if (mode === '2d') continue;
          // The safety badge of the 3D view, inside the view, in every mode.
          const view = await box(page.locator('#stage .view'));
          await expect(page.locator('#aud3d'), label).toBeVisible();
          expect(inside(await box(page.locator('#aud3d')), view), `${label} badge inside the view`).toBe(true);
          // The settings drawer never leaves the stage, and the badge and
          // the toolbars stay reachable while it is open.
          await open3dSettings(page);
          const drawer = await box(page.locator('#render3d'));
          expect(inside(drawer, view), `${label} drawer inside the view`).toBe(true);
          expect(drawer.y, `${label} drawer below the badge`).toBeGreaterThanOrEqual(bottom(await box(page.locator('#aud3d'))) - 0.5);
          await expect(page.locator('#aud3d'), label).toBeVisible();
          await expect(page.locator('#cam3dSeg'), label).toBeVisible();
          for (const sel of TOOLBARS[ws]) await clickable(page, sel);
          await page.locator('#render3dClose').click();
          await expect(page.locator('#render3d')).toBeHidden();
          await expect(page.locator('#render3dBtn')).toHaveAttribute('aria-expanded', 'false');
        }
        await setMode(page, '2d');
        await expect(page.locator('#render3dBtn')).toBeHidden();
      });
    }

    test('LIVE: rows of cues on screen, the rare settings behind « Plus… », pages beyond the edge reachable', async ({ page }) => {
      await page.setViewportSize(size);
      await openUi(page, studio);
      const scroll = await box(page.locator('#cueRegion .cue-scroll'));
      const cells = await page.locator('#cues .cue').evaluateAll((els, s) => els.map(e => e.getBoundingClientRect())
        .filter(r => r.top >= s.y - 0.5 && r.bottom <= s.y + s.height + 0.5).map(r => Math.round(r.top)), scroll);
      const rows = new Set(cells).size;
      expect(rows, 'full rows of cues on screen').toBeGreaterThanOrEqual(size.height >= 800 ? 2 : 1);
      // Folded by default, nothing removed: « Plus… » shows them, the choice is kept on reload.
      await expect(page.locator('#cueMax')).toBeHidden();
      await expect(page.locator('#cueBeatMode')).toBeHidden();
      await openCueMore(page);
      await expect(page.locator('#cueMax')).toBeVisible();
      await expect(page.locator('#gridMenuBtn')).toBeVisible();
      await expect(page.locator('#cueMoreBtn')).toHaveText('Moins');
      await page.reload();
      await expect(page.locator('#cueMax')).toBeVisible();
      await page.locator('#cueMoreBtn').click();
      await expect(page.locator('#cueMax')).toBeHidden();
      // The layers' speed selects are not cut: the widest option's text,
      // measured with the select's font, fits its content box less the arrow.
      for (const sel of await page.locator('#layers .spd').all()) {
        const m = await sel.evaluate(e => {
          const s = e as HTMLSelectElement, cs = getComputedStyle(s);
          const c = document.createElement('canvas').getContext('2d')!;
          c.font = `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
          const widest = Math.max(...Array.from(s.options, o => c.measureText(o.text).width));
          const room = s.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight) - 22; // 22 px: the native arrow
          return { widest, room, shown: s.options[s.selectedIndex].text };
        });
        expect(m.widest, `speed select « ${m.shown} » fits (${m.widest.toFixed(1)} px in ${m.room.toFixed(1)} px)`).toBeLessThanOrEqual(m.room);
      }
      // The executors: on a tall screen they start on screen (as on develop,
      // where their last 24 px scroll at 1080); on short screens the layers
      // column scrolls to them (the 1280×800 trade-off: two rows of cues).
      const region = await box(page.locator('#layerRegion'));
      if (size.height >= 1000) {
        const exec = await box(page.locator('#execRow'));
        expect(exec.y + 24, 'executors start on screen without scrolling').toBeLessThanOrEqual(bottom(region));
      }
      await page.locator('#execRow').scrollIntoViewIfNeeded();
      expect(inside(await box(page.locator('#execRow')), region), 'executors whole after scrolling the layers column').toBe(true);
      // Page tabs past the right edge: the tab row says so and scrolls to them.
      const tabs = page.locator('#cueTabs');
      const overflow = await tabs.evaluate(t => t.scrollWidth > t.clientWidth + 1);
      if (overflow) {
        await expect(tabs).toHaveClass(/more-right/);
        const last = tabs.locator('button').last();
        await last.scrollIntoViewIfNeeded();
        await expect(tabs).toHaveClass(/more-left/);
        await expect(last).toBeInViewport();
      } else {
        await expect(tabs).not.toHaveClass(/more-right/);
      }
    });
  });
}

test('TIMELINE at 1440×900: transport, TC-IN and TC-OUT on screen without scrolling, a 240 px preview', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openUi(page, studio);
  await openWorkspace(page, 'timeline');
  await setMode(page, '2d');
  expect((await box(page.locator('#preview'))).height).toBeGreaterThanOrEqual(240);
  const panel = page.locator('#timelinePanel');
  expect(await panel.evaluate(p => p.scrollTop)).toBe(0);
  const pb = await box(panel);
  for (const id of ['tlPlay', 'tlStop', 'tcArm', 'tcoToggle']) {
    const b = await box(page.locator('#' + id));
    expect(inside(b, pb), `#${id} visible in the panel without scrolling`).toBe(true);
  }
  // The editor's own buttons are not covered by the stage.
  await clickable(page, '#tlEditor .tl-tools button');
});

test('CRÉATION › Figures at 1024×768: the tools do not overlap the frames row', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="figures"]').click();
  await expect(page.locator('#figEditor')).toBeVisible();
  const frames = await box(page.locator('#figEditor .fig-frames'));
  const tools = page.locator('#figTools > *');
  const boxes: Box[] = [];
  for (const t of await tools.all()) {
    const b = await box(t);
    expect(bottom(b), 'tool above the frames row').toBeLessThanOrEqual(frames.y + 0.5);
    boxes.push(b);
  }
  for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
    const a = boxes[i], b = boxes[j];
    const overlap = a.x < right(b) - 1 && b.x < right(a) - 1 && a.y < bottom(b) - 1 && b.y < bottom(a) - 1;
    expect(overlap, `tools ${i} and ${j} overlap`).toBe(false);
  }
  await clickable(page, '#figTools button');
  expect((await box(page.locator('#figCanvas'))).height).toBeGreaterThanOrEqual(MIN_VIEW);
});

test('marker labels never overlap, the full name shows on hover', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openUi(page, studio);
  await openWorkspace(page, 'timeline');
  // The layout rule itself: labels cut before the next flag, nothing under 2 characters.
  const layout = await page.evaluate(async () => {
    const m = await import('/ui/timeline-canvas.js' as string);
    const measure = (s: string) => s.length * 7;
    const marks = [{ x: 10, name: 'Faisceaux' }, { x: 60, name: 'Grand noir' }, { x: 75, name: 'Ciseaux' }, { x: 200, name: 'Rayons tournants inversés' }];
    return { out: m.tlMarkerLayout(marks, 260, measure), marks };
  });
  const out = layout.out as { text: string; x0: number; x1: number; cut: boolean }[];
  expect(out.map(l => l.cut)).toEqual([true, true, false, true]);
  expect(out[0].text.endsWith('…') && out[0].text.length < 'Faisceaux'.length).toBe(true);
  expect(out[1].text).toBe(''); // 15 px: no room for two characters and « … »
  expect(out[2].text).toBe('Ciseaux');
  for (let i = 0; i + 1 < out.length; i++) expect(out[i].x1, `label ${i} ends before flag ${i + 1}`).toBeLessThanOrEqual(layout.marks[i + 1].x - 4);
  expect(out[3].x1).toBeLessThanOrEqual(260 - 4);

  // On the example show: no two drawn labels overlap, hovering one gives its name.
  await page.locator('#tlExamples').click();
  const dlg = page.locator('#tlExDialog');
  await dlg.locator('.tl-ex', { hasText: 'Montée 16 mesures' }).click();
  await expect(dlg).toBeHidden();
  await page.locator('#tlFit').click();
  const labels = await page.evaluate(async () => {
    const { TL } = await import('/ui/timeline-editor.js' as string);
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
    return (TL.markerLabels as { text: string; x0: number; x1: number }[]).map((l, i) => ({ ...l, name: TL.show.markers[i].name }));
  });
  expect(labels.length).toBeGreaterThan(3);
  const drawn = labels.filter(l => l.text);
  for (let i = 0; i + 1 < drawn.length; i++) expect(drawn[i].x1, `${drawn[i].text} / ${drawn[i + 1].text}`).toBeLessThanOrEqual(drawn[i + 1].x0 - 11);
  const cut = labels.find(l => l.text && l.text !== l.name) ?? drawn[0];
  const cv = await box(page.locator('#tlCanvas'));
  await page.mouse.move(cv.x + cut.x0 + 3, cv.y + 30 + 9); // TL_RULER + TL_MARK / 2
  await expect(page.locator('#tlCanvas')).toHaveAttribute('title', cut.name);
  await page.mouse.move(cv.x + 10, cv.y + 120);
  await expect(page.locator('#tlCanvas')).not.toHaveAttribute('title', cut.name);
});

test('show mode: the tracks and the playhead stay on screen, read only', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openUi(page, studio);
  await openWorkspace(page, 'timeline');
  await page.locator('#tlExamples').click();
  await page.locator('#tlExDialog .tl-ex', { hasText: 'Montée 16 mesures' }).click();
  await expect(page.locator('#tlExDialog')).toBeHidden();
  const before = (await studio.get('/api/timeline')).show;
  await page.locator('#lockBtn').click();
  await expect.poll(async () => (await studio.get('/api/lock')).show_mode).toBe(true);
  await expect(page.locator('#tlEditor')).toBeVisible();
  await expect(page.locator('#tlTracks')).toBeVisible();
  await expect(page.locator('#tlCanvas')).toBeVisible();
  await expect(page.locator('#tlEditor .tl-tools')).toBeHidden();
  await expect(page.locator('#tlEditor .tl-lib')).toBeHidden();
  await expect(page.locator('#tlPlay')).toBeVisible();
  // A drag on an event moves nothing; a double-click adds no marker.
  const cv = await box(page.locator('#tlCanvas'));
  await page.mouse.move(cv.x + 120, cv.y + 110);
  await page.mouse.down();
  await page.mouse.move(cv.x + 260, cv.y + 110, { steps: 6 });
  await page.mouse.up();
  await expect(page.locator('#lockToast')).toHaveText('Verrouillé en mode spectacle');
  await page.mouse.dblclick(cv.x + 300, cv.y + 40);
  expect((await studio.get('/api/timeline')).show).toEqual(before);
  // Unlocking (a 1 s hold) brings the toolbar back.
  await page.locator('#lockBtn').hover();
  await page.mouse.down();
  await page.waitForTimeout(1300);
  await page.mouse.up();
  await expect.poll(async () => (await studio.get('/api/lock')).show_mode).toBe(false);
  await expect(page.locator('#tlEditor .tl-tools')).toBeVisible();
});
