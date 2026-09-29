// Strobe limiter and beam horizon (T-101), projection zones (T-003): a fast
// strobe is cut to a steady output after 5 s, beams below the horizon never
// reach the frame, nothing is lit inside a Blank zone, and the « Sécurité »
// panel only loosens a setting after an explicit confirmation.
// Preview only: the studio has no output and stays disarmed.
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Strobe { active: boolean; fast: boolean; rate_hz: number; burst_s: number; beams_blanked: number; points_masked: number }
interface SafetyFrame { points: Point[]; strobe: Strobe; armed: boolean }
const frame = () => studio.get<SafetyFrame>('/api/frame');
const safety = async () => (await studio.get('/api/safety')).settings;
const DEFAULTS = { strobe_max_hz: 4, strobe_burst_s: 5, strobe_cooldown_s: 2, horizon: { y: 0 } };
/** Back to the defaults: looser than a test's settings, so confirmed. */
const resetSafety = () => studio.post('/api/safety', { ...DEFAULTS, confirm_loosen: true });

/** An 8 Hz on/off strobe on the whole output: a square LFO on the master
 * brightness (it can only dim, so offset −1 makes it go fully dark). */
const STROBE_8HZ = [{ target: 'master.brightness', wave: 'square', rate: { hz: 8 }, depth: 1, phase: 0, offset: -1, enabled: true }];

test.beforeEach(async ({ page }) => {
  await studio.post('/api/lfos', []);
  expect(await resetSafety()).toBe(200);
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

  // Lowering it is a loosening: refused without the operator's confirmation.
  expect(await studio.post('/api/safety', { ...DEFAULTS, horizon: { y: -1 } })).toBe(409);
  expect((await safety()).horizon.y).toBe(0);
  // Confirmed, lowered to the bottom: the beams come back.
  expect(await studio.post('/api/safety', { ...DEFAULTS, horizon: { y: -1 }, confirm_loosen: true })).toBe(200);
  await expect.poll(async () => (await frame()).points.filter(isLit).filter(p => p[1] < 0).length).toBeGreaterThan(0);
});

test('the Sécurité panel shows the safe defaults and only tightens', async ({ page }) => {
  await reveal(page, '#safetyPanel summary');
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
  await expect.poll(async () => (await safety()).horizon.y).toBeCloseTo(0.3);
  // Back up to 3 Hz is looser than 2 Hz: the panel asks first.
  await page.locator('#sfHz').fill('3');
  await expect(page.locator('#sfLoosen')).toBeVisible();
  await expect(page.locator('#sfLoosenText')).toContainText('Strobe max');
  await page.locator('#sfLoosenCancel').click();
  await expect(page.locator('#sfLoosen')).toBeHidden();
  await expect(page.locator('#sfHzV')).toHaveText('2 Hz');
  expect((await safety()).strobe_max_hz).toBe(2);

  // Looser than the defaults is refused by the server.
  expect(await studio.post('/api/safety', { ...DEFAULTS, strobe_max_hz: 10 })).toBe(400);
  expect(await studio.post('/api/safety', { ...DEFAULTS, strobe_burst_s: 20 })).toBe(400);
  expect((await safety()).strobe_max_hz).toBe(2);

  // Saved: a reload shows the tightened values.
  await page.reload();
  await reveal(page, '#safetyPanel summary');
  await page.locator('#safetyPanel summary').click();
  await expect(page.locator('#sfHzV')).toHaveText('2 Hz');
});

// ---------- projection zones (T-003) ----------

type Zone = { id: number; name: string; kind: 'blank' | 'dim'; level: number; points: [number, number][] };
/** Strictly inside the polygon shrunk by `margin` (the frame is rounded to
 * 1e-3 and inserted edge points sit 1e-3 outside). Rectangles only. */
function insideRect(p: Point, z: Zone, margin = 0.003) {
  const xs = z.points.map(q => q[0]), ys = z.points.map(q => q[1]);
  return p[0] > Math.min(...xs) + margin && p[0] < Math.max(...xs) - margin && p[1] > Math.min(...ys) + margin && p[1] < Math.max(...ys) - margin;
}
/** Lit points inside, and lit segments whose middle is inside. */
function litInside(points: Point[], z: Zone) {
  let bad = points.filter(p => isLit(p) && insideRect(p, z)).length;
  for (let i = 1; i < points.length; i++) {
    const [a, b] = [points[i - 1], points[i]];
    if (!isLit(a) && !isLit(b)) continue;
    for (const t of [0.25, 0.5, 0.75]) {
      const m: Point = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, 1, 1, 1];
      if (insideRect(m, z)) { bad++; break; }
    }
  }
  return bad;
}
const STRIPE = '-0.3 -1 ; 0.3 -1 ; 0.3 1 ; -0.3 1';

async function openSafety(page: Page) {
  await reveal(page, '#safetyPanel summary');
  if (!(await page.locator('#safetyPanel').evaluate(d => (d as HTMLDetailsElement).open))) await page.locator('#safetyPanel summary').click();
}

/** Adds a zone with the button and redraws it as a vertical stripe through the centre. */
async function addStripe(page: Page) {
  await openSafety(page);
  await page.locator('#sfZoneAdd').click();
  await expect(page.locator('#sfZones .sfzone')).toHaveCount(1);
  await expect.poll(async () => (await safety()).zones.length).toBe(1);
  // Redrawing an existing zone is a loosening (it may uncover something).
  await page.locator('#sfZones .zpts').fill(STRIPE);
  await page.locator('#sfZones .zpts').press('Tab');
  await expect(page.locator('#sfLoosen')).toBeVisible();
  await page.locator('#sfLoosenOk').click();
  await expect(page.locator('#sfLoosen')).toBeHidden();
  await expect.poll(async () => (await safety()).zones[0].points.map((q: number[]) => q.map(v => +v.toFixed(4)))).toEqual([[-0.3, -1], [0.3, -1], [0.3, 1], [-0.3, 1]]);
  return (await safety()).zones[0] as Zone;
}

async function showLook(content: unknown) {
  await studio.post('/api/settings', { ...(await studio.state()).settings, content, scale: 0.8, brightness: 1 });
}

test('a Blank zone: nothing lit inside it in /api/frame, even lines that cross it', async ({ page }) => {
  for (const shape of ['circle', 'line', 'square']) {
    await showLook({ kind: 'shape', shape });
    await expect.poll(async () => (await frame()).points.filter(p => isLit(p) && Math.abs(p[0]) < 0.25).length, { message: shape }).toBeGreaterThan(0);
  }
  const zone = await addStripe(page);
  for (const shape of ['circle', 'line', 'square']) {
    await showLook({ kind: 'shape', shape });
    // Several frames: still lit outside, never inside.
    for (let i = 0; i < 8; i++) {
      const f = await frame();
      expect(f.points.filter(p => isLit(p) && Math.abs(p[0]) > 0.35).length, shape).toBeGreaterThan(0);
      expect(litInside(f.points, zone), shape).toBe(0);
      await page.waitForTimeout(40);
    }
  }
  // Text and a beam fan through the zone too.
  await showLook({ kind: 'text', text: 'LASER' });
  await page.waitForTimeout(100);
  expect(litInside((await frame()).points, zone)).toBe(0);
  await studio.post('/api/control', { id: 'master.reset' });
  expect((await frame()).armed).toBe(false);
});

test('zones persist after a restart and are drawn over the 2D preview', async ({ page }) => {
  const zone = await addStripe(page);
  await showLook({ kind: 'shape', shape: 'circle' });

  // Overlay: the zone is translucent red on the preview, and can be hidden.
  const pixel = () => page.locator('#preview').evaluate(c => {
    const cv = c as HTMLCanvasElement;
    // (0.15, 0.5): inside the stripe, off the centre cross and the circle.
    const d = cv.getContext('2d')!.getImageData(Math.round((0.15 + 1) / 2 * cv.width), Math.round((1 - (0.5 + 1) / 2) * cv.height), 1, 1).data;
    return [d[0], d[1], d[2]];
  });
  await expect.poll(async () => { const [r, g, b] = await pixel(); return r > 40 && r > g + 30 && r > b + 30; }).toBe(true);
  await page.locator('#sfOverlay').uncheck();
  await expect.poll(async () => (await pixel())[0]).toBeLessThan(20);
  await page.locator('#sfOverlay').check();

  await studio.restart();
  await openUi(page, studio);
  const after = await safety();
  expect(after.zones).toEqual([zone]);
  await expect.poll(async () => (await frame()).points.filter(isLit).length).toBeGreaterThan(0);
  expect(litInside((await frame()).points, zone)).toBe(0);
  await openSafety(page);
  await expect(page.locator('#sfZones .zpts')).toHaveValue(STRIPE);
  await expect.poll(async () => { const [r, g] = await pixel(); return r > g + 30; }).toBe(true);
  await page.screenshot({ path: 'test-results/safety-zones.png' });
  expect((await frame()).armed).toBe(false);
});

test('removing a zone asks for confirmation; cancelling keeps it', async ({ page }) => {
  await addStripe(page);
  await page.locator('#sfZones .zdel').click();
  await expect(page.locator('#sfLoosen')).toBeVisible();
  await expect(page.locator('#sfLoosenText')).toContainText('supprimée');
  await page.locator('#sfLoosenCancel').click();
  expect((await safety()).zones.length).toBe(1);
  await expect(page.locator('#sfZones .sfzone')).toHaveCount(1);
  // Over HTTP too: a body without the zone is refused without confirmation.
  expect(await studio.post('/api/safety', DEFAULTS)).toBe(409);
  await page.locator('#sfZones .zdel').click();
  await page.locator('#sfLoosenOk').click();
  await expect(page.locator('#sfZones .sfzone')).toHaveCount(0);
  expect((await safety()).zones).toEqual([]);
});

test('the horizon also covers lines once ticked', async ({ page }) => {
  await showLook({ kind: 'shape', shape: 'circle' });
  await expect.poll(async () => (await frame()).points.filter(p => isLit(p) && p[1] < -0.1).length).toBeGreaterThan(0);
  await openSafety(page);
  await page.locator('#sfLines').check();
  await expect.poll(async () => (await safety()).horizon.lines).toBe(true);
  for (let i = 0; i < 5; i++) {
    const f = await frame();
    expect(f.points.filter(p => isLit(p) && p[1] < -0.002)).toEqual([]);
    expect(f.points.filter(p => isLit(p) && p[1] > 0.2).length).toBeGreaterThan(0);
    expect(f.strobe.points_masked).toBeGreaterThan(0);
    await page.waitForTimeout(40);
  }
  // Unticking loosens: confirmation first.
  await page.locator('#sfLines').uncheck();
  await expect(page.locator('#sfLoosen')).toBeVisible();
  expect((await safety()).horizon.lines).toBe(true);
  await page.locator('#sfLoosenOk').click();
  await expect.poll(async () => (await safety()).horizon.lines).toBe(false);
});
