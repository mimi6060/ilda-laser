// Strobe limiter and beam horizon (T-101): a fast strobe is cut to a
// steady output after 5 s, beams below the horizon never reach the frame,
// and the « Sécurité » panel can only tighten the limits.
// Preview only: the studio has no output and stays disarmed.
import { test, expect, useStudio, openUi, isLit, type Point } from '../studio';

const studio = useStudio();

interface Strobe { active: boolean; fast: boolean; rate_hz: number; burst_s: number; beams_blanked: number }
interface SafetyFrame { points: Point[]; strobe: Strobe; armed: boolean }
const frame = () => studio.get<SafetyFrame>('/api/frame');
const safety = async () => (await studio.get('/api/safety')).settings as Record<string, number>;
const DEFAULTS = { strobe_max_hz: 4, strobe_burst_s: 5, strobe_cooldown_s: 2, beam_floor_y: 0 };

/** An 8 Hz on/off strobe on the whole output: a square LFO on the master
 * brightness (it can only dim, so offset −1 makes it go fully dark). */
const STROBE_8HZ = [{ target: 'master.brightness', wave: 'square', rate: { hz: 8 }, depth: 1, phase: 0, offset: -1, enabled: true }];

test.beforeEach(async ({ page }) => {
  await studio.post('/api/lfos', []);
  await studio.post('/api/safety', DEFAULTS);
  await studio.reset();
  await openUi(page, studio);
});

test('an 8 Hz strobe is held steady after 5 s, and released once it stops', async ({ page }) => {
  test.setTimeout(40_000);
  await studio.post('/api/settings', { ...(await studio.state()).settings, brightness: 1 });
  expect(await studio.post('/api/lfos', STROBE_8HZ)).toBe(200);
  const started = Date.now();

  // It flashes freely at first: both lit and dark frames, measured fast.
  const seen = new Set<boolean>();
  while (Date.now() - started < 2000) {
    seen.add((await frame()).points.some(isLit));
    await page.waitForTimeout(30);
  }
  expect(seen).toEqual(new Set([true, false]));
  expect((await frame()).strobe.fast).toBe(true);
  await expect(page.locator('#strobeLed')).not.toHaveClass(/\bon\b/);

  // Cut after 5 s (poll until the limiter holds; well before 7 s).
  await expect.poll(async () => (await frame()).strobe.active, { timeout: 7000, intervals: [100] }).toBe(true);
  // Not much before 5 s (the unit tests pin 5 s ± 0.1 s; here the look
  // change just before can add a flicker or two to the burst).
  expect(Date.now() - started).toBeGreaterThan(4500);
  await expect(page.locator('#strobeLed')).toHaveClass(/\bon\b/);

  // Steady: every frame lit for the next 1.5 s, although the LFO still strobes.
  const t0 = Date.now();
  while (Date.now() - t0 < 1500) {
    const f = await frame();
    expect(f.points.some(isLit)).toBe(true);
    expect(f.strobe.active).toBe(true);
    await page.waitForTimeout(30);
  }
  expect((await frame()).armed).toBe(false);

  // Stop the strobe: released after the 2 s cool-down.
  await studio.post('/api/lfos', []);
  await expect.poll(async () => (await frame()).strobe.active, { timeout: 5000, intervals: [100] }).toBe(false);
  await expect(page.locator('#strobeLed')).not.toHaveClass(/\bon\b/);
});

test('a 3 Hz strobe is never limited', async ({ page }) => {
  test.setTimeout(20_000);
  expect(await studio.post('/api/lfos', [{ ...STROBE_8HZ[0], rate: { hz: 3 } }])).toBe(200);
  const t0 = Date.now();
  while (Date.now() - t0 < 7000) {
    const f = await frame();
    expect(f.strobe.active).toBe(false);
    expect(f.strobe.fast).toBe(false);
    await page.waitForTimeout(50);
  }
});

test('no beam below the horizon in the frame; lowering it is an explicit setting', async () => {
  const beams = { kind: 'generator', generator: 'beam_fan', params: { count: 6, b: -0.5 } };
  await studio.post('/api/settings', { ...(await studio.state()).settings, content: beams, scale: 0.8, brightness: 1 });
  await expect.poll(async () => (await frame()).strobe.beams_blanked).toBe(6);
  const f = await frame();
  expect(f.points.filter(isLit).filter(p => p[1] < 0)).toEqual([]);

  // Lowered to the bottom: the beams come back.
  expect(await studio.post('/api/safety', { ...DEFAULTS, beam_floor_y: -1 })).toBe(200);
  await expect.poll(async () => (await frame()).points.filter(isLit).filter(p => p[1] < 0).length).toBeGreaterThan(0);
});

test('the Sécurité panel shows the safe defaults and only tightens', async ({ page }) => {
  await page.locator('#safetyPanel summary').click();
  await expect(page.locator('#sfHzV')).toHaveText('4 Hz');
  await expect(page.locator('#sfBurstV')).toHaveText('5 s');
  await expect(page.locator('#sfFloorV')).toHaveText('0.00');
  // The sliders stop at the safe limits.
  await expect(page.locator('#sfHz')).toHaveAttribute('max', '4');
  await expect(page.locator('#sfBurst')).toHaveAttribute('max', '5');

  await page.locator('#sfHz').fill('2');
  await expect.poll(async () => (await safety()).strobe_max_hz).toBe(2);
  await page.locator('#sfFloor').fill('0.3');
  await expect.poll(async () => (await safety()).beam_floor_y).toBeCloseTo(0.3);

  // Looser than the defaults is refused by the server.
  expect(await studio.post('/api/safety', { ...DEFAULTS, strobe_max_hz: 10 })).toBe(400);
  expect(await studio.post('/api/safety', { ...DEFAULTS, strobe_burst_s: 20 })).toBe(400);
  expect((await safety()).strobe_max_hz).toBe(2);

  // Saved: a reload shows the tightened values.
  await page.reload();
  await page.locator('#safetyPanel summary').click();
  await expect(page.locator('#sfHzV')).toHaveText('2 Hz');
});
