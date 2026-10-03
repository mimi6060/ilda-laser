// T-264: an arm request refused by the interlocks says why, lists every
// lock, and nothing is lit. The studio runs with --test-interlock (a lock
// that is never satisfied); preview only, no output.
import { test, expect, useStudio, openUi, focusPage } from '../studio';

const studio = useStudio({ testInterlock: true });
const armed = async () => (await studio.state()).armed as boolean;

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await openUi(page, studio);
});

test('the LASER button is refused with the list of locks, and nothing is lit', async ({ page }) => {
  await page.locator('#armBtn').click();
  const box = page.locator('#armRefused');
  await expect(box).toBeVisible();
  await expect(box).toContainText('Armement impossible');
  await expect(box.locator('li')).toContainText(['Verrou de test']);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  expect(await armed()).toBe(false);
  expect((await studio.get('/api/frame')).output_lit).toBe(0);
});

test('a latched emergency stop is listed with the other locks; its reset does not arm', async ({ page }) => {
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await studio.get('/api/arm')).estop !== null).toBe(true);
  await page.locator('#armBtn').click();
  const items = page.locator('#armRefused li');
  await expect(items).toHaveCount(2);
  await expect(items).toContainText(["Arrêt d'urgence enclenché", 'Verrou de test']);
  await studio.post('/api/estop/reset', {});
  expect(await armed()).toBe(false);
  // The API refuses too, with the same list (409).
  const r = await fetch(`${studio.url}/api/arm`, { method: 'POST', body: JSON.stringify({ on: true, source: 'ui' }) });
  expect(r.status).toBe(409);
  expect((await r.json()).blocking.join(' ')).toContain('Verrou de test');
  expect(await armed()).toBe(false);
});
