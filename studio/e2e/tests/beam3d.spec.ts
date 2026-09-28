// 3D beam view (T-275, T-276): the "3D" button next to the 2D preview,
// the pure geometry of beam3d.js, and its energy-conserving rendering.
// Preview only: the view just draws what /api/frame already says.
import { test, expect, useStudio, openUi } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

/** Console errors and uncaught exceptions, plus every request that leaves the studio. */
function watch(page: Page) {
  const errors: string[] = [], external: string[] = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push(String(e)));
  // blob:<studio>/… is the page's own heartbeat worker (T-252), not a request out.
  page.on('request', r => { if (!r.url().startsWith(studio.url) && !r.url().startsWith('blob:' + studio.url + '/')) external.push(r.url()); });
  return { errors, external };
}

/** Evaluate inside the page with the beam3d module (window.__b3 in the callback). */
async function withModule<T>(page: Page, fn: string): Promise<T> {
  return page.evaluate(async src => {
    (window as any).__b3 = await import('/beam3d.js');
    return new Function('return (' + src + ')(window.__b3)')();
  }, fn);
}

test.beforeEach(async () => {
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
});

test('switch to 3D, a WebGL canvas renders, back to 2D', async ({ page }) => {
  const seen = watch(page);
  await openUi(page, studio);
  await expect(page.locator('#preview')).toBeVisible();
  await expect(page.locator('#beams')).toBeHidden();

  await page.locator('#viewSeg [data-view="3d"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#preview')).toBeHidden();
  await expect(page.locator('#viewSeg [data-view="3d"]')).toHaveClass(/active/);
  await expect(page.locator('#recenter3d')).toBeVisible();
  await expect(page.locator('#render3d')).toBeVisible();
  await expect(page.locator('#hint3d')).toHaveText('Glisser pour tourner, molette pour zoomer, Maj+glisser pour déplacer');
  // The canvas holds a WebGL2 context and something lit gets drawn.
  const kind = await page.evaluate(() => {
    const c = document.getElementById('beams') as HTMLCanvasElement;
    return c.getContext('webgl2') ? 'webgl2' : c.getContext('2d') ? '2d' : 'none';
  });
  expect(kind).toBe('webgl2');
  // Lit pixels in the 3D image (read right after a draw: the WebGL buffer
  // is not kept once shown).
  await expect.poll(() => page.evaluate(`(() => {
    const v = view3d; v.draw();
    const gl = v.gl, W = v.canvas.width, H = v.canvas.height, d = new Uint8Array(W * H * 4);
    gl.readPixels(0, 0, W, H, gl.RGBA, gl.UNSIGNED_BYTE, d);
    let lit = 0;
    for (let i = 0; i < d.length; i += 16) if (d[i] + d[i + 1] + d[i + 2] > 60) lit++;
    return lit;
  })()`)).toBeGreaterThan(50);
  // The 3D view never changes the studio: still disarmed, same look.
  expect((await studio.state()).armed).toBe(false);

  await page.locator('#viewSeg [data-view="2d"]').click();
  await expect(page.locator('#preview')).toBeVisible();
  await expect(page.locator('#beams')).toBeHidden();
  await expect(page.locator('#render3d')).toBeHidden();
  // The 2D preview keeps drawing: the default circle is back on it.
  await expect.poll(() => page.evaluate(() => {
    const c = document.getElementById('preview') as HTMLCanvasElement;
    const d = c.getContext('2d')!.getImageData(0, 0, c.width, c.height).data;
    let lit = 0;
    for (let i = 0; i < d.length; i += 16) if (d[i + 1] > 100) lit++;
    return lit;
  })).toBeGreaterThan(100);

  expect(seen.errors).toEqual([]);
  expect(seen.external).toEqual([]);
});

test('the 3D view follows the studio: a fan cue becomes diverging beams', async ({ page }) => {
  const seen = watch(page);
  await openUi(page, studio);
  await page.locator('#viewSeg [data-view="3d"]').click();
  const presets = await studio.presets();
  const fan = presets.presets.find(p => p.name.startsWith('Éventail'))!;
  expect(fan).toBeTruthy();
  await studio.post('/api/cue', { id: fan.id });
  // Several beams from the projector, spread across the back wall (z = 0):
  // at least 4 m between the outermost hits of a 10 m wide room.
  const spread = () => page.evaluate(`(() => {
    const v = view3d; if (!v || !v.counts.beams) return 0;
    const b = v.geo.beam; let lo = Infinity, hi = -Infinity;
    for (let i = 0; i < v.counts.beams; i++) { if (Math.abs(b[i * 7 + 2]) > 0.01) continue; lo = Math.min(lo, b[i * 7]); hi = Math.max(hi, b[i * 7]); }
    return hi - lo;
  })()`);
  await expect.poll(spread).toBeGreaterThan(4);
  await page.screenshot({ path: 'test-results/beam3d-fan.png' });
  expect(seen.errors).toEqual([]);
});

test('geometry: centre point is a horizontal beam straight ahead to the back wall', async ({ page }) => {
  await openUi(page, studio);
  const r = await withModule<any>(page, `m => {
    const p = m.DEFAULT_PROJECTOR, room = m.DEFAULT_ROOM;
    const d = m.pointToDir(0, 0, p);
    const hit = m.rayBox(p.pos, d, room);
    const right = m.pointToDir(1, 0, p), up = m.pointToDir(0, 1, p);
    const a = m.beamAlpha(200, 1);
    const pts = [[0,0,0,1,0],[0.1,0,0,1,0],[0.2,0,0,1,0],[0.3,0,0,0,0],[0.4,0,0,1,0],[0.5,0,0,1,0]];
    const counts = m.buildGeometry(pts, p, room, {});
    const q = new m.AutoQuality();
    let t = 0;
    for (let i = 0; i < 60; i++) { t += 16; q.sample(16, t); }
    const stillHigh = q.name;
    for (let i = 0; i < 150; i++) { t += 25; q.sample(25, t); }
    return { d, hit, right, up, a, counts, stillHigh, loaded: q.name, low: m.qualityFeatures(0, m.DEFAULT_RENDER) };
  }`);
  // (0, 0): horizontal, straight towards the audience (-z), hits the back wall (face 4).
  expect(r.d[0]).toBeCloseTo(0, 6);
  expect(r.d[1]).toBeCloseTo(0, 6);
  expect(r.d[2]).toBeCloseTo(-1, 6);
  expect(r.hit.face).toBe(4);
  expect(r.hit.t).toBeCloseTo(11, 6);
  // x = 1 turns θ/2 = 20° to the projector's right (+x), y = 1 tilts 20° up.
  expect(Math.atan2(r.right[0], -r.right[2]) * 180 / Math.PI).toBeCloseTo(20, 4);
  expect(Math.asin(r.up[1]) * 180 / Math.PI).toBeCloseTo(20, 4);
  // Energy: exposure / N; sheets = consecutive lit pairs (the blank breaks one).
  expect(r.a).toBeCloseTo(0.005, 9);
  expect(r.counts).toEqual({ beams: 5, sheets: 3 });
  // Auto quality: fine at 16 ms, drops to medium after 2 s over 20 ms.
  expect(r.stillHigh).toBe('high');
  expect(r.loaded).toBe('medium');
  expect(r.low).toEqual({ sheets: false, bloomLevels: 0, noise: false, scale: 0.5 });
});

test('a static beam is much brighter than a 200-point sweep; no haze leaves only the spots', async ({ page }) => {
  await openUi(page, studio);
  const r = await withModule<any>(page, `m => {
    const cv = document.createElement('canvas');
    cv.width = cv.height = 400; cv.style.width = cv.style.height = '400px';
    document.body.appendChild(cv);
    const view = new m.BeamView(cv, { render: { ...m.DEFAULT_RENDER, quality: 'high', bloom: false, roomLight: 0 } });
    const p = m.DEFAULT_PROJECTOR;
    // From above, so the horizontal sweep is seen face-on (not edge-on,
    // where its lines would pile up on screen).
    view.setCamera({ el: 1.45 });
    // Mid-beam and wall hit of the (0, 0) beam.
    const mid = [p.pos[0], p.pos[1], p.pos[2] - 5.5], wall = [0, 3, 0];
    const green = (x) => [x, 0, 0, 1, 0];
    view.setFrame(Array.from({ length: 200 }, () => green(0)));
    const [beamMid, beamWall] = view.probe([mid, wall]);
    view.setFrame(Array.from({ length: 200 }, (_, i) => green(-1 + 2 * i / 199)));
    const [sweepMid] = view.probe([mid]);
    view.setFrame(Array.from({ length: 200 }, () => green(0)));
    view.setRender({ haze: 0 });
    const [noHazeMid, noHazeWall] = view.probe([mid, wall]);
    cv.remove();
    return { beamMid, beamWall, sweepMid, noHazeMid, noHazeWall };
  }`);
  expect(r.beamMid).toBeGreaterThan(150);
  expect(r.beamMid).toBeGreaterThan(3 * r.sweepMid);
  expect(r.noHazeMid).toBe(0);
  expect(r.noHazeWall).toBeGreaterThan(50);
});
