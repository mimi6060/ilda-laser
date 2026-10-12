// Modules ES (T-435): the page is markup, the code lives in /ui/*.js served
// by the studio itself. Preview only: no laser, nothing armed.
import { test, expect, useStudio, openUi, openWorkspace, focusPage, type Workspace } from '../studio';

const studio = useStudio();

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
});

const INCOMPLETE = 'Interface incomplète : rechargez la page';

test('the page loads from the studio only, every module in page order', async ({ page }) => {
  const urls: string[] = [];
  page.on('request', r => urls.push(r.url()));
  await openUi(page, studio);
  for (const ws of ['timeline', 'creation', 'settings', 'live'] as Workspace[]) await openWorkspace(page, ws);
  // blob: URLs (the heartbeat worker) never leave the page.
  const hosts = new Set(urls.map(u => new URL(u)).filter(u => u.protocol.startsWith('http')).map(u => u.host));
  expect([...hosts]).toEqual([new URL(studio.url).host]);
  const modules = urls.filter(u => new URL(u).pathname.startsWith('/ui/')).map(u => new URL(u).pathname);
  expect(modules).toContain('/ui/app.js');
  expect(modules).toContain('/ui/styles.css');
  expect(modules).toContain('/ui/core.js');
  expect(modules).toContain('/ui/command.js');
  await expect(page.locator('#uiIncomplete')).toBeHidden();
  // app.js ran last: Échap is laser.js's from now on.
  expect(await page.evaluate(() => document.documentElement.dataset.uiReady)).toBe('');
});

test('a module that does not load: the page says so and Échap still stops the laser', async ({ page }) => {
  await page.route('**/ui/cues.js', r => r.abort());
  await page.goto(studio.url + '/');
  await expect(page.locator('#uiIncomplete')).toBeVisible();
  await expect(page.locator('#uiIncomplete')).toHaveText(INCOMPLETE);
  expect(await page.evaluate(() => document.documentElement.dataset.uiReady)).toBeUndefined();
  // The guard of <head>: the latched stop, its banner, nothing imported.
  await focusPage(page);
  const stopped = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === '/api/estop');
  await page.keyboard.press('Escape');
  expect((await stopped).status()).toBe(200);
  await expect(page.locator('#estopBanner')).toBeVisible();
  await expect(page.locator('#estopBanner')).toContainText("ARRÊT D'URGENCE — réinitialiser pour réarmer");
  const arm = await studio.get<{ armed: boolean; estop: unknown }>('/api/arm');
  expect(arm.armed).toBe(false);
  expect(arm.estop).not.toBeNull();
});

test('without the entry module at all, Échap still stops the laser', async ({ page }) => {
  await page.route('**/ui/app.js', r => r.abort());
  await page.goto(studio.url + '/');
  await expect(page.locator('#uiIncomplete')).toBeVisible();
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await focusPage(page);
  const stopped = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === '/api/estop');
  await page.keyboard.press('Escape');
  expect((await stopped).status()).toBe(200);
  await expect(page.locator('#estopBanner')).toBeVisible();
  expect((await studio.get<{ estop: unknown }>('/api/arm')).estop).not.toBeNull();
});
