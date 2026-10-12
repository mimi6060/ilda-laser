// Target tracking over PosiStageNet (T-457): RÉGLAGES › Suivi de cible.
// Tracker positions come from POST /api/test/psn (--test-hooks): packets
// built by our own encoder, handed to the receiver as its socket would,
// nothing on the network (the receiver itself listens on 127.0.0.1 with a
// port the system picks). Switching on, assigning a tracker and
// calibrating it wait for the confirmation; the « Cibles miroir » look
// then follows the tracker; no data for 500 ms, or a position in a Blank
// zone, and the beam is dark. Preview only: never armed, never a laser.
import { test, expect, useStudio, openUi, openWorkspace, isLit, centroid } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio({ testHooks: true });
const safety = async () => (await studio.get('/api/safety')).settings;
const lit = async () => (await studio.frame()).points.filter(isLit);

/** Sends tracker 5 (« Chariot ») at `pos` every 100 ms until `stop()`. */
function feeder() {
  let pos: number[] | null = null;
  const timer = setInterval(() => {
    if (pos) studio.post('/api/test/psn', { trackers: [{ id: 5, pos, name: 'Chariot' }] }).catch(() => {});
  }, 100);
  return {
    at(p: number[]) { pos = p; },
    pause() { pos = null; },
    stop() { clearInterval(timer); },
  };
}

async function openPsn(page: Page) {
  await openWorkspace(page, 'settings');
  if ((await page.locator('#psnPanel').getAttribute('open')) === null) await page.locator('#psnPanel > summary').click();
  await expect(page.locator('#psnEnabled')).toBeVisible();
}

/** Clicks the 2D preview at look coordinates (identity calibration). */
async function aim(page: Page, x: number, y: number) {
  const box = (await page.locator('#preview').boundingBox())!;
  await page.locator('#preview').click({ position: { x: box.width * (x + 1) / 2, y: box.height * (1 - y) / 2 } });
}

test.beforeEach(async () => {
  await studio.reset();
  expect(await studio.post('/api/psn', { enabled: false, port: 0, links: [] })).toBe(200);
  expect(await studio.post('/api/targets', { items: [] })).toBe(200);
  const s = await safety();
  expect(await studio.post('/api/safety', { ...s, zones: [], confirm_loosen: true })).toBe(200);
  expect(await studio.post('/api/external/gate', { on: true })).toBe(200);
});

test.afterEach(async () => {
  await studio.post('/api/psn', { enabled: false, port: 0, links: [] });
  await studio.post('/api/external/gate', { on: false });
});

test('assign and calibrate a tracker; the beam follows it, goes dark when the data stops or in a Blank zone', async ({ page }) => {
  const feed = feeder();
  try {
    await openUi(page, studio);
    await openPsn(page);

    // Switching on waits for the confirmation; then it listens on this Mac only.
    await page.locator('#psnEnabled').check();
    await expect(page.locator('#psnLoosenText')).toContainText('Suivi activé');
    expect((await studio.get('/api/psn')).settings.enabled).toBe(false);
    await page.locator('#psnLoosenOk').click();
    await expect.poll(async () => (await studio.get('/api/psn')).settings.enabled).toBe(true);
    await expect(page.locator('#psnListen')).toContainText('127.0.0.1');

    // The tracker shows up; assigning it creates the mobile target « Chariot ».
    feed.at([2, 1, 0]);
    const row = page.locator('#psnTrackerRows tr', { hasText: 'Chariot' });
    await expect(row).toContainText('Non affecté');
    await page.getByRole('button', { name: 'Affecter le traceur 5 à une cible' }).click();
    await expect(page.locator('#psnLoosenText')).toContainText('« Chariot » ajoutée');
    await page.locator('#psnLoosenOk').click();
    await expect(page.locator('#psnName0')).toHaveValue('Chariot');
    await expect(page.locator('.psnstate')).toContainText('Non calibré');

    // Calibrate: four corners of a 4 × 2 m area, each one aimed in the preview.
    await page.getByRole('button', { name: 'Calibrer la cible mobile 1' }).click();
    await expect(page.locator('#psnCalText')).toContainText('point 1');
    const corners: [number[], [number, number]][] = [
      [[0, 0, 0], [-0.5, 0.2]], [[4, 0, 0], [0.5, 0.2]], [[4, 2, 0], [0.5, 0.8]], [[0, 2, 0], [-0.5, 0.8]],
    ];
    for (const [k, [p, [x, y]]] of corners.entries()) {
      feed.at(p);
      await expect(row).toContainText(`${p[0].toFixed(2)} ; ${p[1].toFixed(2)} ; ${p[2].toFixed(2)} m`);
      await aim(page, x, y);
      await expect(page.locator('#psnCalText')).toContainText(`point ${k + 2}`);
    }
    await page.locator('#psnCalDone').click();
    await expect(page.locator('#psnLoosenText')).toContainText('calibrage changé');
    await page.locator('#psnLoosenOk').click();
    await expect.poll(async () => (await studio.get('/api/psn')).calibration[0].ok).toBe(true);
    await expect(page.locator('.psnstate')).toContainText('Erreur moyenne');

    // The « Cibles miroir » look on the mobile target (bit 0: no fixed target).
    const settings = (await studio.state()).settings;
    expect(await studio.post('/api/settings', { ...settings, content: { kind: 'generator', generator: 'mirror_targets', params: { a: 1, b: 0 } } })).toBe(200);
    feed.at([2, 1, 0]);
    await expect.poll(async () => { const c = centroid(await lit()); return [Math.round(c.x * 20) / 20, Math.round(c.y * 20) / 20]; }).toEqual([0, 0.5]);
    feed.at([3, 1.5, 0]);
    await expect.poll(async () => { const c = centroid(await lit()); return [Math.round(c.x * 20) / 20, Math.round(c.y * 20) / 20]; }).toEqual([0.25, 0.65]);
    await expect(page.locator('.psnstate')).toContainText('Suivi');

    // The data stops: dark after the loss delay, and back when it resumes.
    feed.pause();
    await expect.poll(async () => (await lit()).length, { timeout: 3000 }).toBe(0);
    await expect(page.locator('.psnstate')).toContainText('Perdu');
    feed.at([3, 1.5, 0]);
    await expect.poll(async () => (await lit()).length).toBeGreaterThan(0);

    // A Blank zone over the target: never lit, shown « Hors zone ».
    const s = await safety();
    s.zones = [{ id: 1, name: 'Décor', kind: 'blank', level: 0, points: [[0, 0.4], [0.5, 0.4], [0.5, 0.9], [0, 0.9]] }];
    expect(await studio.post('/api/safety', s)).toBe(200);
    await expect.poll(async () => (await lit()).length).toBe(0);
    await expect(page.locator('.psnstate')).toContainText('Hors zone');
    expect((await studio.state()).armed).toBe(false);
  } finally {
    feed.stop();
  }
});

test('switching on is refused armed; switching off always runs; tracking never arms', async ({ page }) => {
  await openUi(page, studio);
  await openPsn(page);
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await page.locator('#psnEnabled').check();
  await expect(page.locator('#psnError')).toContainText('Désarmez le laser');
  expect((await studio.get('/api/psn')).settings.enabled).toBe(false);
  expect(await studio.post('/api/arm', { on: false })).toBe(200);

  expect(await studio.post('/api/psn', { enabled: true, port: 0, confirm_loosen: true })).toBe(200);
  expect(await studio.post('/api/test/psn', { trackers: [{ id: 1, pos: [0, 0, 0] }] })).toBe(200);
  expect((await studio.state()).armed).toBe(false);
  await page.locator('#psnPanel > summary').click();
  await page.locator('#psnPanel > summary').click();
  await expect(page.locator('#psnEnabled')).toBeChecked();
  await page.locator('#psnEnabled').uncheck();
  await expect.poll(async () => (await studio.get('/api/psn')).settings.enabled).toBe(false);
  await expect(page.locator('#psnLoosen')).toBeHidden();
});
