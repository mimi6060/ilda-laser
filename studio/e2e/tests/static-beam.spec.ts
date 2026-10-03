// Static beam guard (T-256): a look shrunk to a point by the master size is
// dimmed (profile « Faisceaux ») or blanked (« Stricte ») in the output
// frame, and comes back when the size is restored. The guard can only be
// loosened after an explicit confirmation, and the panel says plainly
// that it is no protection against a scanner failure.
// Preview only: the studio has no output and stays disarmed.
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';

const studio = useStudio();

interface Dwell { active: boolean; profile: string; extent: number; extent_factor: number; max_dose: number; grid: number; cells: [number, number, number][]; points_dimmed: number }
interface DwellFrame { points: Point[]; dwell: Dwell; armed: boolean }
const frame = () => studio.get<DwellFrame>('/api/frame');
const safety = async () => (await studio.get('/api/safety')).settings;
const DEFAULTS = { strobe_max_hz: 4, strobe_burst_s: 5, strobe_cooldown_s: 2, horizon: { y: 0 } };
const resetSafety = () => studio.post('/api/safety', { ...DEFAULTS, confirm_loosen: true });
/** Brightest channel of any point: how much light the frame sends. */
const peak = (f: DwellFrame) => Math.max(0, ...f.points.map(p => Math.max(p[2], p[3], p[4])));
const setSize = (value: number) => studio.post('/api/control', { id: 'master.size', value });

test.beforeEach(async ({ page }) => {
  expect(await resetSafety()).toBe(200);
  await studio.reset();
  await studio.post('/api/settings', { ...(await studio.state()).settings, brightness: 1 });
  await openUi(page, studio);
});

test.afterEach(async () => {
  await setSize(1);
  expect(await resetSafety()).toBe(200);
});

test('a normal look is untouched by the guard', async () => {
  await expect.poll(async () => peak(await frame())).toBeGreaterThan(0.99);
  const f = await frame();
  expect(f.dwell.active).toBe(false);
  expect(f.dwell.profile).toBe('beams');
  expect(f.dwell.extent).toBeGreaterThan(0.5);
});

test('master size 0: dimmed in « Faisceaux », dark in « Stricte », back when restored', async ({ page }) => {
  await reveal(page, '#safetyPanel summary');
  await page.locator('#safetyPanel summary').click();
  await expect(page.locator('#sfDwellProfile')).toHaveValue('beams');
  await expect(page.locator('#sfDwellMinV')).toHaveText('5 %');
  await expect(page.locator('#sfDwellWarn')).toContainText('ne protège pas contre une panne de scanner');
  await expect(page.locator('#dwellLed')).not.toHaveClass(/\bon\b/);
  await expect.poll(async () => peak(await frame())).toBeGreaterThan(0.99);

  // Shrunk to a point (above the horizon): a static beam, capped by the grid.
  expect(await setSize(0)).toBe(200);
  await expect.poll(async () => (await frame()).dwell.active, { timeout: 3000 }).toBe(true);
  await expect.poll(async () => peak(await frame()), { timeout: 3000 }).toBeLessThanOrEqual(0.61);
  let f = await frame();
  expect(f.points.some(isLit)).toBe(true);
  expect(f.dwell.cells.length).toBeGreaterThan(0);
  await expect(page.locator('#dwellLed')).toHaveClass(/\bon\b/);
  await expect(page.locator('#dwellInfo')).toContainText('Garde active');

  // « Stricte » is tighter: applied at once, the point goes dark.
  await page.locator('#sfDwellProfile').selectOption('strict');
  await expect.poll(async () => (await safety()).dwell.profile).toBe('strict');
  await expect.poll(async () => (await frame()).points.filter(isLit).length, { timeout: 3000 }).toBe(0);
  f = await frame();
  expect(f.dwell.extent).toBeLessThan(0.025);
  expect(f.dwell.extent_factor).toBe(0);
  expect(f.armed).toBe(false);

  // Size restored: the look comes back at full brightness.
  expect(await setSize(1)).toBe(200);
  await expect.poll(async () => peak(await frame()), { timeout: 3000 }).toBeGreaterThan(0.99);
  await expect.poll(async () => (await frame()).dwell.active, { timeout: 3000 }).toBe(false);
  await expect(page.locator('#dwellLed')).not.toHaveClass(/\bon\b/);
});

test('loosening the guard asks first; out of range is refused', async ({ page }) => {
  expect(await studio.post('/api/safety', { ...DEFAULTS, dwell: { profile: 'strict', min_extent: 0.1 } })).toBe(200);
  await page.reload();
  await reveal(page, '#safetyPanel summary');
  await page.locator('#safetyPanel summary').click();
  await expect(page.locator('#sfDwellProfile')).toHaveValue('strict');
  await expect(page.locator('#sfDwellMinV')).toHaveText('10 %');

  // Back to « Faisceaux »: the confirmation bar, « Annuler » keeps Strict.
  await page.locator('#sfDwellProfile').selectOption('beams');
  await expect(page.locator('#sfLoosen')).toBeVisible();
  await expect(page.locator('#sfLoosenText')).toContainText('Garde anti-point fixe : Stricte → Faisceaux');
  await page.locator('#sfLoosenCancel').click();
  await expect(page.locator('#sfLoosen')).toBeHidden();
  await expect(page.locator('#sfDwellProfile')).toHaveValue('strict');
  expect((await safety()).dwell.profile).toBe('strict');

  // Over HTTP too: 409 without the confirmation, 400 out of range.
  expect(await studio.post('/api/safety', { ...DEFAULTS, dwell: { profile: 'off', min_extent: 0.1 } })).toBe(409);
  expect(await studio.post('/api/safety', { ...DEFAULTS, dwell: { profile: 'strict', min_extent: 0.001 } })).toBe(400);
  expect((await safety()).dwell.profile).toBe('strict');

  // A smaller minimum size, confirmed in the panel.
  await page.locator('#sfDwellMin').fill('4');
  await expect(page.locator('#sfLoosenText')).toContainText('Taille minimum');
  await page.locator('#sfLoosenOk').click();
  await expect(page.locator('#sfLoosen')).toBeHidden();
  await expect.poll(async () => (await safety()).dwell.min_extent).toBeCloseTo(0.04, 5);
  expect((await safety()).dwell.profile).toBe('strict');
});
