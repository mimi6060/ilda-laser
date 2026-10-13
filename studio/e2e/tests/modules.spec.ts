// Modules ES (T-435): the page is markup, the code lives in /ui/*.js served
// by the studio itself. Preview only: no laser, nothing armed.
import type { Page } from '@playwright/test';
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

// T-508: a module that called into a later one while it was evaluated
// threw a ReferenceError (an unhandled rejection) in every ?vue= window.
/** Collects the page's uncaught errors and unhandled rejections. */
function pageErrors(page: Page) {
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(`${page.url()}: ${e.message}`));
  return errors;
}

test('no page error on the main page, whatever the stage, in every workspace', async ({ page }) => {
  const errors = pageErrors(page);
  for (const stage of ['2d', '3d', 'both']) {
    await page.goto(studio.url + '/');
    await page.evaluate(s => localStorage.setItem('laserStudio.stageTab', s), stage);
    await openUi(page, studio);
    await expect(page.locator(`#viewSeg [data-view="${stage}"]`)).toHaveClass(/active/);
    for (const ws of ['timeline', 'creation', 'settings', 'desk', 'live'] as Workspace[]) await openWorkspace(page, ws);
    await expect(page.locator('#uiIncomplete')).toBeHidden();
  }
  await page.evaluate(() => localStorage.setItem('laserStudio.stageTab', '2d'));
  expect(errors).toEqual([]);
});

test('no page error in any ?vue= window', async ({ page }) => {
  const errors = pageErrors(page);
  for (const vue of ['sortie', 'grille', 'direct', '3d', 'sortie&rendu=3d', 'sortie&rendu=both']) {
    await page.goto(studio.url + '/?vue=' + vue);
    await expect(page.locator('#vueBar')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.dataset.uiReady)).toBe('');
    // A beat for late rejections and a few frames.
    await page.waitForTimeout(300);
    await expect(page.locator('#uiIncomplete')).toBeHidden();
  }
  expect(errors).toEqual([]);
});

test('?vue=sortie draws the safety zones over the 2D preview', async ({ page }) => {
  const errors = pageErrors(page);
  const settings = (await studio.get('/api/safety')).settings;
  const zone = { name: 'Bande', kind: 'blank', points: [[-0.3, -1], [0.3, -1], [0.3, 1], [-0.3, 1]] };
  expect(await studio.post('/api/safety', { ...settings, zones: [zone], confirm_loosen: true })).toBe(200);
  try {
    await page.goto(studio.url + '/?vue=sortie');
    await expect(page.locator('#preview')).toBeVisible();
    // (0, 0.5): inside the stripe, off the centre cross.
    const pixel = () => page.locator('#preview').evaluate(c => {
      const cv = c as HTMLCanvasElement;
      const d = cv.getContext('2d')!.getImageData(Math.round(cv.width / 2) + 8, Math.round(cv.height / 4), 1, 1).data;
      return [d[0], d[1], d[2]];
    });
    await expect.poll(async () => { const [r, g, b] = await pixel(); return r > 40 && r > g + 30 && r > b + 30; }).toBe(true);
  } finally {
    expect(await studio.post('/api/safety', { ...settings, confirm_loosen: true })).toBe(200);
  }
  expect(errors).toEqual([]);
});

test('an unhandled rejection while the modules run: the page says so', async ({ page }) => {
  await page.route('**/ui/command.js', async r => {
    const res = await r.fetch();
    r.fulfill({ response: res, body: (await res.text()) + "\n(async () => { throw new Error('test T-508'); })();\n" });
  });
  await page.goto(studio.url + '/');
  await expect(page.locator('#uiIncomplete')).toBeVisible();
  await expect(page.locator('#uiIncomplete')).toHaveText(INCOMPLETE);
});
