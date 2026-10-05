// LIVE › Direct › Effets (T-142): strobe, visible points, dotted lines,
// mirror, prism, « Figer » and « Noir ». Preview only: nothing is armed.
import { test, expect, useStudio, openUi, reveal, isLit } from '../studio';

const studio = useStudio();
const live = () => studio.live();
const lit = async () => (await studio.frame()).points.filter(isLit).length;

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
  await expect.poll(lit).toBeGreaterThan(0);
});

test('« Noir » held through /api/control darkens the frame without disarming', async () => {
  const before = await studio.frame();
  expect(before.armed).toBe(false);
  expect(await studio.post('/api/control', { id: 'transport.black_hold', value: 1 })).toBe(200);
  await expect.poll(lit, { timeout: 1000 }).toBe(0);
  const dark = await studio.frame();
  expect(dark.points.length).toBeGreaterThan(0);
  expect(dark.armed).toBe(before.armed);
  expect(await studio.post('/api/control', { id: 'transport.black_hold', value: 0 })).toBe(200);
  await expect.poll(lit, { timeout: 1000 }).toBeGreaterThan(0);
  expect((await studio.frame()).armed).toBe(before.armed);
});

test('the « Noir » and « Figer » buttons act only while held', async ({ page }) => {
  await reveal(page, '#fxBlack');
  const black = page.locator('#fxBlack');
  await black.hover();
  await page.mouse.down();
  await expect.poll(async () => (await live()).black).toBe(true);
  await expect(black).toHaveClass(/active/);
  await expect.poll(lit).toBe(0);
  await page.mouse.up();
  await expect.poll(async () => (await live()).black).toBe(false);
  await expect.poll(lit).toBeGreaterThan(0);

  const freeze = page.locator('#fxFreeze');
  await freeze.hover();
  await page.mouse.down();
  await expect.poll(async () => (await live()).freeze).toBe(true);
  await page.mouse.up();
  await expect.poll(async () => (await live()).freeze).toBe(false);
});

type Strobe = { on: boolean; duty: number; rate: { hz?: number; beats?: number } };
const strobe = async () => (await live()).strobe as Strobe;

test('strobe: toggle, synced rate and duty reach the live state', async ({ page }) => {
  await reveal(page, '#fxStrobe');
  // Slower than the strobe limiter's default, so the output really flashes.
  await page.locator('#fxRate').fill('2');
  await expect.poll(async () => (await strobe()).rate.hz).toBe(2);
  await page.locator('#fxStrobe').click();
  await expect.poll(async () => (await strobe()).on).toBe(true);
  await expect(page.locator('#fxStrobe')).toHaveClass(/active/);
  // Some frames dark, some lit: it strobes.
  const seen = new Set<boolean>();
  const until = Date.now() + 3000;
  while (Date.now() < until && seen.size < 2) seen.add((await lit()) > 0);
  expect(seen.size).toBe(2);
  await page.locator('#fxDuty').fill('30');
  await expect.poll(async () => (await strobe()).duty).toBeCloseTo(0.3, 3);
  await page.locator('#fxSync button', { hasText: '1/4' }).click();
  await expect.poll(async () => (await strobe()).rate.beats).toBe(0.25);
  await expect(page.locator('#fxRate')).toBeDisabled();
  await page.locator('#fxSync button', { hasText: 'Hz' }).click();
  await expect.poll(async () => (await strobe()).rate.hz).toBe(8);
  await page.locator('#fxStrobe').click();
  await expect.poll(async () => (await strobe()).on).toBe(false);
});

test('visible points, dotted lines, mirror and prism', async ({ page }) => {
  await reveal(page, '#mTrace');
  const full = await lit();
  await page.locator('#mTrace').fill('50');
  await expect.poll(async () => (await live()).trace).toBeCloseTo(0.5, 3);
  await expect.poll(lit).toBeLessThan(full * 0.75);
  await page.locator('label', { has: page.locator('#mTraceV') }).dblclick(); // T-431: neutral from the name
  await expect.poll(async () => (await live()).trace).toBe(1);

  await page.locator('#mDots').fill('60');
  await expect.poll(async () => (await live()).dots).toBeCloseTo(0.6, 3);
  await expect.poll(lit).toBeLessThan(full);
  await page.locator('label', { has: page.locator('#mDotsV') }).dblclick();

  await reveal(page, '#fxMirror');
  await page.locator('#fxMirror button', { hasText: 'XY' }).click();
  await expect.poll(async () => (await live()).mirror).toBe('xy');
  await expect(page.locator('#fxMirror button', { hasText: 'XY' })).toHaveClass(/active/);
  await page.locator('#fxMirror button', { hasText: 'Aucun' }).click();
  await expect.poll(async () => (await live()).mirror).toBe('none');

  await page.locator('#fxPrism').fill('4');
  await expect.poll(async () => (await live()).prism).toBe(4);
  await expect(page.locator('#fxPrismV')).toHaveText('4 copies');
  // Four copies, each lit run entered from the dark.
  await expect.poll(async () => {
    const pts = (await studio.frame()).points;
    return pts.filter((p, i) => isLit(p) && i > 0 && !isLit(pts[i - 1])).length;
  }).toBeGreaterThanOrEqual(3);
  // Tout réinitialiser brings every effect back to neutral.
  await page.locator('#liveReset').click();
  await expect.poll(async () => (await live()).prism).toBe(1);
});
