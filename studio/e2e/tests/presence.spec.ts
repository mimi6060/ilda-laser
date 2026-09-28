// Operator presence (T-252): the UI heartbeat and hold-to-run.
// Preview only: "armed" and output_lit come from a studio with no output.
import type { Browser, Page } from '@playwright/test';
import { test, expect, useStudio, openUi, focusPage } from '../studio';

const studio = useStudio();
const arm = async () => (await studio.get('/api/arm')) as { armed: boolean; last_disarm: { reason: string; reason_fr: string } };
const armed = async () => (await arm()).armed;
const DEFAULTS = { ui_timeout_ms: 2000, hold_to_run: false, hold_release_disarm_s: 10, hold_key: 'ShiftRight' };

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/presence', DEFAULTS);
  await studio.post('/api/control', { id: 'cue.stop_all' });
});
test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/presence', DEFAULTS);
});

/** A page in its own browser context, armed with the LASER button. */
async function armedPage(browser: Browser): Promise<Page> {
  const page = await (await browser.newContext()).newPage();
  await openUi(page, studio);
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  return page;
}

/** Hang the page's main thread (a stuck page: no JS event runs any more,
 *  no keyup, no blur). Its heartbeat worker stops once the page no longer
 *  answers it. */
function freeze(page: Page) {
  page.evaluate(() => { for (;;) { /* stuck */ } }).catch(() => { /* page closed */ });
}

test('the page shows the operator as present', async ({ page }) => {
  await openUi(page, studio);
  await expect(page.locator('#presenceLed')).toHaveText('Opérateur présent');
  await expect(page.locator('#presenceLed')).toHaveClass(/\bok\b/);
  await expect(page.locator('#engineLed')).toHaveText('Moteur');
  expect((await studio.state()).presence.ui_alive).toBe(true);
});

test('closing the only page disarms within 2.5 s, reason « Interface perdue »', async ({ browser }) => {
  const page = await armedPage(browser);
  await page.context().close();
  const closedAt = Date.now();
  await expect.poll(armed, { timeout: 2_500, intervals: [100] }).toBe(false);
  expect(Date.now() - closedAt).toBeLessThan(2_500);
  const a = await arm();
  expect(a.last_disarm.reason).toBe('ui_lost');
  expect(a.last_disarm.reason_fr).toBe('Interface perdue');
  // And without a page, nothing can arm it again.
  expect(await studio.post('/api/arm', { on: true, source: 'api' })).toBe(409);
});

test('a frozen page stops beating: disarmed after the timeout', async ({ browser }) => {
  const page = await armedPage(browser);
  freeze(page);
  const frozenAt = Date.now();
  // Still armed right after (the timeout is 2 s)...
  expect(await armed()).toBe(true);
  // ...then disarmed: at most 1.5 s for the worker to notice, plus 2 s.
  await expect.poll(armed, { timeout: 5_000, intervals: [100] }).toBe(false);
  expect(Date.now() - frozenAt).toBeGreaterThan(1_500);
  expect((await arm()).last_disarm.reason).toBe('ui_lost');
  await page.context().close();
});

test('two pages open, one closed: stays armed', async ({ browser }) => {
  const context = await browser.newContext();
  const [one, two] = [await context.newPage(), await context.newPage()];
  await openUi(one, studio);
  await openUi(two, studio);
  await one.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await one.close();
  await two.waitForTimeout(3_000);
  expect(await armed()).toBe(true);
  expect((await studio.state()).presence.clients).toBe(1);
  await context.close();
  await expect.poll(armed, { timeout: 2_500 }).toBe(false);
});

test('a flash held in a page that dies is released', async ({ browser }) => {
  const page = await (await browser.newContext()).newPage();
  await openUi(page, studio);
  await focusPage(page);
  await page.keyboard.down('ShiftLeft');
  await page.keyboard.down('a'); // Shift + cue key = flash, held while the key is down
  await expect.poll(async () => (await studio.frame() as any).cues.active.filter((c: any) => c.held).length).toBe(1);
  freeze(page); // no keyup, no blur: only the server can end it
  await expect.poll(async () => (await studio.frame() as any).cues.active.length, { timeout: 5_000 }).toBe(0);
  await page.context().close();
});

test('hold-to-run: black when released, emits while held, disarms after the release limit', async ({ page }) => {
  await studio.post('/api/presence', { ...DEFAULTS, hold_to_run: true, hold_release_disarm_s: 3 });
  await openUi(page, studio);
  await expect(page.locator('#holdBanner')).toBeVisible();
  await expect(page.locator('#holdBanner')).toHaveText('MAINTENIR « Maj droite » POUR ÉMETTRE');
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  const lit = async () => (await studio.frame() as any).output_lit as number;
  // Armed but not held: nothing lit leaves for the laser (the preview still draws).
  await expect.poll(lit).toBe(0);
  expect((await studio.frame()).points.length).toBeGreaterThan(0);

  await focusPage(page);
  await page.keyboard.down('ShiftRight');
  await expect.poll(lit).toBeGreaterThan(0);
  await expect(page.locator('#holdBanner')).toHaveClass(/emitting/);
  await page.keyboard.up('ShiftRight');
  await expect.poll(lit).toBe(0);
  expect(await armed()).toBe(true); // released: black, not disarmed

  await page.keyboard.down('ShiftRight');
  await expect.poll(lit).toBeGreaterThan(0);
  await page.keyboard.up('ShiftRight');
  const releasedAt = Date.now();
  await expect.poll(armed, { timeout: 6_000, intervals: [200] }).toBe(false);
  expect(Date.now() - releasedAt).toBeGreaterThan(2_500);
  expect((await arm()).last_disarm.reason).toBe('hold_released');
});

test('hold-to-run: Escape with the hold key down still latches the emergency stop', async ({ page }) => {
  await studio.post('/api/presence', { ...DEFAULTS, hold_to_run: true });
  await openUi(page, studio);
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await focusPage(page);
  await page.keyboard.down('ShiftRight');
  await page.keyboard.press('Escape');
  await page.keyboard.up('ShiftRight');
  await expect.poll(async () => (await studio.state()).estop).toBe(true);
  expect(await armed()).toBe(false);
});

test('the safety panel sets the timeout, capped at 10 000 ms, and hold mode', async ({ page }) => {
  await openUi(page, studio);
  await page.locator('#safetyPanel summary').click();
  await expect(page.locator('#uiTimeout')).toHaveValue('2000');
  await page.locator('#uiTimeout').fill('60000');
  await page.locator('#uiTimeout').press('Tab');
  await expect(page.locator('#uiTimeout')).toHaveValue('10000');
  expect((await studio.get('/api/presence')).settings.ui_timeout_ms).toBe(10_000);
  await page.locator('#holdToRun').check();
  await expect.poll(async () => (await studio.get('/api/presence')).settings.hold_to_run).toBe(true);
  await expect(page.locator('#holdBanner')).toBeVisible();
  // Nothing in the panel arms the laser.
  expect(await armed()).toBe(false);
});
