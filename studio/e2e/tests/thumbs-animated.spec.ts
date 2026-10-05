// Animated cue thumbnails (T-273): a canvas per filled cell, looping one
// bar of the cue at the tempo; « Fixes » stops them, « Aucune » removes
// the canvases; a hidden tab draws nothing. Display only: asking for the
// loops never changes what plays, and the laser stays disarmed.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

// A fingerprint of every grid picture, to see them move.
const pictures = (page: Page) => page.locator('#cues .cue canvas.th').evaluateAll((cs: HTMLCanvasElement[]) => cs.map(c => {
  const d = c.getContext('2d')!.getImageData(0, 0, c.width, c.height).data;
  let h = 0;
  for (let i = 0; i < d.length; i += 4) h = (h * 31 + d[i] + 3 * d[i + 1] + 7 * d[i + 2]) | 0;
  return h;
}).join(','));

const setMode = async (page: Page, mode: 'animated' | 'static' | 'none') => {
  await openWorkspace(page, 'settings');
  if (!(await page.locator('#generalPanel').evaluate((d: HTMLDetailsElement) => d.open))) await page.locator('#generalPanel summary').click();
  await page.locator('#thumbMode').selectOption(mode);
  await openWorkspace(page, 'live');
};

test.beforeEach(async () => {
  await studio.reset();
});

test('every filled cell has a picture that plays its loop, and nothing else changes', async ({ page }) => {
  const loops: string[] = [];
  page.on('request', r => { if (r.url().includes('/api/presets/loops')) loops.push(r.url()); });
  await openUi(page, studio);
  const cells = page.locator('#cues .cue');
  await expect(cells.first()).toBeVisible();
  const n = await cells.count();
  expect(n).toBeGreaterThan(0);
  await expect(page.locator('#cues .cue canvas.th')).toHaveCount(n);
  // The page's cues are asked for, and the pictures move.
  await expect.poll(() => loops.length).toBeGreaterThan(0);
  const first = await pictures(page);
  await expect.poll(() => pictures(page), { timeout: 5_000 }).not.toBe(first);
  // Only the cues on screen were asked for.
  const ids = new Set(await cells.evaluateAll(bs => bs.map(b => (b as HTMLElement).dataset.id)));
  for (const u of loops) {
    for (const id of decodeURIComponent(new URL(u).searchParams.get('ids')!).split(',')) expect(ids.has(id), id).toBe(true);
  }
  const frame = await studio.frame();
  expect(frame.armed).toBe(false);
  expect(frame.active_cue).toBeNull();
});

test('a hidden tab stops the loops', async ({ page }) => {
  await openUi(page, studio);
  const first = await pictures(page);
  await expect.poll(() => pictures(page), { timeout: 5_000 }).not.toBe(first);
  await page.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  const still = await pictures(page);
  await page.waitForTimeout(1_200);
  expect(await pictures(page)).toBe(still);
  await page.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect.poll(() => pictures(page), { timeout: 5_000 }).not.toBe(still);
});

test('« Fixes » freezes the pictures, « Aucune » removes them, and it is remembered', async ({ page }) => {
  await openUi(page, studio);
  const n = await page.locator('#cues .cue').count();
  await setMode(page, 'static');
  await expect(page.locator('#cues .cue canvas.th')).toHaveCount(n);
  const still = await pictures(page);
  await page.waitForTimeout(1_200);
  expect(await pictures(page)).toBe(still);

  await setMode(page, 'none');
  await expect(page.locator('#cues .cue')).toHaveCount(n);
  await expect(page.locator('#cues canvas')).toHaveCount(0);
  await page.reload();
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await expect(page.locator('#cues canvas')).toHaveCount(0);
  await openWorkspace(page, 'settings');
  await expect(page.locator('#thumbMode')).toHaveValue('none');

  await setMode(page, 'animated');
  await expect(page.locator('#cues .cue canvas.th')).toHaveCount(n);
  const first = await pictures(page);
  await expect.poll(() => pictures(page), { timeout: 5_000 }).not.toBe(first);
});
