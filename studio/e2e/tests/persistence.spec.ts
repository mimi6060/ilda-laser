// Calibration (and the other saved state) survives a studio restart on
// the same --data-dir; the output always stays inside -1..1.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi, centroid, extent } from '../studio';

const studio = useStudio();
const calibration = async () => (await studio.state()).calibration;
const readJson = (file: string) => {
  try { return JSON.parse(readFileSync(path.join(studio.dataDir, file), 'utf8')); } catch { return null; }
};

test.describe.configure({ mode: 'serial' });

test('calibration sliders move the output and persist after a restart', async ({ page }) => {
  await openUi(page, studio);
  await page.locator('summary', { hasText: 'Calibration' }).click();
  await page.locator('#cOx').fill('30');
  await expect(page.locator('#cOxV')).toHaveText('0.30');
  await page.locator('#cRot').fill('45');
  await expect(page.locator('#cRotV')).toHaveText('45°');
  await expect.poll(calibration).toEqual(expect.objectContaining({ offset_x: expect.closeTo(0.3, 3), rotation_deg: 45 }));
  await expect.poll(async () => centroid((await studio.frame()).points).x).toBeCloseTo(0.3, 1);
  await expect.poll(() => readJson('calibration.json')).toEqual(expect.objectContaining({ offset_x: expect.closeTo(0.3, 3), rotation_deg: 45 }));

  await studio.restart();
  expect(await calibration()).toEqual(expect.objectContaining({ offset_x: expect.closeTo(0.3, 3), rotation_deg: 45 }));
  await openUi(page, studio);
  await page.locator('summary', { hasText: 'Calibration' }).click();
  await expect(page.locator('#cOx')).toHaveValue('30');
  await expect(page.locator('#cOxV')).toHaveText('0.30');
  await expect(page.locator('#cRot')).toHaveValue('45');
  await expect.poll(async () => centroid((await studio.frame()).points).x).toBeCloseTo(0.3, 1);
});

test('the output stays inside -1..1 whatever the sliders say', async ({ page }) => {
  await openUi(page, studio);
  await page.locator('summary', { hasText: 'Calibration' }).click();
  await page.locator('#cSx').fill('200');
  await page.locator('#cSy').fill('200');
  await page.locator('#mSize').fill('200');
  await page.locator('#scale').fill('100');
  await expect.poll(async () => (await studio.live()).size).toBe(2);
  await expect.poll(calibration).toEqual(expect.objectContaining({ scale_x: 2, scale_y: 2 }));
  const pts = (await studio.frame()).points;
  expect(pts.length).toBeGreaterThan(0);
  expect(extent(pts)).toBeLessThanOrEqual(1);
  expect(pts.every(p => Math.abs(p[0]) <= 1 && Math.abs(p[1]) <= 1)).toBe(true);
});

test('scenes and master live modifiers persist after a restart', async ({ page }) => {
  await openUi(page, studio);
  await page.locator('#mSize').fill('120');
  await page.locator('#rotPresets button', { hasText: 'Lent' }).click();
  await page.locator('#sceneName').fill('Gardée');
  await page.locator('#sceneSave').click();
  await expect(page.locator('#sceneList .scene', { hasText: 'Gardée' })).toBeVisible();
  // live.json is written at most once a second.
  await expect.poll(() => readJson('live.json'), { timeout: 5_000 })
    .toEqual(expect.objectContaining({ size: expect.closeTo(1.2, 3), rot_speed: [0, 0, 30] }));

  await studio.restart();
  expect((await studio.state()).scenes.map((s: { name: string }) => s.name)).toEqual(['Gardée']);
  expect(await studio.live()).toEqual(expect.objectContaining({ size: expect.closeTo(1.2, 3), rot_speed: [0, 0, 30] }));
  await openUi(page, studio);
  await expect(page.locator('#sceneList .scene', { hasText: 'Gardée' })).toBeVisible();
  await expect(page.locator('#mSizeV')).toHaveText('120 %');
  await expect(page.locator('#rotPresets button', { hasText: 'Lent' })).toHaveClass(/active/);
  // Still preview-only and disarmed after the restart.
  expect((await studio.state()).armed).toBe(false);
});
