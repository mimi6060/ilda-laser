// Per-output power caps and projector sheet (T-254): the cap is applied to
// everything that goes out (/api/frame), it persists with the machine
// settings, lowering applies at once and raising asks for a confirmation
// (and is refused while armed).
// Preview only: the studio has no output (no --device).
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Limits { max_power: number; max_color: [number, number, number] }
const limits = async (): Promise<Limits> => {
  const j = await studio.get('/api/outputs/limits');
  return j.outputs.find((o: { id: string }) => o.id === j.active).limits;
};
const points = async () => (await studio.frame()).points as Point[];
/** Brightest value of one channel (2 = red, 3 = green, 4 = blue) or of all. */
const peak = (pts: Point[], ch?: number) => Math.max(0, ...pts.filter(isLit).map(p => ch === undefined ? Math.max(p[2], p[3], p[4]) : p[ch]));

async function openOutputs(page: Page) {
  await reveal(page, '#outputsPanel');
  await expect(page.locator('#pwPowerV')).not.toHaveText('');
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/lfos', []);
  // Back to the defaults (higher than a test's caps, so confirmed).
  expect(await studio.post('/api/outputs/limits', { limits: { max_power: 0.5, max_color: [1, 1, 1] }, confirm_loosen: true })).toBe(200);
  await studio.reset();
  await openUi(page, studio);
  // A white circle at full brightness.
  await studio.post('/api/settings', { ...(await studio.state()).settings, content: { kind: 'shape', shape: 'circle' }, color: [255, 255, 255], brightness: 1 });
});

test('the default cap is 50 % and the panel shows it', async ({ page }) => {
  await expect.poll(async () => peak(await points())).toBeCloseTo(0.5, 2);
  await openOutputs(page);
  await expect(page.locator('#pwPowerV')).toHaveText('50 %');
  await expect(page.locator('#pwGreenV')).toHaveText('100 %');
  await expect(page.locator('#capHint')).toContainText('50 %');
});

test('a 30 % cap with brightness at 100 %: no colour above 0.3 in /api/frame; it persists', async ({ page }) => {
  await openOutputs(page);
  await page.locator('#pwPower').fill('30');
  await expect.poll(async () => (await limits()).max_power).toBeCloseTo(0.3, 5);
  await expect(page.locator('#pwLoosen')).toBeHidden();
  // Master brightness at 100 % too, and an LFO pushing the look brightness.
  expect(await studio.post('/api/control', { id: 'master.brightness', value: 1 })).toBe(200);
  expect(await studio.post('/api/lfos', [{ target: 'look.brightness', wave: 'sine', rate: { hz: 3 }, depth: 1, phase: 0, offset: 1, enabled: true }])).toBe(200);
  await expect.poll(async () => peak(await points())).toBeCloseTo(0.3, 2);
  for (let i = 0; i < 20; i++) {
    const pts = await points();
    expect(pts.filter(isLit).length).toBeGreaterThan(0);
    expect(peak(pts)).toBeLessThanOrEqual(0.3 + 1e-3);
    await page.waitForTimeout(25);
  }
  // Green clipped to 20 %: green ≤ 0.2 × 0.3, red and blue still 0.3.
  await page.locator('#pwGreen').fill('20');
  await expect.poll(async () => peak(await points(), 3)).toBeLessThanOrEqual(0.06 + 1e-3);
  expect(peak(await points(), 2)).toBeCloseTo(0.3, 2);

  // Saved with the machine: still there after a restart.
  await studio.restart();
  await openUi(page, studio);
  expect(await limits()).toEqual({ max_power: expect.closeTo(0.3, 5), max_color: [1, expect.closeTo(0.2, 5), 1] });
  await studio.post('/api/settings', { ...(await studio.state()).settings, content: { kind: 'shape', shape: 'circle' }, color: [255, 255, 255], brightness: 1 });
  await expect.poll(async () => (await points()).filter(isLit).length).toBeGreaterThan(0);
  expect(peak(await points())).toBeLessThanOrEqual(0.3 + 1e-3);
  await openOutputs(page);
  await expect(page.locator('#pwPowerV')).toHaveText('30 %');
  await expect(page.locator('#pwGreenV')).toHaveText('20 %');
  expect((await studio.frame()).armed).toBe(false);
});

test('raising a cap asks for confirmation; cancelling keeps it; never while armed', async ({ page }) => {
  expect(await studio.post('/api/outputs/limits', { limits: { max_power: 0.3, max_color: [1, 1, 1] } })).toBe(200);
  await page.reload();
  await openOutputs(page);
  await expect(page.locator('#pwPowerV')).toHaveText('30 %');
  await page.locator('#pwPower').fill('80');
  await expect(page.locator('#pwLoosen')).toBeVisible();
  await expect(page.locator('#pwLoosenText')).toContainText('30 % → 80 %');
  await page.locator('#pwLoosenCancel').click();
  await expect(page.locator('#pwLoosen')).toBeHidden();
  await expect(page.locator('#pwPowerV')).toHaveText('30 %');
  expect((await limits()).max_power).toBeCloseTo(0.3, 5);
  // Over HTTP too: 409 without the confirmation.
  expect(await studio.post('/api/outputs/limits', { limits: { max_power: 0.8, max_color: [1, 1, 1] } })).toBe(409);

  // Armed (preview only: no output): refused even confirmed, lowering works.
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  try {
    expect(await studio.post('/api/outputs/limits', { limits: { max_power: 0.8, max_color: [1, 1, 1] }, confirm_loosen: true })).toBe(409);
    expect(await studio.post('/api/outputs/limits', { limits: { max_power: 0.2, max_color: [1, 1, 1] } })).toBe(200);
    await expect.poll(async () => peak(await points())).toBeLessThanOrEqual(0.2 + 1e-3);
  } finally {
    await studio.post('/api/arm', { on: false });
  }
  expect((await limits()).max_power).toBeCloseTo(0.2, 5);

  // Disarmed and confirmed: applied.
  await page.reload();
  await openOutputs(page);
  await page.locator('#pwPower').fill('80');
  await expect(page.locator('#pwLoosen')).toBeVisible();
  await page.locator('#pwLoosenOk').click();
  await expect(page.locator('#pwLoosen')).toBeHidden();
  await expect.poll(async () => (await limits()).max_power).toBeCloseTo(0.8, 5);
  await expect.poll(async () => peak(await points())).toBeCloseTo(0.8, 2);
});

test('the projector sheet is saved and does not change the caps', async ({ page }) => {
  await openOutputs(page);
  await page.locator('#pjName').fill('Projecteur scène');
  await page.locator('#pjName').blur();
  await page.locator('#pjClass').selectOption('3B');
  await page.locator('#pjPowG').fill('500');
  await page.locator('#pjPowG').blur();
  await page.locator('#pjScanFail').selectOption('yes');
  await expect.poll(async () => {
    const j = await studio.get('/api/outputs/limits');
    const p = j.outputs[0].projector;
    return [p.name, p.class, p.power_mw[1], p.hw_scan_fail];
  }).toEqual(['Projecteur scène', '3B', 500, true]);
  expect((await limits()).max_power).toBeCloseTo(0.5, 5);
  await page.reload();
  await openOutputs(page);
  await expect(page.locator('#pjName')).toHaveValue('Projecteur scène');
  await expect(page.locator('#pjClass')).toHaveValue('3B');
  await page.screenshot({ path: 'test-results/power-caps.png' });
});
