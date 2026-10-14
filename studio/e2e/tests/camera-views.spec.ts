// Points de vue caméra (T-278): ready-made viewpoints of the 3D view,
// computed from the venue, reached in 400 ms, on Alt+Maj+1…6, saved in
// venue.json. Display only: the camera never changes the output, and the
// test studio has no laser output anyway.
import { test, expect, useStudio, openUi, open3dSettings, focusPage } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
type Cam = { preset: string; pos: number[]; target: number[]; fov: number; orbit: string; moving: boolean };
type Pose = { pos: number[]; target: number[]; fov: number };

const camera = (page: Page) => page.evaluate(() => (window as any).__viz.camera() as Cam);
const presets = (page: Page) => page.evaluate(() => (window as any).__viz.presets() as Record<string, Pose>);
/** The camera once its transition is over. */
async function settled(page: Page): Promise<Cam> {
  await expect.poll(async () => (await camera(page)).moving, { timeout: 3000 }).toBe(false);
  return camera(page);
}
const near = (a: number[], b: number[], tol = 0.01) => a.length === b.length && a.every((v, i) => Math.abs(v - b[i]) <= tol);
async function open3d(page: Page) {
  await openUi(page, studio);
  await page.locator('#viewSeg [data-view="3d"]').click();
  await open3dSettings(page);
  await expect(page.locator('#cam3dSeg')).toBeVisible();
  await expect.poll(() => page.evaluate(() => !!(window as any).__viz.camera())).toBe(true);
}

test.beforeEach(async () => {
  await studio.reset();
  await studio.post('/api/venue/camera', { preset: '' });
});

test('positions computed from a venue (pure geometry, ±1 cm)', async ({ page }) => {
  await openUi(page, studio);
  const r = await page.evaluate(async () => {
    const b = await import('/beam3d.js' as string);
    const p = b.cameraPresets({ room: { w: 10, d: 15, h: 5 }, projectors: [{ pos: [0, 3, 11] }], audience: { rect_m: [-4, 1, 4, 9], min_height_m: 3 } });
    const o = b.orbitFromPose([1, 2, 3], [4, 5, 9]);
    return { p, back: b.eyeOf(o), order: b.CAMERA_PRESETS };
  });
  const reach = Math.tan(25 * Math.PI / 180), top = 7.5 / reach * 1.1, side = 7.5 / reach * 1.1 + 5;
  const want: Record<string, [number[], number[]]> = {
    public: [[0, 1.7, 0.5], [0, 3, 15]],
    front: [[0, 1.7, 9], [0, 3, 15]],
    stage: [[0, 3.5, 12.5], [0, 1.7, 5]],
    top: [[0, top, 7.5 - top * 0.01], [0, 0, 7.5]],
    side: [[-side, 2.5, 7.5], [0, 2.5, 7.5]],
  };
  for (const [k, [pos, target]] of Object.entries(want)) {
    expect(near(r.p[k].pos, pos), `${k} pos ${r.p[k].pos}`).toBe(true);
    expect(near(r.p[k].target, target), `${k} target ${r.p[k].target}`).toBe(true);
    expect(r.p[k].fov).toBe(50);
  }
  expect(near(r.back, [1, 2, 3], 1e-6)).toBe(true);
  expect(r.order).toEqual(['public', 'front', 'stage', 'top', 'side', 'free']);
});

test('each button places the camera, smoothly, and the choice is saved', async ({ page }) => {
  await open3d(page);
  const p = await presets(page);
  // Labels as the task names them.
  await expect(page.locator('#cam3dSeg button')).toHaveText(['Public (régie)', 'Premier rang', 'Scène', 'Dessus', 'Côté', 'Libre']);
  await expect(page.locator('label[for="cam3dOrbit"]')).toHaveText('Tourner autour');
  for (const id of ['public', 'front', 'stage', 'top', 'side']) {
    await page.locator(`#cam3dSeg [data-cam="${id}"]`).click();
    const c = await settled(page);
    expect(c.preset).toBe(id);
    expect(near(c.pos, p[id].pos), `${id}: ${c.pos} vs ${p[id].pos}`).toBe(true);
    expect(near(c.target, p[id].target), `${id} target`).toBe(true);
    await expect(page.locator(`#cam3dSeg [data-cam="${id}"]`)).toHaveAttribute('aria-pressed', 'true');
  }
  // A transition: under way right after a click, done after 400 ms.
  await page.locator('#cam3dSeg [data-cam="public"]').click();
  const during = await camera(page);
  expect(during.moving).toBe(true);
  expect(near(during.pos, p.public.pos)).toBe(false);
  await page.waitForTimeout(500);
  expect(near((await camera(page)).pos, p.public.pos)).toBe(true);
  // Saved in the venue; nothing armed, no history entry.
  await expect.poll(async () => (await studio.get('/api/venue')).venue.camera.preset).toBe('public');
  expect((await studio.state()).armed).toBe(false);
});

test('Libre goes back to the last manual position; the orbit modes', async ({ page }) => {
  await open3d(page);
  await page.locator('#cam3dSeg [data-cam="top"]').click();
  await settled(page);
  // A drag: the viewpoint becomes « Libre ». The settings drawer (T-503)
  // lies over the bottom of the view: closed first, as the operator would.
  await page.locator('#render3dClose').click();
  await expect(page.locator('#render3d')).toBeHidden();
  const box = (await page.locator('#beams').boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 80, box.y + box.height / 2 + 20, { steps: 5 });
  await page.mouse.up();
  const free = await camera(page);
  expect(free.preset).toBe('free');
  await open3dSettings(page);
  await expect(page.locator('#cam3dSeg [data-cam="free"]')).toHaveAttribute('aria-pressed', 'true');
  await page.locator('#cam3dSeg [data-cam="side"]').click();
  await settled(page);
  await page.locator('#cam3dSeg [data-cam="free"]').click();
  const back = await settled(page);
  expect(back.preset).toBe('free');
  expect(near(back.pos, free.pos)).toBe(true);
  // Turning around the projector looks at it; the camera mode keeps the eye.
  await page.locator('#cam3dOrbit').selectOption('projector');
  const pr = await settled(page);
  expect(near(pr.target, [0, 3, 11])).toBe(true);
  await page.locator('#cam3dOrbit').selectOption('camera');
  const eye = (await camera(page)).pos;
  await page.locator('#render3dClose').click(); // the drag again goes to the view, not the drawer
  await expect(page.locator('#render3d')).toBeHidden();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 - 60, box.y + box.height / 2, { steps: 4 });
  await page.mouse.up();
  const turned = await camera(page);
  expect(near(turned.pos, eye)).toBe(true);
  expect(near(turned.target, pr.target)).toBe(false);
  expect(turned.orbit).toBe('camera');
  await open3dSettings(page);
  await page.locator('#cam3dOrbit').selectOption('room');
});

test('Alt+Maj+1…6 pick a viewpoint while the 3D view is shown, never a cue', async ({ page }) => {
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await open3d(page);
  await focusPage(page);
  const before = await studio.controlValues();
  const p = await presets(page);
  for (const [i, id] of ['public', 'front', 'stage', 'top', 'side'].entries()) {
    await page.keyboard.press(`Alt+Shift+Digit${i + 1}`);
    const c = await settled(page);
    expect(c.preset, `Alt+Maj+${i + 1}`).toBe(id);
    expect(near(c.pos, p[id].pos)).toBe(true);
  }
  await page.keyboard.press('Alt+Shift+Digit6');
  expect((await settled(page)).preset).toBe('free');
  const after = await studio.controlValues();
  expect(after.active_cue).toBe(before.active_cue);
  expect(after.cue_page).toBe(before.cue_page);
  // In the help, under « Vue 3D ».
  expect(await page.evaluate(() => (window as any).shortcutsDebug.conflicts([]))).toEqual([]);
  // 2D only: the keys do nothing.
  await page.locator('#viewSeg [data-view="2d"]').click();
  await page.keyboard.press('Alt+Shift+Digit4');
  expect((await camera(page)).preset).toBe('free');
});

test('the viewpoint is back after a reload, and can change in show mode', async ({ page }) => {
  await open3d(page);
  await page.locator('#cam3dSeg [data-cam="side"]').click();
  await settled(page);
  await expect.poll(async () => (await studio.get('/api/venue')).venue.camera.preset).toBe('side');
  await open3d(page);
  const c = await settled(page);
  expect(c.preset).toBe('side');
  expect(near(c.pos, (await presets(page)).side.pos)).toBe(true);
  await expect(page.locator('#cam3dSeg [data-cam="side"]')).toHaveAttribute('aria-pressed', 'true');
  // Show mode: a viewpoint is a view, not an edit.
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  await page.locator('#cam3dSeg [data-cam="stage"]').click();
  await expect.poll(async () => (await studio.get('/api/venue')).venue.camera.preset).toBe('stage');
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
});
