// Tracing the safety zones for verification (T-447): RÉGLAGES › Sécurité ›
// Zones de projection › « Tracer les zones au laser ». The trace replaces
// the look with the outlines just outside the zones, green at a fixed low
// level, never arms, and the look comes back when it ends.
// Preview only: the studio has no output and stays disarmed.
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface TraceState { on: boolean; remaining_s: number; only: number | null }
interface TraceFrame { points: Point[]; armed: boolean; zone_trace: TraceState }
const frame = () => studio.get<TraceFrame>('/api/frame');
const DEFAULTS = { strobe_max_hz: 4, strobe_burst_s: 5, strobe_cooldown_s: 2, horizon: { y: 0 } };
/** A Blank zone in the middle of the field, and a second one top right. */
const STAGE = { name: 'Scène', kind: 'blank', points: [[-0.2, -0.6], [0.2, -0.6], [0.2, -0.2], [-0.2, -0.2]] };
const BOOTH = { name: 'Régie', kind: 'blank', points: [[0.5, 0.5], [0.8, 0.5], [0.8, 0.8], [0.5, 0.8]] };
const inside = (p: Point, z: { points: number[][] }) => {
  const xs = z.points.map(q => q[0]), ys = z.points.map(q => q[1]);
  return p[0] >= Math.min(...xs) && p[0] <= Math.max(...xs) && p[1] >= Math.min(...ys) && p[1] <= Math.max(...ys);
};
const near = (p: Point, z: { points: number[][] }, d: number) => {
  const xs = z.points.map(q => q[0]), ys = z.points.map(q => q[1]);
  return p[0] >= Math.min(...xs) - d && p[0] <= Math.max(...xs) + d && p[1] >= Math.min(...ys) - d && p[1] <= Math.max(...ys) + d;
};

async function openSafety(page: Page) {
  await reveal(page, '#safetyPanel summary');
  if (!(await page.locator('#safetyPanel').evaluate(d => (d as HTMLDetailsElement).open))) await page.locator('#safetyPanel summary').click();
  await expect(page.locator('#ztBtn')).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/safety/trace', { on: false });
  expect(await studio.post('/api/safety', { ...DEFAULTS, zones: [], confirm_loosen: true })).toBe(200);
  await studio.reset();
  // A big bright square: far from the zones' outlines.
  await studio.post('/api/settings', { ...(await studio.state()).settings, content: { kind: 'shape', shape: 'square' }, scale: 0.9, brightness: 1, rotation_speed: 0 });
  expect(await studio.post('/api/safety', { ...DEFAULTS, zones: [STAGE, BOOTH] })).toBe(200);
  await openUi(page, studio);
});

test('the trace draws the outlines just outside the zones, never arms, and the look comes back', async ({ page }) => {
  // The look first: lit points far out (the square's sides).
  await expect.poll(async () => (await frame()).points.filter(p => isLit(p) && Math.abs(p[0]) > 0.7 && Math.abs(p[1]) < 0.3).length).toBeGreaterThan(0);
  await openSafety(page);
  await expect(page.locator('#ztOnly option')).toHaveText(['Toutes les zones', 'Scène', 'Régie']);
  await expect(page.locator('#ztArmHint')).toBeVisible();
  await expect(page.locator('#ztHelp')).toContainText('juste à l\'extérieur');
  await expect(page.locator('#ztHelp')).toContainText('arrêt d\'urgence matériel');
  await page.locator('#ztBtn').click();
  await expect(page.locator('#ztBanner')).toBeVisible();
  await expect(page.locator('#ztBannerText')).toContainText(/Vérification des zones — fin dans \d+ s/);
  await expect(page.locator('#ztBtn')).toHaveText('Arrêter le tracé');
  // The frame is the trace: green only, low, around both zones and along
  // the horizon; nothing lit inside a zone, nothing of the square.
  await expect.poll(async () => (await frame()).points.filter(p => isLit(p) && Math.abs(p[0]) > 0.85 && Math.abs(p[1]) > 0.3).length).toBe(0);
  for (let i = 0; i < 5; i++) {
    const f = await frame();
    const lit = f.points.filter(isLit);
    expect(f.armed).toBe(false);
    expect(f.zone_trace.on).toBe(true);
    expect(lit.every(p => p[2] === 0 && p[4] === 0 && p[3] > 0 && p[3] <= 0.1 + 1e-6)).toBe(true);
    expect(lit.filter(p => inside(p, STAGE) || inside(p, BOOTH)).length).toBe(0);
    expect(lit.filter(p => near(p, STAGE, 0.03)).length).toBeGreaterThan(50);
    expect(lit.filter(p => near(p, BOOTH, 0.03)).length).toBeGreaterThan(50);
    expect(lit.filter(p => Math.abs(p[1] - 0.02) < 1e-3 && Math.abs(p[0]) > 0.9).length).toBeGreaterThan(0);
    await page.waitForTimeout(40);
  }
  expect((await studio.state()).zone_trace.on).toBe(true);
  // Quitter: the look comes back, still disarmed.
  await page.locator('#ztBannerExit').click();
  await expect(page.locator('#ztBanner')).toBeHidden();
  await expect(page.locator('#ztBtn')).toHaveText('Tracer les zones au laser');
  await expect.poll(async () => (await frame()).points.filter(p => isLit(p) && Math.abs(p[0]) > 0.7 && Math.abs(p[1]) < 0.3).length).toBeGreaterThan(0);
  expect((await frame()).armed).toBe(false);
});

test('one zone at a time: only its outline, no horizon', async ({ page }) => {
  await openSafety(page);
  await page.locator('#ztOnly').selectOption({ label: 'Régie' });
  await page.locator('#ztBtn').click();
  await expect(page.locator('#ztBanner')).toBeVisible();
  await expect.poll(async () => {
    const lit = (await frame()).points.filter(isLit);
    return lit.length > 50 && lit.every(p => near(p, BOOTH, 0.03) && !inside(p, BOOTH));
  }).toBe(true);
  expect((await frame()).zone_trace.only).toBe((await studio.get<{ settings: { zones: { id: number; name: string }[] } }>('/api/safety')).settings.zones.find(z => z.name === 'Régie')!.id);
  await page.locator('#ztBtn').click();
  await expect(page.locator('#ztBanner')).toBeHidden();
});

test('Échap ends the trace at once; starting again takes a new click', async ({ page }) => {
  await openSafety(page);
  await page.locator('#ztBtn').click();
  await expect(page.locator('#ztBanner')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('#ztBanner')).toBeHidden();
  expect((await frame()).zone_trace.on).toBe(false);
  // Refused under the e-stop, with the reason shown.
  await page.locator('#ztBtn').click();
  await expect(page.locator('#ztError')).toContainText('arrêt d\'urgence');
  expect((await frame()).zone_trace.on).toBe(false);
  await studio.post('/api/estop/reset', {});
  // The reset alone doesn't bring it back.
  await page.waitForTimeout(200);
  expect((await frame()).zone_trace.on).toBe(false);
  expect((await frame()).armed).toBe(false);
});
