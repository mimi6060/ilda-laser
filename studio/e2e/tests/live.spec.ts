// « Direct » panel: master live modifiers on top of any look or cue.
import { test, expect, useStudio, openUi, focusPage, extent, centroid, isLit } from '../studio';

const studio = useStudio();
const live = () => studio.live();
const values = async () => (await studio.controlValues()).values;
const points = async () => (await studio.frame()).points;

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('rotation presets set the Z rotation speed and highlight the button', async ({ page }) => {
  const presets = page.locator('#rotPresets button');
  await expect(presets.filter({ hasText: 'Stop' })).toHaveClass(/active/);

  await presets.filter({ hasText: 'Moyen' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(90);
  await expect(presets.filter({ hasText: 'Moyen' })).toHaveClass(/active/);
  await expect(presets.filter({ hasText: 'Stop' })).not.toHaveClass(/active/);

  await presets.filter({ hasText: 'Rapide' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(270);
  await expect(presets.filter({ hasText: 'Rapide' })).toHaveClass(/active/);

  await presets.filter({ hasText: 'Stop' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(0);
});

test('a rotation preset spins the drawing', async ({ page }) => {
  await page.getByRole('button', { name: 'Carré', exact: true }).click();
  await expect.poll(async () => (await studio.state()).settings.content.shape).toBe('square');
  await page.locator('#rotPresets button', { hasText: 'Rapide' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(270);
  const a = (await points()).find(isLit)!;
  await expect.poll(async () => {
    const b = (await points()).find(isLit)!;
    return Math.hypot(a[0] - b[0], a[1] - b[1]);
  }).toBeGreaterThan(0.01);
});

test('Synchro tempo keeps the preset step, in turns per bar', async ({ page }) => {
  await page.locator('#rotPresets button', { hasText: 'Moyen' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(90);
  await page.locator('#rotSync').check();
  await expect.poll(async () => (await live()).rot_sync).toBe(true);
  expect((await live()).rot_speed[2]).toBe(1); // Moyen = 1 turn per bar
  await expect(page.locator('#rotPresets button', { hasText: 'Moyen' })).toHaveClass(/active/);
  await page.locator('#rotSync').uncheck();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(90);
});

test('master size slider scales the output, on /api/live and /api/control-values', async ({ page }) => {
  await expect(page.locator('#mSizeV')).toHaveText('100 %');
  await expect.poll(async () => extent(await points())).toBeCloseTo(0.5, 1);
  await page.locator('#mSize').fill('150');
  await expect(page.locator('#mSizeV')).toHaveText('150 %');
  await expect.poll(async () => (await live()).size).toBeCloseTo(1.5, 3);
  expect((await values())['master.size']).toBeCloseTo(1.5, 3);
  await expect.poll(async () => extent(await points())).toBeCloseTo(0.75, 1);

  await page.locator('#mSize').fill('50');
  await expect.poll(async () => extent(await points())).toBeCloseTo(0.25, 1);
});

test('position X moves the drawing', async ({ page }) => {
  await page.locator('#mPosX').fill('40');
  await expect(page.locator('#mPosXV')).toHaveText('40 %');
  await expect.poll(async () => (await live()).pos_x).toBeCloseTo(0.4, 3);
  await expect.poll(async () => centroid(await points()).x).toBeCloseTo(0.4, 1);
});

test('master brightness dims every point', async ({ page }) => {
  const before = Math.max(...(await points()).map(p => p[3]));
  await page.locator('#mBright').fill('50');
  await expect.poll(async () => (await live()).brightness).toBeCloseTo(0.5, 3);
  await expect.poll(async () => Math.max(...(await points()).map(p => p[3]))).toBeCloseTo(before * 0.5, 2);
});

test('Réinitialiser brings every master modifier back to neutral', async ({ page }) => {
  await page.locator('#mSize').fill('150');
  await page.locator('#rotPresets button', { hasText: 'Lent' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(30);
  await expect.poll(async () => (await live()).size).toBeCloseTo(1.5, 3);
  await page.locator('#liveReset').click();
  await expect.poll(async () => (await live()).size).toBe(1);
  expect((await live()).rot_speed[2]).toBe(0);
  await expect(page.locator('#mSizeV')).toHaveText('100 %');
  await expect(page.locator('#rotPresets button', { hasText: 'Stop' })).toHaveClass(/active/);
});

test('Inverser reverses the rotation only while held (button and < key)', async ({ page }) => {
  const btn = page.locator('#rotReverse');
  await btn.hover();
  await page.mouse.down();
  await expect.poll(async () => (await live()).rot_reverse).toBe(true);
  await page.mouse.up();
  await expect.poll(async () => (await live()).rot_reverse).toBe(false);

  await focusPage(page);
  await page.keyboard.down('<');
  await expect.poll(async () => (await live()).rot_reverse).toBe(true);
  await page.keyboard.up('<');
  await expect.poll(async () => (await live()).rot_reverse).toBe(false);
});

test('a live change made elsewhere (MIDI/API) moves the slider', async ({ page }) => {
  expect(await studio.post('/api/control', { id: 'master.size', value: 0.8 })).toBe(200);
  await expect(page.locator('#mSizeV')).toHaveText('80 %');
  await expect(page.locator('#mSize')).toHaveValue('80');
});
