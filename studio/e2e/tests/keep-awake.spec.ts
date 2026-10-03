// Keep the Mac awake during a show (T-412): armed or playing → the sleep
// assertion is held, released within 6 s after. Preview only, and the
// harness's --test-keep-awake (plus --test-hooks here) means a fake
// blocker: this Mac's power settings are never touched.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio({ testHooks: true });
interface KeepAwake { enabled: boolean; held: boolean; error: string | null }
const keepAwake = async () => (await studio.get<{ keep_awake: KeepAwake }>('/api/state')).keep_awake;
const held = async () => (await keepAwake()).held;
const armed = async () => (await studio.state()).armed;

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/timeline/stop');
  await studio.reset(); // a stopped show leaves nothing drawn
  await studio.post('/api/keep_awake', { enabled: true });
});

test('arming holds the assertion; disarming lets go within 6 s', async ({ page }) => {
  await openUi(page, studio);
  await expect.poll(held).toBe(false);
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await expect.poll(held).toBe(true);
  await openWorkspace(page, 'settings');
  await page.locator('#generalPanel summary').click();
  await expect(page.locator('#kaStatus')).toHaveText('Veille bloquée');
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(false);
  // Still held just after (no flapping on short pauses), gone within 6 s.
  expect(await held()).toBe(true);
  await expect.poll(held, { timeout: 6_000 }).toBe(false);
  await expect(page.locator('#kaStatus')).toHaveText('Veille autorisée');
});

test('a playing timeline alone holds it, without arming', async () => {
  const catalog = await studio.presets();
  const show = {
    name: 'Veille e2e', time_base: 'seconds', tempo_map: [{ at_s: 0, bpm: 120, beats_per_bar: 4 }],
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len: 60, source: { kind: 'cue', id: catalog.presets[0].id } }] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
  expect(await studio.post('/api/timeline/load', { name: 'Veille e2e' })).toBe(200);
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await expect.poll(held).toBe(true);
  expect(await armed()).toBe(false);
  expect(await studio.post('/api/timeline/stop')).toBe(200);
  await expect.poll(held, { timeout: 6_000 }).toBe(false);
});

test('unchecked: nothing is held, and the setting survives a restart', async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#generalPanel summary').click();
  await expect(page.locator('#kaEnabled')).toBeChecked();
  await page.locator('#kaEnabled').uncheck();
  await expect.poll(async () => (await keepAwake()).enabled).toBe(false);
  await openWorkspace(page, 'live');
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await page.waitForTimeout(600);
  expect(await held()).toBe(false);
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(false);
  await studio.restart();
  expect((await keepAwake()).enabled).toBe(false);
  expect(await studio.post('/api/keep_awake', { enabled: true })).toBe(200);
});
