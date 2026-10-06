// Outdoor mode (T-262): declared sectors and the 45° limit cut the frame
// that goes out, « Ciel coupé » darkens everything above the horizon from
// the blue button or Alt+C without disarming, arming waits for the DGTA
// reference, and turning the mode off needs the operator's confirmation.
// Preview only: the fake output, never a laser.
import { test, expect, useStudio, openUi, openWorkspace, focusPage, isLit } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
const frame = () => studio.frame();
const lit = async () => (await frame()).points.filter(isLit);

async function openOutdoor(page: Page) {
  await openWorkspace(page, 'settings');
  if ((await page.locator('#outdoorPanel').getAttribute('open')) === null) await page.locator('#outdoorPanel > summary').click();
  await expect(page.locator('#odEnabled')).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
  // A square over most of the field: lines above and below the horizon.
  await studio.post('/api/settings', { ...(await studio.state()).settings, content: { kind: 'shape', shape: 'square' }, scale: 0.8, brightness: 1 });
  await expect.poll(async () => (await lit()).filter(p => p[1] > 0.5).length).toBeGreaterThan(0);
});

test('declared sectors and the vertical limit cut the frame; no reference, no arming', async ({ page }) => {
  await openOutdoor(page);
  await expect(page.locator('#odBanner')).toContainText('Aide logicielle seulement');
  await expect(page.locator('#odBanner')).toContainText("l'arrêt matériel");
  // On: tighter, applied at once. No sector declared: everything is cut.
  await page.locator('#odEnabled').check();
  await expect.poll(async () => (await studio.get('/api/safety')).settings.outdoor.enabled).toBe(true);
  await expect.poll(async () => (await lit()).length).toBe(0);
  await expect(page.locator('#skyBtn')).toBeVisible();
  // No DGTA reference: arming refused, with the reason.
  await expect(page.locator('#odRefWarn')).toBeVisible();
  expect(await studio.post('/api/arm', { on: true })).toBe(409);
  // A sector over the projector's own field (40°, axis north, level).
  // It lets light out where nothing was: a loosening, confirmed.
  await page.locator('#odSectorAdd').click();
  await expect(page.locator('#odLoosenText')).toContainText('Secteur 1 ajouté ou élargi');
  expect((await studio.get('/api/safety')).settings.outdoor.sectors).toEqual([]);
  await page.locator('#odLoosenOk').click();
  await expect(page.locator('.odsector')).toHaveCount(1);
  await expect.poll(async () => (await studio.get('/api/safety')).settings.outdoor.sectors).toEqual([{ az_min: 340, az_max: 20, el_min: -20, el_max: 20 }]);
  // The ground part is back; above the horizon, 0..20° is more than 45°
  // from the vertical: cut.
  await expect.poll(async () => (await lit()).length).toBeGreaterThan(0);
  for (let i = 0; i < 6; i++) {
    const pts = await lit();
    expect(pts.length).toBeGreaterThan(0);
    expect(pts.filter(p => p[1] > 0.001).length).toBe(0);
    await page.waitForTimeout(40);
  }
  // The reference: arming allowed (preview output), then off again.
  await page.locator('#odRef').fill('BCAA-2026-0042');
  await page.locator('#odRef').press('Tab');
  await expect.poll(async () => (await studio.get('/api/safety')).settings.outdoor.authorization_ref).toBe('BCAA-2026-0042');
  await expect(page.locator('#odRefWarn')).toBeHidden();
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
  // The export is what was entered.
  const res = await page.request.get(`${studio.url}/api/safety/outdoor/export`);
  expect((await res.text()).split('\n')[1]).toBe('1;340;20;-20;20');
  // Off: a loosening, only once confirmed.
  await page.locator('#odEnabled').uncheck();
  await expect(page.locator('#odLoosen')).toBeVisible();
  await expect(page.locator('#odLoosenText')).toContainText('Mode extérieur désactivé');
  expect((await studio.get('/api/safety')).settings.outdoor.enabled).toBe(true);
  await page.locator('#odLoosenOk').click();
  await expect.poll(async () => (await studio.get('/api/safety')).settings.outdoor.enabled).toBe(false);
  await expect(page.locator('#skyBtn')).toBeHidden();
  await expect.poll(async () => (await lit()).filter(p => p[1] > 0.5).length).toBeGreaterThan(0);
});

test('« Ciel coupé » darkens the sky at once, keeps the ground show, and comes back from the keyboard', async ({ page }) => {
  // From a controller: cut, never restored from there.
  expect(await studio.post('/api/control', { id: 'safety.sky_off' })).toBe(200);
  await expect(page.locator('#skyBtn')).toBeVisible();
  await expect(page.locator('#skyBtn')).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(async () => (await lit()).filter(p => p[1] > 0).length).toBe(0);
  expect((await lit()).filter(p => p[1] < -0.5).length).toBeGreaterThan(0);
  expect(await studio.post('/api/safety/sky', { on: false, source: 'midi' })).toBe(403);
  expect((await studio.state()).outdoor.sky_off).toBe(true);
  expect((await studio.state()).armed).toBe(false);
  // Alt+C: the operator's explicit gesture brings the sky back.
  await focusPage(page);
  await page.keyboard.press('Alt+KeyC');
  await expect.poll(async () => (await studio.state()).outdoor.sky_off).toBe(false);
  await expect(page.locator('#skyBtn')).toBeHidden();
  await expect.poll(async () => (await lit()).filter(p => p[1] > 0.5).length).toBeGreaterThan(0);
  // With the mode on (everything declared), the blue button cuts and restores.
  const { settings } = await studio.get('/api/safety');
  settings.outdoor = { ...settings.outdoor, enabled: true, authorization_ref: 'X', sectors: [{ az_min: 0, az_max: 360, el_min: -90, el_max: 90 }], vertical_limit_waived: true, waiver_ref: 'Y' };
  expect(await studio.post('/api/safety', settings)).toBe(200);
  await expect.poll(async () => (await lit()).filter(p => p[1] > 0.5).length).toBeGreaterThan(0);
  await page.locator('#skyBtn').click();
  await expect(page.locator('#skyBtn')).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(async () => (await lit()).filter(p => p[1] > 0).length).toBe(0);
  await page.locator('#skyBtn').click();
  await expect.poll(async () => (await studio.state()).outdoor.sky_off).toBe(false);
});

test('the top bar still fits at 1280 px with « CIEL COUPÉ » shown', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  expect(await studio.post('/api/safety/sky', { on: true, source: 'ui' })).toBe(200);
  await expect(page.locator('#skyBtn')).toBeVisible();
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  const arm = await page.locator('#armBtn').boundingBox();
  expect(arm && arm.x + arm.width).toBeLessThanOrEqual(1280);
  const sky = await page.locator('#skyBtn').boundingBox();
  expect(sky && sky.height).toBeGreaterThanOrEqual(44);
});
