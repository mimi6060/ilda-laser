// Scan limits of the output (T-171): speed range and points per frame in
// RÉGLAGES › Sorties et projecteurs, the « Images/s » alert, and the live
// « Vitesse de balayage ». Applied before the safety stage.
// Preview only: the studio has no output (no --device) and stays disarmed.
import { test, expect, useStudio, openUi, reveal } from '../studio';

const studio = useStudio();
const DEFAULTS = { pps_min: 10000, pps_max: 40000, min_points: 200, max_points: 0, min_fps: 40 };
const scan = async () => (await studio.get('/api/outputs/scan_limits')).scan;

test.beforeEach(async ({ page }) => {
  await studio.reset();
  expect(await studio.post('/api/outputs/scan_limits', { scan: DEFAULTS })).toBe(200);
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/outputs/scan_limits', { scan: DEFAULTS });
});

test('points minimum completes the frame and the frame rate alert follows', async ({ page }) => {
  await reveal(page, '#oscanMinPts');
  await expect(page.locator('#oscanMinPts')).toHaveValue('200');
  await expect(page.locator('#oscanInfo')).toContainText('30000 points/s');
  await page.locator('#oscanMinPts').fill('2000');
  await page.locator('#oscanSave').click();
  await expect.poll(async () => (await scan()).min_points).toBe(2000);
  // Every frame now has at least 2000 points: 30 000 / 2000 = 15 images/s.
  await expect.poll(async () => (await studio.frame()).points.length).toBeGreaterThanOrEqual(2000);
  await expect(page.locator('#stFps')).toHaveClass(/bad/);
  // Back to 200: above 35 images/s again for a light look.
  await page.locator('#oscanMinPts').fill('200');
  await page.locator('#oscanSave').click();
  await expect.poll(async () => (await scan()).min_points).toBe(200);
});

test('a speed max below the rate warns, out of range is refused, faster is refused armed', async ({ page }) => {
  await reveal(page, '#oscanPpsMax');
  await page.locator('#oscanPpsMax').fill('25000');
  await page.locator('#oscanSave').click();
  await expect(page.locator('#oscanWarn')).toContainText('au-dessus de la vitesse max');
  expect((await scan()).pps_max).toBe(25000);
  // Min above max: refused with the reason, nothing changed.
  await page.locator('#oscanPpsMin').fill('30000');
  await page.locator('#oscanSave').click();
  await expect(page.locator('#oscanError')).toContainText('Vitesse min');
  expect((await scan()).pps_min).toBe(10000);
  // Armed: a faster max is refused.
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await page.locator('#oscanPpsMax').fill('40000');
  await page.locator('#oscanSave').click();
  await expect(page.locator('#oscanError')).toContainText('Désarmez');
  expect((await scan()).pps_max).toBe(25000);
});

test('the live scan speed is a master slider', async ({ page }) => {
  await reveal(page, '#mScan');
  await expect(page.locator('#mScanV')).toHaveText('100 %');
  await page.locator('#mScan').fill('150');
  await expect.poll(async () => (await studio.live()).scan).toBeCloseTo(1.5, 3);
  // T-431: a double-click on the slider types a value; on its name, neutral.
  await page.locator('label', { has: page.locator('#mScanV') }).dblclick();
  await expect.poll(async () => (await studio.live()).scan).toBe(1);
});
