// Outdoor mode, one mounting per projector (T-477): RÉGLAGES › Mode
// extérieur lists the venue's other projectors. Without a mounting a
// projector is fully cut; declaring or changing one lets light out, so it
// waits for « Confirmer l'assouplissement »; removing one cuts at once; a
// value that is not a number is refused; show mode locks the settings.
// The 3D view shows each projector's cut (display only). Preview only:
// the fake output, never a laser.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
const VENUE = { preset: 'club', projectors: [{ id: 'main', name: 'Projecteur 1' }, { id: 'p2', name: 'Jardin' }] };
const safety = async () => (await studio.get('/api/safety')).settings;

async function openOutdoor(page: Page) {
  await openWorkspace(page, 'settings');
  if ((await page.locator('#outdoorPanel').getAttribute('open')) === null) await page.locator('#outdoorPanel > summary').click();
  await expect(page.locator('#odEnabled')).toBeVisible();
}

test.beforeEach(async () => {
  await studio.reset();
  expect(await studio.post('/api/venue', VENUE)).toBe(200);
  const s = await safety();
  s.outdoor = { ...s.outdoor, enabled: true, authorization_ref: 'DGTA-E2E', mountings: {},
    sectors: [{ az_min: 0, az_max: 360, el_min: -90, el_max: 30 }] };
  expect(await studio.post('/api/safety', { ...s, confirm_loosen: true })).toBe(200);
});

test('declare, change and remove another projector\'s mounting, with the confirmation', async ({ page }) => {
  await openUi(page, studio);
  await openOutdoor(page);
  const row = page.locator('.odmount[data-pid="p2"]');
  await expect(row).toContainText('Jardin');
  await expect(row).toContainText('Montage non déclaré : entièrement éteint');
  // Undeclared: the whole field of p2 is cut (what the 3D view draws).
  expect((await studio.state()).outdoor.cuts.p2.length).toBeGreaterThan(0);

  // Declaring lets light out: nothing changes before the confirmation.
  await row.getByRole('button', { name: 'Déclarer le montage de Jardin' }).click();
  await expect(page.locator('#odLoosen')).toBeVisible();
  await expect(page.locator('#odLoosenText')).toContainText('« p2 » déclaré');
  expect((await safety()).outdoor.mountings).toEqual({});
  await page.locator('#odLoosenOk').click();
  await expect.poll(async () => Object.keys((await safety()).outdoor.mountings)).toEqual(['p2']);
  await expect(page.locator('#odLoosen')).toBeHidden();
  const tilt = row.getByLabel('Inclinaison (°)');
  await expect(tilt).toBeVisible();

  // Aimed down, within the sector: p2 is no longer cut.
  await tilt.fill('-30');
  await tilt.press('Tab');
  await expect(page.locator('#odLoosenText')).toContainText('« p2 » modifié');
  expect((await safety()).outdoor.mountings.p2.tilt_deg).toBe(0);
  // « Annuler »: nothing sent, the field shows the stored value again.
  await page.locator('#odLoosenCancel').click();
  await expect(tilt).toHaveValue('0');
  await tilt.fill('-30');
  await tilt.press('Tab');
  await page.locator('#odLoosenOk').click();
  await expect.poll(async () => (await safety()).outdoor.mountings.p2.tilt_deg).toBe(-30);
  await expect.poll(async () => (await studio.state()).outdoor.cuts.p2).toEqual([]);

  // Not a number: refused with a message, never guessed.
  await tilt.fill('');
  await tilt.press('Tab');
  await expect(page.locator('#odError')).toContainText('chaque valeur doit être un nombre');
  expect((await safety()).outdoor.mountings.p2.tilt_deg).toBe(-30);
  // Out of range: the server refuses it, the field comes back.
  await tilt.fill('200');
  await tilt.press('Tab');
  await expect(page.locator('#odError')).toContainText('inclinaison');
  await expect(tilt).toHaveValue('-30');

  // Removing only cuts more: applied at once, no confirmation.
  await row.getByRole('button', { name: 'Retirer le montage de Jardin' }).click();
  await expect.poll(async () => (await safety()).outdoor.mountings).toEqual({});
  await expect(page.locator('#odLoosen')).toBeHidden();
  await expect(row).toContainText('Montage non déclaré');
  await expect.poll(async () => (await studio.state()).outdoor.cuts.p2.length).toBeGreaterThan(0);
  expect((await studio.state()).armed).toBe(false);
});

test('show mode locks the mountings', async () => {
  const s = await safety();
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  s.outdoor.mountings = { p2: { scan_angle_deg: 40, tilt_deg: -30, azimuth_deg: 0, height_m: 2 } };
  expect(await studio.post('/api/safety', { ...s, confirm_loosen: true })).toBe(423);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
  expect((await safety()).outdoor.mountings).toEqual({});
});

test('the 3D view shows each projector\'s outdoor cut, display only', async ({ page }) => {
  const posts: string[] = [];
  page.on('request', q => { if (q.method() !== 'GET' && !q.url().endsWith('/api/heartbeat')) posts.push(q.method() + ' ' + q.url()); });
  await openUi(page, studio);
  await page.locator('#viewSeg [data-view="3d"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#r3sfOutdoor')).toBeChecked();
  // The main projector (level, sector up to 30°, 45° limit) has its sky
  // cut; p2 has no mounting: its whole field.
  await expect.poll(() => page.evaluate('view3d.outdoor && view3d.outdoor.length')).toBe(2);
  expect(await page.evaluate('view3d.outdoor.every(c => Array.isArray(c) && c.length > 0)')).toBe(true);
  const size = () => page.evaluate('view3d.overlayTris') as Promise<number>;
  const withCut = await size();
  const before = posts.length;
  const frameBefore = (await studio.get('/api/frame')).points.length;
  await page.locator('#r3sfOutdoor').uncheck();
  expect(await size()).toBeLessThan(withCut);
  await page.locator('#r3sfOutdoor').check();
  expect(await size()).toBe(withCut);
  // Display only: nothing sent, the frame is the studio's.
  expect(posts.slice(before)).toEqual([]);
  expect((await studio.get('/api/frame')).points.length).toBe(frameBefore);
  // The mode off: nothing cut, nothing drawn.
  const s = await safety();
  s.outdoor.enabled = false;
  expect(await studio.post('/api/safety', { ...s, confirm_loosen: true })).toBe(200);
  await expect.poll(() => page.evaluate('view3d.outdoor')).toBeNull();
  expect(await size()).toBeLessThan(withCut);
  // Geometry: the cut adds faces for its projector only, and its layer switch hides it.
  // A string, so the browser does the import (as safety-overlay-3d.spec.ts).
  const r = await page.evaluate(`(async () => {
    const m = await import('/beam3d.js');
    const room = m.DEFAULT_ROOM, p = m.DEFAULT_PROJECTOR;
    const full = [[[-1.01, -1.01], [1.01, -1.01], [1.01, 1.01], [-1.01, 1.01]]];
    const n = o => [o.tris.length / 7, o.lines.length / 7];
    return {
      none: n(m.buildSafetyOverlay({ room, projectors: [p, p] })),
      one: n(m.buildSafetyOverlay({ room, projectors: [p, p], outdoor: [null, full] })),
      two: n(m.buildSafetyOverlay({ room, projectors: [p, p], outdoor: [full, full] })),
      hidden: n(m.buildSafetyOverlay({ room, projectors: [p, p], outdoor: [full, full], show: { outdoor: false } })),
    };
  })()`) as { none: number[]; one: number[]; two: number[]; hidden: number[] };
  expect(r.none).toEqual([0, 0]);
  expect(r.one[0]).toBeGreaterThan(0);
  expect(r.two[0]).toBe(2 * r.one[0]);
  expect(r.hidden).toEqual([0, 0]);
  expect((await studio.state()).armed).toBe(false);
});
