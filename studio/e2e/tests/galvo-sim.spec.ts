// Galvo simulation and divergence of the previews (T-280): RÉGLAGES ›
// Simulation de l'aperçu. The page runs the frame through a second-order
// low-pass filter at the output's point rate (corners round off), warns
// about a frame longer than 1/25 s, and widens the 3D beams with the
// divergence. Display only: /api/frame and the output never change.
// Preview only: the studio has the fake output and stays disarmed.
import { test, expect, useStudio, openUi, reveal } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
const SCAN_DEFAULTS = { pps_min: 10000, pps_max: 40000, min_points: 200, max_points: 0, min_fps: 40 };
const SIM_DEFAULTS = { enabled: false, response_ms: 0.3, divergence_mrad: 1 };
const render = async () => (await studio.get('/api/venue')).venue.render;

async function openPanel(page: Page) {
  await reveal(page, '#galvoPanel summary');
  await page.locator('#galvoPanel summary').click();
  await expect(page.locator('#galvoOn')).toBeVisible();
}

test.beforeEach(async () => {
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  expect(await studio.post('/api/venue/sim', SIM_DEFAULTS)).toBe(200);
  expect(await studio.post('/api/outputs/scan_limits', { scan: SCAN_DEFAULTS })).toBe(200);
});

test.afterEach(async () => {
  await studio.post('/api/venue/sim', SIM_DEFAULTS);
  await studio.post('/api/outputs/scan_limits', { scan: SCAN_DEFAULTS });
});

test('step response of the filter: rise time, overshoot, a static beam stays put', async ({ page }) => {
  await openUi(page, studio);
  const r = await page.evaluate(() => {
    const g = (window as any).__galvo;
    const step = (pps: number, ms: number) => {
      const pts = Array.from({ length: 2000 }, () => [1, 0, 0, 1, 0]);
      const out = g.filter(pts, pps, ms, { cyclic: false, from: [0, 0] }).map((p: number[]) => p[0]);
      const at = (v: number) => out.findIndex((x: number) => x >= v);
      return { riseMs: (at(0.9) - at(0.1)) / pps * 1000, overshoot: Math.max(...out) - 1, end: out[out.length - 1] };
    };
    const still = g.filter(Array.from({ length: 50 }, () => [0.3, -0.2, 1, 1, 1]), 30000, 1);
    return { fast: step(30000, 0.3), slow: step(30000, 2), lowRate: step(10000, 1), still, frameS: g.frameS(3000, 30000) };
  });
  for (const [s, ms] of [[r.fast, 0.3], [r.slow, 2], [r.lowRate, 1]] as const) {
    // 10–90 % rise time = « Réponse (ms) », within a sample or 10 %.
    expect(Math.abs(s.riseMs - ms)).toBeLessThan(Math.max(0.1 * ms, 0.11));
    // Damping 0.7: about 4.6 % overshoot, then settled.
    expect(s.overshoot).toBeGreaterThan(0.03);
    expect(s.overshoot).toBeLessThan(0.06);
    expect(Math.abs(s.end - 1)).toBeLessThan(1e-3);
  }
  // A static beam (one point repeated) is drawn exactly where it is, with its colour.
  expect(r.still.every((p: number[]) => Math.abs(p[0] - 0.3) < 1e-9 && Math.abs(p[1] + 0.2) < 1e-9 && p[2] === 1)).toBe(true);
  expect(r.frameS).toBeCloseTo(0.1, 6);
});

test('a square at 30 kpps: corners rounded with the simulation, sharp without', async ({ page }) => {
  await openUi(page, studio);
  const r = await page.evaluate(() => {
    const g = (window as any).__galvo;
    const corners = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];
    const pts: number[][] = [];
    for (let c = 0; c < 4; c++) {
      const [a, b] = [corners[c], corners[(c + 1) % 4]];
      for (let i = 0; i < 60; i++) pts.push([a[0] + (b[0] - a[0]) * i / 60, a[1] + (b[1] - a[1]) * i / 60, 0, 1, 0]);
    }
    // How close the trace comes to each corner (0 = it reaches it).
    const miss = (trace: number[][]) => Math.max(...corners.map(k => Math.min(...trace.map(p => Math.hypot(p[0] - k[0], p[1] - k[1])))));
    return { sharp: miss(pts), rounded: miss(g.filter(pts, 30000, 0.3)), slower: miss(g.filter(pts, 30000, 1)), n: g.filter(pts, 30000, 0.3).length };
  });
  expect(r.sharp).toBe(0);
  expect(r.rounded).toBeGreaterThan(0.01);
  expect(r.slower).toBeGreaterThan(r.rounded);
  expect(r.n).toBe(240);
});

test('turned on in RÉGLAGES: saved, labelled an approximation, /api/frame unchanged', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(String(e)));
  await openUi(page, studio);
  await expect(page.locator('#galvoBadge')).toBeHidden();
  const before = await studio.frame();
  await openPanel(page);
  await expect(page.locator('#galvoPanel')).toContainText('Approximation');
  await page.locator('#galvoOn').check();
  await expect.poll(async () => (await render()).galvo.enabled).toBe(true);
  await expect(page.locator('#galvoBadge')).toBeVisible();
  await expect(page.locator('#galvoBadge')).toHaveText('Galvos simulés : approximation');
  // The preview draws the simulated trace; the frame the studio sends is the same.
  await expect.poll(() => page.evaluate('lastShown !== lastPoints && lastShown.length === lastPoints.length')).toBe(true);
  const after = await studio.frame();
  expect(after.points).toEqual(before.points);
  expect(after.armed).toBe(false);
  // Réponse: a slider, clamped by the studio.
  await page.locator('#galvoResp').fill('1');
  await expect.poll(async () => (await render()).galvo.response_ms).toBeCloseTo(1, 5);
  await expect(page.locator('#galvoRespV')).toHaveText('1,00 ms');
  expect(await studio.post('/api/venue/sim', { response_ms: 50, divergence_mrad: -1 })).toBe(200);
  expect((await render()).galvo.response_ms).toBe(5);
  expect((await render()).divergence_mrad).toBe(0);
  expect(await studio.post('/api/venue/sim', { response_ms: 'vite' })).toBe(400);
  // Divergence slider.
  await page.locator('#galvoDiv').fill('2.5');
  await expect.poll(async () => (await render()).divergence_mrad).toBeCloseTo(2.5, 5);
  // Off again: the preview draws the frame as sent.
  await page.locator('#galvoOn').uncheck();
  await expect.poll(async () => (await render()).galvo.enabled).toBe(false);
  await expect(page.locator('#galvoBadge')).toBeHidden();
  await expect.poll(() => page.evaluate('lastShown === lastPoints')).toBe(true);
  // Kept across a reload (venue.json).
  await page.locator('#galvoOn').check();
  await expect.poll(async () => (await render()).galvo.enabled).toBe(true);
  await page.reload();
  await expect(page.locator('#galvoBadge')).toBeVisible();
  expect(errors).toEqual([]);
});

test('a frame of 3000 points at 30 kpps warns « Image trop longue »', async ({ page }) => {
  await openUi(page, studio);
  expect(await studio.post('/api/venue/sim', { enabled: true })).toBe(200);
  await page.reload();
  await expect(page.locator('#galvoBadge')).toBeVisible();
  await expect(page.locator('#galvoLong')).toBeHidden();
  // Every frame now has at least 3000 points: 3000 / 30 000 = 100 ms > 1/25 s.
  expect(await studio.post('/api/outputs/scan_limits', { scan: { ...SCAN_DEFAULTS, min_points: 3000 } })).toBe(200);
  await expect.poll(async () => (await studio.frame()).points.length).toBeGreaterThanOrEqual(3000);
  await expect(page.locator('#galvoLong')).toBeVisible();
  await expect(page.locator('#galvoLong')).toHaveText('Image trop longue : scintillement probable');
  // The flicker dims the preview a little, never hides it.
  const gains = await page.evaluate(async () => {
    const seen: number[] = [];
    for (let i = 0; i < 30; i++) { seen.push((window as any).__galvo.settings().gain); await new Promise(r => setTimeout(r, 20)); }
    return seen;
  });
  expect(Math.min(...gains)).toBeGreaterThanOrEqual(0.7);
  expect(Math.max(...gains)).toBeLessThanOrEqual(1);
  // A light frame again: no warning.
  expect(await studio.post('/api/outputs/scan_limits', { scan: SCAN_DEFAULTS })).toBe(200);
  await expect(page.locator('#galvoLong')).toBeHidden();
});

test('3D: an audience hit of the sent frame counts even if the simulated trace misses it; divergence widens beams', async ({ page }) => {
  await openUi(page, studio);
  const r = await page.evaluate(async () => {
    const m = await import('/beam3d.js');
    const box = m.audienceBox({ rect_m: [4, 9, -4, 1], min_height_m: 3 });
    const p = m.DEFAULT_PROJECTOR, room = m.DEFAULT_ROOM;
    // Sent: one point down into the audience; simulated: it never got there.
    const raw = [[0, 0.2, 0, 1, 0], [0, -0.6, 0, 1, 0], [0, 0.2, 0, 1, 0]];
    const sim = [[0, 0.2, 0, 1, 0], [0, 0.1, 0, 1, 0], [0, 0.2, 0, 1, 0]];
    const alone = m.buildGeometry(sim, p, room, {}, true, { beams: 0, sheets: 0 }, box).inAudience;
    const withRaw = m.buildGeometry(sim, p, room, {}, true, { beams: 0, sheets: 0 }, box, raw).inAudience;
    // Divergence: the same static beam (along -z, 3 m high) seen from the
    // side at 1.2 m, 7 m from the projector: a column across it, thin vs wide.
    const c = document.createElement('canvas'); c.width = c.height = 400;
    const v = new m.BeamView(c, { render: { ...m.DEFAULT_RENDER, bloom: false, spots: false, sheets: false, quality: 'high' } });
    v.setFrame(Array.from({ length: 10 }, () => [0, 0, 0, 1, 0]));
    v.setPose({ pos: [1.2, 3, 4], target: [0, 3, 4], fov: 50 }, 0);
    const width = () => {
      v.draw();
      const gl = v.gl, d = new Uint8Array(400 * 4);
      gl.readPixels(200, 0, 1, 400, gl.RGBA, gl.UNSIGNED_BYTE, d);
      let n = 0;
      for (let i = 0; i < d.length; i += 4) if (d[i + 1] > 20) n++;
      return n;
    };
    const thin = width();
    v.setSim({ divergenceMrad: 20 });
    const wide = width();
    v.setSim({ gain: 0, divergenceMrad: Number.NaN });
    return { alone, withRaw, thin, wide, sim: v.sim };
  });
  expect(r.alone).toBe(0);
  expect(r.withRaw).toBe(1);
  expect(r.thin).toBeGreaterThan(0);
  expect(r.wide).toBeGreaterThan(r.thin);
  // Clamped: the gain never hides a beam, a NaN is ignored.
  expect(r.sim).toEqual({ gain: 0.5, divergenceMrad: 20 });
});
