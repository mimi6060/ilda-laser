// Colour rendering of the output (T-414): white balance, a curve per
// colour and the colour delay, set in RÉGLAGES › Sorties et projecteurs in
// front of the test patterns, applied live before the safety stage and the
// caps. Darker applies at once, brighter asks for a confirmation.
// Preview only: the studio has no output (no --device) and stays disarmed.
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

const IDENTITY = {
  white_balance: [1, 1, 1],
  curve: [{ type: 'gamma', gamma: 1 }, { type: 'gamma', gamma: 1 }, { type: 'gamma', gamma: 1 }],
  color_delay_points: 0,
};
const points = async () => (await studio.frame()).points as Point[];
const colour = async () => (await studio.get('/api/outputs/color')).color;

/** Two identical frames in a row (a pattern is static once shown). */
async function steady(): Promise<Point[]> {
  let out: Point[] = [];
  await expect.poll(async () => {
    const [a, b] = [await points(), await points()];
    out = b;
    return a.some(isLit) && JSON.stringify(a) === JSON.stringify(b);
  }).toBe(true);
  return out;
}

async function openColours(page: Page) {
  await reveal(page, '#ocBox');
  await expect(page.locator('#ocWbRV')).toHaveText('100 %');
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  expect(await studio.post('/api/outputs/color', { color: IDENTITY, confirm_loosen: true })).toBe(200);
  // Caps wide open, so /api/frame shows the colour rendering itself.
  expect(await studio.post('/api/outputs/limits', { limits: { max_power: 1, max_color: [1, 1, 1] }, confirm_loosen: true })).toBe(200);
  expect(await studio.post('/api/testpattern', { pattern: 'white_ramp', brightness: 1 })).toBe(200);
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/testpattern', { pattern: null, brightness: 0.2 });
  await studio.post('/api/outputs/color', { color: IDENTITY, confirm_loosen: true });
  await studio.post('/api/outputs/limits', { limits: { max_power: 0.5, max_color: [1, 1, 1] } });
});

test('green gamma 2.2 on the white ramp: middle steps darker, the ends unchanged', async ({ page }) => {
  const before = await steady();
  const lit = before.filter(isLit);
  expect(lit.every(p => Math.abs(p[2] - p[3]) < 1e-4)).toBe(true); // neutral grey
  await openColours(page);
  await page.locator('#ocGamma1').fill('2.2');
  await expect.poll(async () => (await colour()).curve[1]).toEqual({ type: 'gamma', gamma: expect.closeTo(2.2, 3) });
  await expect(page.locator('#ocLoosen')).toBeHidden(); // darker: no confirmation
  await expect.poll(async () => (await points()).filter(isLit).some(p => p[3] < p[2] - 0.05)).toBe(true);
  const after = await steady();
  expect(after.length).toBe(before.length);
  for (const p of after.filter(isLit)) {
    // Red is the input level; green follows level^2.2.
    expect(p[3]).toBeCloseTo(Math.pow(p[2], 2.2), 2);
    expect(p[4]).toBeCloseTo(p[2], 4);
  }
  // The ends: full white stays full, dark stays dark.
  const top = Math.max(...after.map(p => p[2]));
  expect(top).toBeCloseTo(1, 3);
  expect(Math.max(...after.map(p => p[3]))).toBeCloseTo(1, 3);
  after.forEach((p, i) => { if (!isLit(before[i])) expect(isLit(p)).toBe(false); });
  await expect(page.locator('#ocGammaV1')).toHaveText('2.20');
});

test('a colour delay shifts the colour along the strokes and never lights the travel', async ({ page }) => {
  // The shortcut to the « Extinction et retard couleur » pattern.
  await openColours(page);
  await page.locator('#ocTpBlank').click();
  await expect.poll(async () => (await studio.get('/api/testpattern')).state.active).toBe('blanking');
  // Its colour changes along a stroke (the white ramp has none).
  await expect.poll(async () => (await points()).some(p => isLit(p) && Math.abs(p[2] - p[3]) > 0.1)).toBe(true);
  const before = await steady();
  await page.locator('#ocDelay').fill('3');
  await expect.poll(async () => (await colour()).color_delay_points).toBe(3);
  await expect(page.locator('#ocDelayV')).toHaveText('3');
  await expect.poll(async () => JSON.stringify(await points()) !== JSON.stringify(before)).toBe(true);
  const after = await steady();
  expect(after.length).toBe(before.length);
  let shifted = 0;
  for (let i = 0; i < before.length; i++) {
    expect([after[i][0], after[i][1]]).toEqual([before[i][0], before[i][1]]); // positions unchanged
    if (!isLit(before[i])) { expect(isLit(after[i])).toBe(false); continue; }
    // Inside the stroke: the colour of 3 points earlier (held at its start).
    let src = i;
    for (let k = 0; k < 3 && src > 0 && isLit(before[src - 1]); k++) src--;
    expect(after[i][2]).toBeCloseTo(before[src][2], 4);
    if (Math.abs(after[i][2] - before[i][2]) > 1e-4) shifted++;
  }
  expect(shifted).toBeGreaterThan(0);
});

test('raising the white balance asks for confirmation, never while armed; the caps still apply after it', async ({ page }) => {
  expect(await studio.post('/api/outputs/color', { color: { ...IDENTITY, white_balance: [1, 0.5, 1] } })).toBe(200);
  await page.reload();
  await openColours(page);
  await expect(page.locator('#ocWbGV')).toHaveText('50 %');
  await page.locator('#ocWbG').fill('100');
  await expect(page.locator('#ocLoosen')).toBeVisible();
  await expect(page.locator('#ocLoosenText')).toContainText('Ce réglage augmente la lumière émise');
  await expect(page.locator('#ocLoosenText')).toContainText('Balance vert : 50 % → 100 %');
  await page.locator('#ocLoosenCancel').click();
  await expect(page.locator('#ocLoosen')).toBeHidden();
  await expect(page.locator('#ocWbGV')).toHaveText('50 %');
  expect((await colour()).white_balance[1]).toBeCloseTo(0.5, 5);
  await page.locator('#ocWbG').fill('100');
  await page.locator('#ocLoosenOk').click();
  await expect.poll(async () => (await colour()).white_balance[1]).toBeCloseTo(1, 5);
  // Armed (preview only: no output): brighter refused even confirmed, the
  // UI says why; dimming still applies.
  expect(await studio.post('/api/outputs/color', { color: { ...IDENTITY, white_balance: [1, 0.4, 1] } })).toBe(200);
  await page.reload();
  await openColours(page);
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  try {
    expect(await studio.post('/api/outputs/color', { color: IDENTITY, confirm_loosen: true })).toBe(409);
    await page.locator('#ocWbG').fill('90');
    await expect(page.locator('#ocError')).toContainText("Désarmez avant d'augmenter les couleurs");
    await expect(page.locator('#ocLoosen')).toBeHidden();
    await expect(page.locator('#ocWbGV')).toHaveText('40 %');
    expect(await studio.post('/api/outputs/color', { color: { ...IDENTITY, white_balance: [1, 0.3, 1] } })).toBe(200);
  } finally {
    await studio.post('/api/arm', { on: false });
  }
  expect((await colour()).white_balance[1]).toBeCloseTo(0.3, 5);
  expect(await studio.post('/api/outputs/color', { color: IDENTITY, confirm_loosen: true })).toBe(200);
  // A 30 % cap on green comes after the colours: green never above 0.3.
  expect(await studio.post('/api/outputs/limits', { limits: { max_power: 1, max_color: [1, 0.3, 1] } })).toBe(200);
  await expect.poll(async () => Math.max(...(await points()).map(p => p[3]))).toBeLessThanOrEqual(0.3 + 1e-4);
  // Reset: identity again, no confirmation needed from a brighter state.
  await page.locator('#ocReset').click();
  await expect.poll(async () => (await studio.get('/api/outputs/color')).identity).toBe(true);
  expect((await studio.frame()).armed).toBe(false);
});
