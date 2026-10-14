// Régression visuelle (T-434): screenshots of the four workspaces at two
// window sizes, and of the safety controls (LASER, Noir, ARRÊT, « Ciel
// coupé », the e-stop banner, a safety warning) in day, night and touch
// modes, compared with the reference images committed next to this file.
//
// Deterministic by construction: a fresh data dir (the harness's), fixed
// viewports, reduced motion (CSS animations and transitions off), the
// system font, the caret hidden, the mouse parked off every control, and
// every moving area masked (2D/3D preview, cue thumbnails, tempo lights,
// waveforms, clocks and counters). Tolerance: 0.1 % of the pixels.
//
// The references are macOS renderings (the project's platform): font
// rendering differs elsewhere, so the screenshots are skipped there, with
// a message. Regenerate after an intended change with
// `npx playwright test visual --update-snapshots` and look at every image.
//
// Preview only: the fake test output; arming is a click on the LASER
// button, like the operator's, and every test ends disarmed.
import { test, expect, useStudio, openUi, openWorkspace, focusPage, reveal, type Workspace } from '../studio';
import type { Locator, Page } from '@playwright/test';

const studio = useStudio();
const DISPLAY_KEY = 'laserStudio.display';
const SIZES = [{ width: 1440, height: 900 }, { width: 1280, height: 800 }] as const;
const WORKSPACES: Workspace[] = ['live', 'timeline', 'creation', 'settings'];
const SHOT = { maxDiffPixelRatio: 0.001, animations: 'disabled', caret: 'hide', scale: 'css' } as const;

test.use({ reducedMotion: 'reduce' });
test.skip(process.platform !== 'darwin', 'reference images are macOS renderings (font rendering differs elsewhere)');

/** Everything that moves on its own or shows the time. */
function moving(page: Page): Locator[] {
  return [
    page.locator('canvas'), // 2D/3D preview, cue thumbnails, timeline, waveforms, meters
    page.locator('#beats, #gBeats, #latFlash'), // tempo lights
    page.locator('.stats, #stPoints'), // points / fps counters
    page.locator('#armInfo, #presenceLed, #engineLed'), // arm duration, heartbeat
    page.locator('#toasts, #noticeBadge'), // notifications appear on their own schedule
    page.locator('[id*="Clock"], [id*="clock"], [id$="Time"], [id*="Tc"], [id*="Pos"]'), // clocks, timecode, playhead
  ];
}

/** Open the page with these display preferences (night, touch) at this size. */
async function openWith(page: Page, size: { width: number; height: number }, prefs: object) {
  await page.setViewportSize(size);
  await openUi(page, studio);
  await page.evaluate(([k, v]) => localStorage.setItem(k, v), [DISPLAY_KEY, JSON.stringify(prefs)]);
  await openUi(page, studio);
  await settle(page);
}

/** No hover, no focus ring, fonts loaded, one layout pass done. */
async function settle(page: Page) {
  await page.mouse.move(0, 0);
  await page.evaluate(async () => {
    (document.activeElement as HTMLElement | null)?.blur();
    await document.fonts.ready;
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
  });
}

test.beforeEach(async ({ page }) => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  await page.goto(studio.url + '/');
  await page.evaluate(k => localStorage.removeItem(k), DISPLAY_KEY);
});

test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  if ((await studio.state()).outdoor?.sky_off) await studio.post('/api/safety/sky', { on: false, source: 'ui' });
});

for (const size of SIZES) {
  test.describe(`${size.width}×${size.height}`, () => {
    for (const ws of WORKSPACES) {
      test(`workspace ${ws}`, async ({ page }) => {
        await openWith(page, size, { touch: 'off' });
        await openWorkspace(page, ws);
        await settle(page);
        await expect(page).toHaveScreenshot(`ws-${ws}-${size.width}x${size.height}.png`, { ...SHOT, mask: moving(page) });
      });
    }

    // T-507: the effect names, two rows of four cells per line.
    test('LIVE › Effets', async ({ page }) => {
      await openWith(page, size, { touch: 'off' });
      await reveal(page, '#fgGrid');
      await expect(page.locator('#fgGrid .fg-cell')).toHaveCount(32);
      await settle(page);
      await expect(page).toHaveScreenshot(`live-fx-${size.width}x${size.height}.png`, { ...SHOT, mask: moving(page) });
    });

    test('LIVE shows the preview, the cue grid and the top bar without scrolling the page', async ({ page }) => {
      await openWith(page, size, { touch: 'off' });
      const fits = await page.evaluate(() => document.documentElement.scrollHeight <= innerHeight);
      expect(fits, 'no vertical page scroll in LIVE').toBe(true);
      for (const sel of ['#stage', '#cueRegion', 'header']) {
        const box = (await page.locator(sel).boundingBox())!;
        expect(box, sel).not.toBeNull();
        expect(box.y, `${sel} top`).toBeGreaterThanOrEqual(0);
        expect(box.y + box.height, `${sel} bottom`).toBeLessThanOrEqual(size.height + 0.5);
        expect(box.x + box.width, `${sel} right`).toBeLessThanOrEqual(size.width + 0.5);
      }
    });
  });
}

const MODES = {
  day: { touch: 'off' },
  night: { night: true, nightLevel: 0.6, redTint: false, touch: 'off' },
  touch: { touch: 'on' },
} as const;

for (const [mode, prefs] of Object.entries(MODES)) {
  test.describe(`safety controls, ${mode}`, () => {
    test('LASER, Noir, ARRÊT and « Ciel coupé », disarmed then armed', async ({ page }) => {
      await openWith(page, SIZES[0], prefs);
      if (mode === 'night') await expect(page.locator('html')).toHaveClass(/\bnight\b/);
      if (mode === 'touch') await expect(page.locator('html')).toHaveClass(/\btouch\b/);
      // « Ciel coupé » only shows once the sky is cut (or the outdoor mode is on).
      expect(await studio.post('/api/safety/sky', { on: true, source: 'ui' })).toBe(200);
      await expect(page.locator('#skyBtn')).toHaveAttribute('aria-pressed', 'true');
      const controls = page.locator('.tb-safety');
      const arm = page.locator('#armBtn');
      await settle(page);
      await expect(controls).toHaveScreenshot(`safety-${mode}-sky-cut.png`, SHOT);
      await expect(arm).toHaveScreenshot(`laser-${mode}-off.png`, SHOT);

      // The sky back: the blue button hides, Noir and ARRÊT stay.
      expect(await studio.post('/api/safety/sky', { on: false, source: 'ui' })).toBe(200);
      await expect(page.locator('#skyBtn')).toBeHidden();
      await settle(page);
      await expect(controls).toHaveScreenshot(`safety-${mode}.png`, SHOT);

      // Armed by the operator's click (fake output, never a laser).
      await arm.click();
      await expect.poll(async () => (await studio.state()).armed).toBe(true);
      await expect(arm).toHaveClass(/\bon\b/);
      await settle(page);
      await expect(arm).toHaveScreenshot(`laser-${mode}-on.png`, SHOT);
      await page.locator('#blackoutBtn').click();
      await expect.poll(async () => (await studio.state()).armed).toBe(false);
    });

    test('the e-stop banner and a safety warning', async ({ page }) => {
      await openWith(page, SIZES[0], prefs);
      expect(await studio.post('/api/estop')).toBe(200);
      const banner = page.locator('#estopBanner');
      await expect(banner).toBeVisible();
      await settle(page);
      await expect(banner).toHaveScreenshot(`estop-banner-${mode}.png`, SHOT);
      await page.locator('#estopReset').click();
      await expect(banner).toBeHidden();

      // The epilepsy warning of the shortcuts sheet (orange, never dimmed).
      await focusPage(page);
      await page.keyboard.press('Shift+Slash'); // « ? » on QWERTY
      const warning = page.locator('#keysHelp .hint--warn');
      await expect(warning).toBeVisible();
      await settle(page);
      await expect(warning).toHaveScreenshot(`epilepsy-warning-${mode}.png`, SHOT);
      await page.locator('#keysClose').click();
      await expect(page.locator('#keysHelp')).toBeHidden();
    });
  });
}
