// Mode nuit et mode tactile (T-282): a darker interface whose safety
// colours stay at full strength, and a touch mode with 44 px targets, a
// long press that works like the right click and a flash cue held for as
// long as the finger is down. Preview only: the fake test output.
import { test, expect, useStudio, openUi, openWorkspace, reveal, focusPage } from '../studio';
import type { Page, CDPSession } from '@playwright/test';

const studio = useStudio();
const DISPLAY_KEY = 'laserStudio.display';

/** Mean luma (0–255) of a screenshot of the window. */
async function meanLuma(page: Page): Promise<number> {
  const png = (await page.screenshot()).toString('base64');
  return page.evaluate(async b64 => {
    const img = new Image();
    img.src = 'data:image/png;base64,' + b64;
    await img.decode();
    const c = document.createElement('canvas');
    c.width = img.width; c.height = img.height;
    const g = c.getContext('2d')!;
    g.drawImage(img, 0, 0);
    const d = g.getImageData(0, 0, c.width, c.height).data;
    let sum = 0;
    for (let i = 0; i < d.length; i += 4) sum += 0.2126 * d[i] + 0.7152 * d[i + 1] + 0.0722 * d[i + 2];
    return sum / (d.length / 4);
  }, png);
}

/** Set the display preferences the way the page stores them, then reload. */
async function withPrefs(page: Page, prefs: object) {
  await page.evaluate(([k, v]) => localStorage.setItem(k, v), [DISPLAY_KEY, JSON.stringify(prefs)]);
  await openUi(page, studio);
}

/** A real touch (CDP): finger down at the centre of `selector`, held `ms`, up. */
async function touchHold(page: Page, cdp: CDPSession, selector: string, ms: number, during?: () => Promise<void>) {
  await page.locator(selector).first().evaluate(e => e.scrollIntoView({ block: 'center' }));
  const box = (await page.locator(selector).first().boundingBox())!;
  const pt = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [pt] });
  const t0 = Date.now();
  if (during) await during();
  const left = ms - (Date.now() - t0);
  if (left > 0) await page.waitForTimeout(left);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
}

test.beforeEach(async ({ page }) => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  await openUi(page, studio);
  await page.evaluate(k => localStorage.removeItem(k), DISPLAY_KEY);
});

test.describe('mode nuit', () => {
  test('at 60 % the page is at least 35 % darker; safety colours unchanged', async ({ page }) => {
    await page.setViewportSize({ width: 1400, height: 900 });
    await openUi(page, studio);
    await focusPage(page);
    const day = await meanLuma(page);
    const colours = () => page.evaluate(() => {
      const st = (sel: string) => getComputedStyle(document.querySelector(sel)!);
      return { estop: st('#estopBtn').backgroundImage, arm: st('#armBtn').color, armBorder: st('#armBtn').borderTopColor,
        noir: st('#blackoutBtn').color, banner: st('#estopBanner').backgroundColor };
    });
    const dayColours = await colours();

    // Through the RÉGLAGES › Affichage controls.
    await reveal(page, '#displayPanel');
    await page.locator('#displayPanel summary').click();
    await expect(page.locator('#dispLevel')).toBeDisabled();
    await page.locator('#dispNight').check();
    await expect(page.locator('html')).toHaveClass(/\bnight\b/);
    await expect(page.locator('#dispLevelV')).toHaveText('60 %');
    expect(JSON.parse(await page.evaluate(k => localStorage.getItem(k)!, DISPLAY_KEY)))
      .toEqual({ night: true, nightLevel: 0.6, redTint: false, touch: 'auto' });

    // Same view as the day measure, after a reload (the choice is kept).
    await openUi(page, studio);
    await expect(page.locator('html')).toHaveClass(/\bnight\b/);
    await focusPage(page);
    const night = await meanLuma(page);
    expect(night, `day ${day.toFixed(1)}, night ${night.toFixed(1)}`).toBeLessThanOrEqual(day * 0.65);
    expect(await colours()).toEqual(dayColours);
    await page.screenshot({ path: 'test-results/night-60.png' });

    // Armed: the red frame and the lit laser button, as in day mode.
    await page.locator('#armBtn').click();
    await expect.poll(async () => (await studio.state()).armed).toBe(true);
    await expect(page.locator('#armBtn')).toHaveText('LASER ON');
    expect(await page.locator('#armBtn').evaluate(e => getComputedStyle(e).backgroundImage)).toContain('linear-gradient');
    await page.keyboard.press('Escape');
    await expect.poll(async () => (await studio.state()).armed).toBe(false);
    await expect(page.locator('#estopBanner')).toBeVisible();
    expect(await page.locator('#estopBanner').evaluate(e => getComputedStyle(e).backgroundColor)).toBe('rgb(208, 2, 27)');
    await studio.post('/api/estop/reset');
  });

  test('Teinte rouge: red neutrals, a thick outline on the armed laser button', async ({ page }) => {
    await withPrefs(page, { night: true, nightLevel: 0.8, redTint: true, touch: 'off' });
    await expect(page.locator('html')).toHaveClass(/red-tint/);
    const text = await page.evaluate(() => getComputedStyle(document.body).color);
    const [r, g, b] = text.match(/[\d.]+/g)!.map(Number);
    expect(r).toBeGreaterThan(g * 1.5);
    expect(r).toBeGreaterThan(b * 1.5);
    await page.locator('#armBtn').click();
    await expect.poll(async () => (await studio.state()).armed).toBe(true);
    await expect.poll(() => page.locator('#armBtn').evaluate(e => parseFloat(getComputedStyle(e).outlineWidth))).toBeGreaterThanOrEqual(4);
    await page.locator('#blackoutBtn').click();
    await expect.poll(async () => (await studio.state()).armed).toBe(false);
  });

  test('a broken stored value falls back to the defaults', async ({ page }) => {
    await page.evaluate(k => localStorage.setItem(k, '{not json'), DISPLAY_KEY);
    await openUi(page, studio);
    await expect(page.locator('html')).not.toHaveClass(/\bnight\b/);
    await withPrefs(page, { night: true, nightLevel: 0.1, touch: 'nope' });
    expect(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--nl').trim())).toBe('0.4');
  });
});

test.describe('mode tactile', () => {
  test.use({ hasTouch: true });

  test('forced on: every button and field of the top bar is at least 44 px high', async ({ page }) => {
    await withPrefs(page, { touch: 'on' });
    await expect(page.locator('html')).toHaveClass(/\btouch\b/);
    const small = await page.evaluate(() => [...document.querySelectorAll('header button, header input, header select')]
      .filter(el => (el as HTMLElement).offsetParent && !el.closest('#projMenu'))
      .map(el => ({ id: el.id || el.textContent!.trim(), h: el.getBoundingClientRect().height }))
      .filter(x => x.h < 44));
    expect(small).toEqual([]);
    // The global search (T-429) and the Direct scope (T-144) too.
    expect((await page.locator('#searchBtn').boundingBox())!.height).toBeGreaterThanOrEqual(44);
    // Taller grid cells.
    expect((await page.locator('#cues .cue').first().boundingBox())!.height).toBeGreaterThanOrEqual(76);
    expect(await page.evaluate(() => getComputedStyle(document.body).touchAction)).toBe('manipulation');
    await page.screenshot({ path: 'test-results/touch-top-bar.png', clip: { x: 0, y: 0, width: 1400, height: 160 } });

    // Off: back to the desk sizes; the choice is in RÉGLAGES › Affichage.
    await reveal(page, '#displayPanel');
    await page.locator('#displayPanel summary').click();
    await page.locator('#dispTouch [data-touch="off"]').click();
    await expect(page.locator('html')).not.toHaveClass(/\btouch\b/);
    await expect(page.locator('#dispTouch [data-touch="off"]')).toHaveAttribute('aria-checked', 'true');
  });

  test('auto follows the pointer (coarse = touch)', async ({ page }) => {
    const coarse = await page.evaluate(() => matchMedia('(pointer: coarse)').matches);
    await expect(page.locator('html')).toHaveClass(coarse ? /\btouch\b/ : /^(?!.*\btouch\b)/);
    await reveal(page, '#displayPanel');
    await page.locator('#displayPanel summary').click();
    await expect(page.locator('#dispTouch [data-touch="auto"]')).toHaveClass(/active/);
    await expect(page.locator('#dispTouchInfo')).toHaveText(coarse ? /activé/ : /désactivé/);
  });

  test('a long press on a Direct slider puts it back to neutral', async ({ page }) => {
    await withPrefs(page, { touch: 'on' });
    const cdp = await page.context().newCDPSession(page);
    await reveal(page, '#mSize');
    await studio.post('/api/control', { id: 'master.size', value: 0.4 });
    await expect(page.locator('#mSize')).toHaveValue('40');
    await expect.poll(async () => (await studio.live()).size).toBeCloseTo(0.4, 3);
    // A short tap is not a long press (and sets nothing by itself on a slider press).
    await touchHold(page, cdp, '#mSize', 900);
    await expect.poll(async () => (await studio.live()).size).toBeCloseTo(1, 3);
    await expect(page.locator('#mSize')).toHaveValue('100');
    // It stays there: the press did not also send the tapped position.
    await page.waitForTimeout(700);
    expect((await studio.live()).size).toBeCloseTo(1, 3);
    await expect(page.locator('#midiMenu')).toBeHidden();
  });

  test('a long press elsewhere is the right click; never on the laser button', async ({ page }) => {
    await withPrefs(page, { touch: 'on' });
    const cdp = await page.context().newCDPSession(page);
    // A button with a MIDI control: its « Apprendre MIDI » menu.
    await reveal(page, '#liveReset');
    await touchHold(page, cdp, '#zoomIn', 800);
    await expect(page.locator('#lockToast')).toBeVisible(); // its help, shown without hover
    await touchHold(page, cdp, '#rotSync', 800);
    await expect(page.locator('#midiMenu')).toBeVisible();
    await page.locator('h1').tap();
    await expect(page.locator('#midiMenu')).toBeHidden();
    // The laser button: a long press never arms, nor opens anything.
    await touchHold(page, cdp, '#armBtn', 900);
    await page.waitForTimeout(300);
    // The release is a tap, which is the explicit arm action; disarm right away.
    if ((await studio.state()).armed) await page.keyboard.press('Shift+Escape');
    await expect(page.locator('#midiMenu')).toBeHidden();
    await expect.poll(async () => (await studio.state()).armed).toBe(false);
  });

  test('a flash cue stays on while the finger is down', async ({ page }) => {
    await withPrefs(page, { touch: 'on' });
    const cdp = await page.context().newCDPSession(page);
    await studio.post('/api/control', { id: 'cue.mode', value: 1 }); // Flash
    await expect(page.locator('#cueModes [data-mode="flash"]')).toHaveClass(/active/);
    const cue = page.locator('#cues .cue').nth(2);
    const id = await cue.getAttribute('data-id');
    const sel = `#cues .cue[data-id="${id}"]`;
    await touchHold(page, cdp, sel, 1500, async () => {
      await expect(cue).toHaveClass(/\bflash\b/);
      await page.waitForTimeout(900); // well past a long press: still held, no menu
      await expect(cue).toHaveClass(/\bflash\b/);
      await expect(page.locator('#cueMenu')).toBeHidden();
    });
    await expect(cue).not.toHaveClass(/\bflash\b/);
    await expect(cue).not.toHaveClass(/\bactive\b/);
    expect((await studio.state()).armed).toBe(false);
  });
});

test('Affichage is a RÉGLAGES section of its own', async ({ page }) => {
  await openWorkspace(page, 'settings');
  await expect(page.locator('#displayPanel summary')).toHaveText('Affichage');
  await expect(page.locator('#displayPanel')).toHaveAttribute('data-search', /Affichage/);
});
