// Engine watchdog (T-253): a stalled engine disarms, reason « Moteur bloqué ».
// Preview only; --test-hooks enables POST /api/test/stall, which makes the
// engine sleep while holding its lock (the worst case for the watchdog).
import { test, expect, useStudio, openUi } from '../studio';

const studio = useStudio({ testHooks: true });
const arm = async () => (await studio.get('/api/arm')) as { armed: boolean; last_disarm: { reason: string; reason_fr: string } };
const armed = async () => (await arm()).armed;

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
});

test('a 200 ms engine stall disarms with « Moteur bloqué », and the operator can re-arm', async ({ page }) => {
  await openUi(page, studio);
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  expect(await studio.post('/api/test/stall?ms=200')).toBe(200);
  await expect.poll(armed).toBe(false);
  const a = await arm();
  expect(a.last_disarm.reason).toBe('engine_stall');
  expect(a.last_disarm.reason_fr).toBe('Moteur bloqué');
  await expect(page.locator('#engineLed')).toHaveText('Moteur bloqué — laser coupé');
  await expect(page.locator('#armInfo')).toContainText('Moteur bloqué');
  // Not latched: a deliberate arm works, and clears the message.
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await expect(page.locator('#engineLed')).toHaveText('Moteur');
});

test('a stall while disarmed changes nothing', async ({ page }) => {
  await openUi(page, studio);
  expect(await studio.post('/api/test/stall?ms=200')).toBe(200);
  await page.waitForTimeout(500);
  const a = await arm();
  expect(a.armed).toBe(false);
  expect(a.last_disarm.reason).not.toBe('engine_stall');
});
