// Extra windows (T-281): the same interface opened with ?vue=sortie,
// grille, direct or 3d, for a second screen or a touch screen. Two pages
// of one browser context share the grid page at once (BroadcastChannel);
// every window shows the laser state and stops it on Escape; a window
// that closes lets go of what it held. Preview only: the fake test output.
import type { Page } from '@playwright/test';
import { test, expect, useStudio, openWorkspace, focusPage } from '../studio';

const studio = useStudio();
const arm = async () => (await studio.get('/api/arm')) as { armed: boolean };
const armed = async () => (await arm()).armed;

// T-508: no window may throw while it loads or runs (an uncaught error or
// an unhandled rejection is a `pageerror`), whichever page of the context.
let pageErrors: string[] = [];
test.beforeEach(async ({ context, page }) => {
  pageErrors = [];
  const watch = (p: Page) => p.on('pageerror', e => pageErrors.push(`${p.url()}: ${e.message}`));
  watch(page);
  context.on('page', watch);
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/control', { id: 'page.1' });
});
test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/estop/reset', {});
  expect(pageErrors).toEqual([]);
});

/** The main window, on LIVE. (Not openUi: a test may leave no cue playing, so no points.) */
async function openMain(page: Page) {
  await page.goto(studio.url + '/');
  await openWorkspace(page, 'live');
  await expect(page.locator('#cues .cue').first()).toBeVisible();
}
/** A window of `page`'s browser context, opened at `?vue=…`. */
async function openVue(page: Page, vue: string): Promise<Page> {
  const w = await page.context().newPage();
  await w.goto(studio.url + '/?vue=' + vue);
  await expect(w.locator('#vueBar')).toBeVisible();
  return w;
}
/** The visible things a pointer could use on the page, by id or tag. */
function clickables(page: Page) {
  return page.evaluate(() => {
    const sel = 'button, input, select, textarea, a[href], summary, [role="button"], [role="tab"], [role="menuitem"], [tabindex]:not([tabindex="-1"])';
    return [...document.querySelectorAll<HTMLElement>(sel)]
      .filter(el => el.getClientRects().length > 0 && getComputedStyle(el).visibility !== 'hidden')
      .map(el => el.id || el.tagName.toLowerCase());
  });
}

test('?vue=grille: the cue grid and the top bar, no panel', async ({ page }) => {
  const w = await openVue(page, 'grille');
  await expect(w.locator('#cues .cue').first()).toBeVisible();
  await expect(w.locator('header')).toBeVisible();
  await expect(w.locator('#armBtn')).toBeVisible();
  await expect(w.locator('#estopBtn')).toBeVisible();
  await expect(w.locator('#blackoutBtn')).toBeVisible();
  for (const id of ['livePanel', 'layerRegion', 'stage', 'wsTabs']) await expect(w.locator('#' + id)).toBeHidden();
  await expect(w.locator('#vueName')).toHaveText('Fenêtre Grille');
  await expect(w.locator('#vueFull')).toBeVisible();
  // The window never becomes the main one's layout: F2 (TIMELINE) does nothing here.
  await focusPage(w);
  await w.keyboard.press('F2');
  await expect(w.locator('#cues .cue').first()).toBeVisible();
  expect(await w.evaluate(() => localStorage.getItem('laserStudio.workspace'))).not.toBe('timeline');
  // The top bar still fits the window.
  expect(await w.evaluate(() => document.querySelector('header')!.scrollWidth <= document.querySelector('header')!.clientWidth + 1)).toBe(true);
});

test('?vue=direct: the top bar and the Direct panel only', async ({ page }) => {
  const w = await openVue(page, 'direct');
  await expect(w.locator('#livePanel [data-panel="direct"]')).toBeVisible();
  await expect(w.locator('#armBtn')).toBeVisible();
  for (const id of ['cueRegion', 'layerRegion', 'stage', 'liveTabs', 'wsTabs']) await expect(w.locator('#' + id)).toBeHidden();
});

test('?vue=3d: the 3D view alone, with the laser state', async ({ page }) => {
  const w = await openVue(page, '3d');
  await expect(w.locator('header')).toBeHidden();
  await expect(w.locator('#vueArm')).toHaveText('LASER OFF');
  // The 3D view, or its message on a browser without WebGL2.
  await expect(w.locator('#beams:visible, #beamsMsg:visible')).toHaveCount(1);
  await expect(w.locator('#preview')).toBeHidden();
  await expect(w.locator('#viewSeg')).toBeHidden();
});

test('?vue=sortie: no control but « Plein écran »; the pointer hides after 2 s', async ({ page }) => {
  const w = await openVue(page, 'sortie');
  await expect(w.locator('#preview')).toBeVisible();
  await expect(w.locator('#vueArm')).toHaveText('LASER OFF');
  expect(await clickables(w)).toEqual(['vueFull']);
  await expect(w.locator('#vueFull')).toHaveText('Plein écran');
  await w.mouse.move(200, 200);
  await expect(w.locator('body')).not.toHaveClass(/vue-idle/);
  await expect(w.locator('body')).toHaveClass(/vue-idle/, { timeout: 3_500 });
  await w.mouse.move(220, 220);
  await expect(w.locator('body')).not.toHaveClass(/vue-idle/);
});

/** Epoch ms (shared by the pages) when `page`'s grid shows page `n`; 0 until then. */
async function watchPage(p: Page, n: number) {
  await p.evaluate(n => {
    const W = window as any;
    W.__seenAt = 0;
    // A MutationObserver runs as soon as the tabs change, unthrottled.
    new MutationObserver(() => {
      if (!W.__seenAt && W.__vue().page === n) W.__seenAt = performance.timeOrigin + performance.now();
    }).observe(document.getElementById('cueTabs')!, { subtree: true, childList: true, attributes: true });
  }, n);
}
async function clickTime(p: Page) {
  await p.evaluate(() => {
    const W = window as any;
    W.__clickAt = 0;
    document.getElementById('cueTabs')!.addEventListener('click', () => { W.__clickAt = performance.timeOrigin + performance.now(); }, { capture: true, once: true });
  });
}
const seenAt = (p: Page) => p.evaluate(() => (window as any).__seenAt as number);
const clickAt = (p: Page) => p.evaluate(() => (window as any).__clickAt as number);

test('changing the page in one window changes it in the other in < 200 ms', async ({ page }) => {
  await openMain(page);
  const grid = await studio.get('/api/grid');
  test.skip(grid.pages.length < 2, 'needs two grid pages');
  const w = await openVue(page, 'grille');
  await expect(w.locator('#cueTabs [role="tab"]').nth(1)).toBeVisible();
  expect(await w.evaluate(() => (window as any).__vue().page)).toBe(0);
  // Main window → grille window.
  await watchPage(w, 1); await clickTime(page);
  await page.locator('#cueTabs [role="tab"]').nth(1).click();
  await expect.poll(() => seenAt(w)).toBeGreaterThan(0);
  expect(await seenAt(w) - await clickAt(page)).toBeLessThan(200);
  await expect(w.locator('#cueTabs [role="tab"]').nth(1)).toHaveClass(/active/);
  // Grille window → main window.
  await watchPage(page, 0); await clickTime(w);
  await w.locator('#cueTabs [role="tab"]').nth(0).click();
  await expect.poll(() => seenAt(page)).toBeGreaterThan(0);
  expect(await seenAt(page) - await clickAt(w)).toBeLessThan(200);
  // And the studio follows (the engine state stays the studio's).
  await expect.poll(async () => (await studio.frame()).cue_page).toBe(0);
});

test('the cue pressed in one window is marked in the other', async ({ page }) => {
  await openMain(page);
  const w = await openVue(page, 'grille');
  const cue = page.locator('#cues .cue').first();
  const id = await cue.getAttribute('data-id');
  await cue.click();
  await expect(w.locator(`#cues .cue[data-id="${id}"]`)).toHaveClass(/\blinked\b/);
  await expect(cue).not.toHaveClass(/\blinked\b/);
});

test('every window shows the laser state; Escape in the Sortie window stops the laser', async ({ page }) => {
  await openMain(page);
  const w = await openVue(page, 'sortie');
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await expect(w.locator('#vueArm')).toHaveText('LASER ARMÉ');
  await expect(w.locator('#vueArm')).toHaveAttribute('data-state', 'armed');
  // Space does not arm from the Sortie window, Escape stops from it.
  await w.bringToFront();
  await w.locator('#preview').evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await w.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  expect((await studio.state()).estop).toBe(true);
  await expect(w.locator('#vueArm')).toHaveText('ARRÊT D\'URGENCE');
  await expect(page.locator('#estopBanner')).toBeVisible();
  await studio.post('/api/estop/reset', {});
  await expect(w.locator('#vueArm')).toHaveText('LASER OFF');
  await w.keyboard.press('Space');
  await page.waitForTimeout(300);
  expect(await armed()).toBe(false);
});

test('Escape in the Grille window stops the laser too', async ({ page }) => {
  await openMain(page);
  const w = await openVue(page, 'grille');
  await w.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await w.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  expect((await studio.state()).estop).toBe(true);
  await expect(w.locator('#estopBanner')).toBeVisible();
});

test('closing a window lets go of the flash it held; the laser stays as it was', async ({ page }) => {
  await openMain(page);
  const w = await openVue(page, 'grille');
  await expect(w.locator('#cues .cue').first()).toBeVisible();
  await focusPage(w);
  await w.keyboard.down('ShiftLeft');
  await w.keyboard.down('a'); // Shift + cue key = flash, held while the key is down
  await expect.poll(async () => (await studio.frame() as any).cues.active.filter((c: any) => c.held).length).toBe(1);
  await w.close();
  // The main window is still beating: only the closing window's release ends it.
  await expect.poll(async () => (await studio.frame() as any).cues.active.length, { timeout: 1_500, intervals: [100] }).toBe(0);
  expect((await studio.state()).presence.ui_alive).toBe(true);
  expect(await armed()).toBe(false);
});

test('« Ouvrir une fenêtre » opens the chosen window', async ({ page }) => {
  await openMain(page);
  await page.locator('#winBtn').click();
  await expect(page.locator('#winMenu')).toBeVisible();
  await expect(page.locator('#winMenu [role="menuitem"]')).toHaveText(['Sortie', 'Grille', 'Direct', '3D']);
  const [popup] = await Promise.all([page.waitForEvent('popup'), page.locator('#winMenu [data-vue="direct"]').click()]);
  await popup.waitForLoadState();
  expect(new URL(popup.url()).searchParams.get('vue')).toBe('direct');
  await expect(popup.locator('#vueName')).toHaveText('Fenêtre Direct');
  await expect(page.locator('#winMenu')).toBeHidden();
  // The main window keeps its layout: the window never wrote it.
  await expect(page.locator('#wsTabs')).toBeVisible();
  await expect(page.locator('#vueBar')).toBeHidden();
});
