// Top bar (T-271): always on screen, with tempo, Maître, the output state,
// Noir, ARRÊT and the laser button on the right edge. Noir is a plain
// disarm (like Shift+Escape) and never anything else; Escape stops even
// from a text field; armed = a red frame around the window; a dead engine
// link shows an orange warning. Preview only: the fake test output.
import { test, expect, useStudio, openUi, openWorkspace, focusPage, type Workspace } from '../studio';
import type { Page } from '@playwright/test';
import { readFileSync } from 'node:fs';

const studio = useStudio();
const WORKSPACES: Workspace[] = ['live', 'timeline', 'creation', 'settings', 'desk'];
const armed = async () => (await studio.state()).armed as boolean;
/** The fake output's last arm/light event: what the « laser » last did. */
const lastOutputEvent = () => readFileSync(studio.outputLog, 'utf8').trim().split('\n')
  .filter(l => ['arm', 'disarm', 'lit', 'dark', 'blank'].includes(l)).pop();

async function arm(page: Page) {
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
}
/** Width of the red frame drawn around the window (0 = none). */
const frameWidth = (page: Page) => page.evaluate(() => {
  const st = getComputedStyle(document.body, '::after');
  return st.content === 'none' ? 0 : parseFloat(st.borderTopWidth);
});

test.beforeEach(async ({ page }) => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  await openUi(page, studio);
});

test('Noir disarms at once, without latching the stop, also in show mode', async ({ page }) => {
  await arm(page);
  await expect(page.locator('#armBtn')).toHaveText('LASER ON');
  const t0 = Date.now();
  await page.locator('#blackoutBtn').click();
  await expect.poll(armed).toBe(false);
  // The output is told to stop (the preview keeps drawing the look).
  await expect.poll(lastOutputEvent).not.toBe('lit');
  expect(['disarm', 'dark', 'blank']).toContain(lastOutputEvent());
  expect(Date.now() - t0).toBeLessThan(2000);
  // A blackout, not the e-stop: Space arms again straight away.
  expect((await studio.state()).estop).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');

  // Show mode (T-283) locks editing, never Noir.
  await page.locator('#lockBtn').click();
  await expect.poll(async () => (await studio.get('/api/lock')).show_mode).toBe(true);
  await arm(page);
  await page.locator('#blackoutBtn').click();
  await expect.poll(armed).toBe(false);
  // Clicking Noir again never arms.
  await page.locator('#blackoutBtn').click();
  await page.waitForTimeout(200);
  expect(await armed()).toBe(false);
});

test('Escape stops even with the focus in a text field', async ({ page }) => {
  await arm(page);
  await page.locator('#bpm').click();
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  expect((await studio.state()).estop).toBe(true);
  await expect(page.locator('#estopBanner')).toBeVisible();
});

test('armed: a red frame around the window; disarmed: none', async ({ page }) => {
  expect(await frameWidth(page)).toBe(0);
  await arm(page);
  await expect.poll(() => frameWidth(page)).toBe(3);
  await page.locator('#blackoutBtn').click();
  await expect.poll(() => frameWidth(page)).toBe(0);
});

test('the bar stays on screen in every workspace and while scrolling', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 600 });
  for (const ws of WORKSPACES) {
    await openWorkspace(page, ws);
    await page.mouse.wheel(0, 3000);
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    for (const sel of ['#bpm', '#topMaster', '#output', '#blackoutBtn', '#estopBtn', '#armBtn']) {
      await expect(page.locator(sel), `${sel} in ${ws}`).toBeInViewport();
    }
  }
});

test('at 1280 px: one row, laser button at the right edge, 24 px from the rest', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  // Auto tempo widens the tempo zone (Guider, follow state): still no scroll.
  await page.locator('#tempoAuto').click();
  await expect(page.locator('#tempoGuide')).toBeVisible();
  for (const ws of WORKSPACES) {
    await openWorkspace(page, ws);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), ws).toBe(true);
  }
  const geo = await page.evaluate(() => {
    const r = (sel: string) => document.querySelector(sel)!.getBoundingClientRect();
    const header = r('header'), btn = r('#armBtn');
    const others = [...document.querySelectorAll('header button, header input, header #output')]
      .filter(el => el.id !== 'armBtn' && (el as HTMLElement).offsetParent)
      .map(el => el.getBoundingClientRect());
    const gap = Math.min(...others.filter(o => o.top < btn.bottom && o.bottom > btn.top).map(o => btn.left - o.right));
    return { height: header.height, rightGap: header.right - btn.right, gap, bottom: btn.bottom, headerBottom: header.bottom };
  });
  expect(geo.height).toBeLessThanOrEqual(72);
  expect(geo.rightGap).toBeLessThan(24);
  expect(geo.gap).toBeGreaterThanOrEqual(24);
  await page.locator('#tempoAuto').click();
  await page.screenshot({ path: 'test-results/top-bar-1280.png', clip: { x: 0, y: 0, width: 1280, height: 120 } });
});

test('at 900 px the bar wraps but everything stays reachable', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 800 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  for (const sel of ['#projBtn', '#bpm', '#topMaster', '#output', '#armInfo', '#blackoutBtn', '#estopBtn', '#armBtn']) {
    await expect(page.locator(sel), sel).toBeInViewport();
  }
  await page.screenshot({ path: 'test-results/top-bar-900.png', clip: { x: 0, y: 0, width: 900, height: 160 } });
});

test('Maître drives master.brightness, never above 100 %, in sync with LIVE', async ({ page }) => {
  const master = page.locator('#topMaster');
  await expect(master).toHaveAttribute('max', '100');
  await master.fill('40');
  await focusPage(page); // a focused slider is not overwritten by the server
  await expect.poll(async () => (await studio.live()).brightness).toBeCloseTo(0.4, 2);
  await expect(page.locator('#topMasterV')).toHaveText('40 %');
  await expect(page.locator('#mBright')).toHaveValue('40');
  // Moved elsewhere (API, MIDI): the top bar follows.
  await studio.post('/api/control', { id: 'master.brightness', value: 0.75 });
  await expect(master).toHaveValue('75');
  // Clamped by the server too.
  await studio.post('/api/control', { id: 'master.brightness', value: 5 });
  await expect.poll(async () => (await studio.live()).brightness).toBeLessThanOrEqual(1);
});

// T-504: at the usual screen sizes, no horizontal scroll, every top-bar
// and tab-bar control inside the window, and the arm state with its
// reason never cut (no ellipsis, no clipping by the status column).
const SIZES = [[1024, 768], [1280, 800], [1440, 900], [1920, 1080]] as const;
for (const [width, height] of SIZES) {
  test(`at ${width}×${height}: nothing off screen, the arm reason in full`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    for (const ws of WORKSPACES) {
      await openWorkspace(page, ws);
      const fit = await page.evaluate(() => {
        const de = document.documentElement;
        const shown = (el: Element) => !!(el as HTMLElement).offsetParent;
        const off = [...document.querySelectorAll('header button, header input, #wsTabs button')].filter(shown)
          .filter(el => { const r = el.getBoundingClientRect(); return r.left < -0.5 || r.right > innerWidth + 0.5 || r.top < -0.5; })
          .map(el => el.id || el.textContent!.trim());
        const line = document.querySelector('.arm-line')!, status = document.querySelector('.tb-status')!.getBoundingClientRect();
        const info = document.querySelector('#armInfo')!;
        const outside = [...info.getClientRects()].some(r => r.left < status.left - 0.5 || r.right > status.right + 0.5 || r.top < status.top - 0.5 || r.bottom > status.bottom + 0.5);
        const lock = document.querySelector('#lockBtn')!;
        return { scroll: de.scrollWidth - de.clientWidth, off, text: info.textContent,
          cut: line.scrollWidth > line.clientWidth || line.scrollHeight > line.clientHeight || outside,
          lock: lock.scrollWidth - lock.clientWidth };
      });
      expect(fit.scroll, `horizontal scroll in ${ws}`).toBe(0);
      expect(fit.off, `controls off screen in ${ws}`).toEqual([]);
      expect(fit.text, ws).toMatch(/^Désarmé/);
      expect(fit.cut, `arm state cut in ${ws}: ${fit.text}`).toBe(false);
      expect(fit.lock, `#lockBtn overflows in ${ws}`).toBeLessThanOrEqual(0);
      for (const sel of ['#blackoutBtn', '#estopBtn', '#armBtn', '#armInfo']) {
        await expect(page.locator(sel), `${sel} in ${ws}`).toBeInViewport({ ratio: 1 });
      }
    }
    // The secondary tempo controls: inline from 1900 px, else behind « ⋯ »,
    // and once shown they fit in the window too.
    await openWorkspace(page, 'live');
    if (width < 1900) {
      await expect(page.locator('#latBtn')).toBeHidden();
      await page.locator('#tbMoreBtn').click();
      await expect(page.locator('#tbMoreBtn')).toHaveAttribute('aria-expanded', 'true');
    } else {
      await expect(page.locator('#tbMoreBtn')).toBeHidden();
    }
    for (const sel of ['button[data-ctl="tempo.half"]', '#latBtn', '#vljBtn']) {
      await expect(page.locator(sel), sel).toBeInViewport({ ratio: 1 });
    }
    if (width < 1900) {
      await page.locator('h1').click();
      await expect(page.locator('#tbMore')).toBeHidden();
      await expect(page.locator('#tbMoreBtn')).toHaveAttribute('aria-expanded', 'false');
    }
  });
}

test('a long disarm reason wraps instead of being cut, at 1280 px', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  // The longest text the line can take: the reason and its source, the time.
  // Set and measured in one go: the next frame writes the real state back.
  const r = await page.evaluate(() => {
    document.querySelector('#armInfo')!.textContent = 'Désarmé — raison : Sortie perdue, reconnexion en cours (système), 23:59:59';
    const line = document.querySelector('.arm-line')!, arm = document.querySelector('#armBtn')!.getBoundingClientRect();
    const status = document.querySelector('.tb-status')!.getBoundingClientRect();
    const outside = [...document.querySelector('#armInfo')!.getClientRects()].some(q => q.right > status.right + 0.5 || q.bottom > status.bottom + 0.5);
    return { cut: line.scrollWidth > line.clientWidth || line.scrollHeight > line.clientHeight || outside,
      armInside: arm.right <= innerWidth && arm.top >= 0, scroll: document.documentElement.scrollWidth > innerWidth };
  });
  expect(r).toEqual({ cut: false, armInside: true, scroll: false });
});

// Last: stops this file's studio.
test('engine gone: « Connexion au moteur perdue » within 2 s', async ({ page }) => {
  await focusPage(page);
  await expect(page.locator('#linkLost')).toBeHidden();
  await studio.stop();
  await expect(page.locator('#linkLost')).toBeVisible({ timeout: 2000 });
  await expect(page.locator('#linkLost')).toHaveText('Connexion au moteur perdue');
});
