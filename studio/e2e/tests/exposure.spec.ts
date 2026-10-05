// Exposure estimator (T-257): RÉGLAGES › Estimation d'exposition turns the
// projector sheet, the output cap and the audience distance into an MPE
// estimate and a hazard distance. Informative only: editing its fields
// never changes the output, the caps or the arming.
// Preview only: the studio has no output (no --device).
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

const peak = (pts: Point[]) => Math.max(0, ...pts.filter(isLit).map(p => Math.max(p[2], p[3], p[4])));

async function openExposure(page: Page) {
  await reveal(page, '#exposurePanel');
  await page.locator('#exposurePanel > summary').click();
  await expect(page.locator('#exDistance')).not.toHaveValue('');
}

test.beforeEach(async ({ page }) => {
  // 1 W in all (300 + 400 + 300 mW), 5 mm, 1 mrad; the default 50 % cap.
  expect(await studio.post('/api/outputs/limits', { projector: { name: 'Banc', power_mw: [300, 400, 300], wavelength_nm: [638, 520, 450], aperture_mm: 5, divergence_mrad: 1 } })).toBe(200);
  expect(await studio.post('/api/safety/exposure', { exposure: { distance_m: 3, attenuation_pct: 0 } })).toBe(200);
  await studio.reset();
  await openUi(page, studio);
  await studio.post('/api/settings', { ...(await studio.state()).settings, content: { kind: 'shape', shape: 'circle' }, color: [255, 255, 255], brightness: 1 });
});

test('a watt-class beam at 10 m is shown in red, with the disclaimer, and the output does not change', async ({ page }) => {
  await openExposure(page);
  await expect(page.locator('#exBanner')).toContainText('ne remplace pas une mesure');
  const before = { frame: peak((await studio.frame()).points), limits: (await studio.get('/api/outputs/limits')).outputs[0].limits, armed: (await studio.state()).armed };
  await page.locator('#exDistance').fill('10');
  await page.locator('#exDistance').press('Tab');
  // 500 mW (50 % cap) on a 15 mm beam: 282.9 mW/cm², × 111 the MPE; the
  // NOHD of a fixed beam: sqrt(4 × 500 / (π × 2.5456)) = 15.81 cm → 153.1 m.
  await expect.poll(async () => (await studio.get('/api/safety/exposure')).settings.distance_m).toBe(10);
  await expect(page.locator('#exVerdict')).toHaveAttribute('data-over', 'true');
  await expect(page.locator('#exVerdict')).toContainText('au-dessus');
  await expect(page.locator('#exMpe')).toContainText('2,55');
  await expect(page.locator('#exIrr')).toContainText('282,9');
  await expect(page.locator('#exNohd')).toContainText('153,1');
  await expect(page.locator('#exSuggest')).toContainText('À titre indicatif');
  const after = { frame: peak((await studio.frame()).points), limits: (await studio.get('/api/outputs/limits')).outputs[0].limits, armed: (await studio.state()).armed };
  expect(after.limits).toEqual(before.limits);
  expect(after.armed).toBe(before.armed);
  expect(after.frame).toBeCloseTo(before.frame, 3);
});

test('a heavily attenuated low-power beam is shown in green, never as safe', async ({ page }) => {
  expect(await studio.post('/api/outputs/limits', { projector: { power_mw: [0, 1, 0], aperture_mm: 5, divergence_mrad: 1 } })).toBe(200);
  await openExposure(page);
  await page.locator('#exAttenuation').fill('90');
  await page.locator('#exAttenuation').press('Tab');
  await expect(page.locator('#exVerdict')).toHaveAttribute('data-over', 'false');
  await expect(page.locator('#exVerdict')).toContainText('pas une garantie');
  await expect(page.locator('#exVerdict')).not.toContainText(/\bsûr\b/);
});

test('an out-of-range distance is refused and the saved value kept', async ({ page }) => {
  await openExposure(page);
  await page.locator('#exDistance').fill('-5');
  await page.locator('#exDistance').press('Tab');
  await expect(page.locator('#exError')).toContainText('hors limites');
  expect((await studio.get('/api/safety/exposure')).settings.distance_m).toBe(3);
  await expect(page.locator('#exDistance')).toHaveValue('3');
});
