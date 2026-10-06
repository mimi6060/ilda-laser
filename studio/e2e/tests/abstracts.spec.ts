// Abstract editor (T-440): CRÉATION › Abstraits. Two oscillators set to
// X 3 / Y 2 draw a Lissajous 3:2 in the engine-drawn preview (never the
// laser, never what plays in LIVE); « Points » at its maximum is cut by
// the layer's budget and says so; « Enregistrer comme cue… » makes a user
// cue that plays the same figure (/api/frame), within the budget; show
// mode refuses saving. Preview only: nothing is ever armed.
import type { Page } from '@playwright/test';
import { test, expect, useStudio, openUi, openWorkspace, isLit, type Point } from '../studio';

const studio = useStudio();

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

/** Packed preview picture (5 hex bytes a point) → points. */
function unpack(hex: string): Point[] {
  const out: Point[] = [];
  for (let i = 0; i + 10 <= hex.length; i += 10) {
    const v = [0, 2, 4, 6, 8].map(k => parseInt(hex.substr(i + k, 2), 16));
    out.push([v[0] / 127.5 - 1, v[1] / 127.5 - 1, v[2] / 255, v[3] / 255, v[4] / 255]);
  }
  return out;
}

/** Changes of direction along axis `k` of a closed lit curve (ties skipped). */
function turns(points: Point[], k: 0 | 1, eps = 0.004): number {
  const v = points.filter(isLit).map(p => p[k]);
  const signs: number[] = [];
  for (let i = 0; i < v.length; i++) {
    const d = v[(i + 1) % v.length] - v[i];
    if (Math.abs(d) > eps) signs.push(Math.sign(d));
  }
  let n = 0;
  for (let i = 0; i < signs.length; i++) if (signs[i] !== signs[(i + 1) % signs.length]) n++;
  return n;
}

async function openEditor(page: Page) {
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="abstract"]').click();
  await expect(page.locator('#absEdit')).toBeVisible();
  const reset = page.waitForResponse(r => r.url().endsWith('/api/abstracts/preview'));
  await page.locator('#absReset').click();
  await reset;
}

/** Sets oscillator `n` (1-based) to `ratio` and returns the next preview. */
async function setRatio(page: Page, n: number, ratio: number) {
  const pv = page.waitForResponse(r => r.url().endsWith('/api/abstracts/preview'));
  await page.locator(`#absRatio${n}`).fill(String(ratio));
  await page.locator(`#absRatio${n}`).press('Tab');
  return (await pv).json();
}

test('X 3 / Y 2 previews a Lissajous 3:2 without touching what plays', async ({ page }) => {
  const before = await studio.state();
  await openEditor(page);
  await expect(page.locator('#absOscs .abs-osc')).toHaveCount(2);
  await expect(page.locator('#absOscs .abs-osc').nth(0).locator('[data-target="x"]')).toHaveClass(/active/);
  await expect(page.locator('#absOscs .abs-osc').nth(1).locator('[data-target="y"]')).toHaveClass(/active/);
  await setRatio(page, 1, 3);
  const pv = await setRatio(page, 2, 2);
  expect(pv.frames.length).toBe(1);
  const pts = unpack(pv.frames[0]);
  expect(turns(pts, 0)).toBe(6); // 3 horizontal lobes: 3 maxima, 3 minima
  expect(turns(pts, 1)).toBe(4); // 2 vertical lobes
  await expect(page.locator('#absBudget')).toHaveText('400 points (budget 750)');
  await expect(page.locator('#absLimited')).toBeHidden();

  // A drift animates it: several pictures, the figure moves.
  const moving = page.waitForResponse(r => r.url().endsWith('/api/abstracts/preview'));
  await page.locator('#absDrift1').evaluate((el: HTMLInputElement) => { el.value = '0.25'; el.dispatchEvent(new Event('input', { bubbles: true })); });
  const mv = await (await moving).json();
  expect(mv.frames.length).toBeGreaterThan(8);
  expect(mv.frames[0]).not.toEqual(mv.frames[1]);

  // LIVE is untouched: same look, same revision.
  const after = await studio.state();
  expect(after.settings).toEqual(before.settings);
  expect(after.settings_rev).toBe(before.settings_rev);
  expect(after.armed).toBe(false);
});

test('« Points » at its maximum is cut by the layer budget', async ({ page }) => {
  await openEditor(page);
  const pv = page.waitForResponse(r => r.url().endsWith('/api/abstracts/preview'));
  await page.locator('#absPoints').evaluate((el: HTMLInputElement) => { el.value = '2000'; el.dispatchEvent(new Event('input', { bubbles: true })); });
  const p = await (await pv).json();
  expect(p.limited).toBe(true);
  expect(p.points).toBeLessThanOrEqual(p.budget);
  await expect(page.locator('#absBudget')).toHaveText('749 points (budget 750)');
  await expect(page.locator('#absLimited')).toBeVisible();
  await expect(page.locator('#absLimited')).toHaveText('Limité par le budget du calque');
});

test('« Enregistrer comme cue… » makes a cue that plays the same figure', async ({ page }) => {
  await openEditor(page);
  await setRatio(page, 1, 3);
  await setRatio(page, 2, 2);
  await page.locator('#absPoints').evaluate((el: HTMLInputElement) => { el.value = '2000'; el.dispatchEvent(new Event('input', { bubbles: true })); });
  page.once('dialog', d => d.accept('Lissajous e2e'));
  const stored = page.waitForResponse(r => r.url().endsWith('/api/abstracts/store'));
  await page.locator('#absStore').click();
  const g = await (await stored).json();
  await expect(page.locator('#absMsg')).toContainText('Enregistré dans la page');
  const cue = (await studio.presets()).presets.find(p => p.id === g.cue)!;
  expect(cue.name).toBe('Lissajous e2e');
  expect((await studio.get('/api/history')).undo.length).toBeGreaterThan(0);

  // Launched, it plays the 3:2 figure within the budget, disarmed.
  expect(await studio.post('/api/cue', { id: g.cue })).toBe(200);
  // (A launch may wait for its quantise point or fade in: poll.)
  await expect.poll(async () => { const f = await studio.frame(); return [turns(f.points, 0), turns(f.points, 1)]; }, { timeout: 10_000 }).toEqual([6, 4]);
  const frame = await studio.frame();
  expect(frame.points.length).toBeLessThanOrEqual(750);
  expect(turns(frame.points, 0)).toBe(6);
  expect(turns(frame.points, 1)).toBe(4);
  expect(frame.armed).toBe(false);
});

test('show mode refuses saving', async ({ page }) => {
  await openEditor(page);
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  try {
    await expect(page.locator('body')).toHaveClass(/show-mode/);
    expect(await studio.post('/api/abstracts/store', { look: { content: { kind: 'abstract' } } })).toBe(423);
    await page.evaluate(() => document.getElementById('absStore')!.click());
    await expect(page.locator('#absMsg')).toHaveText('Verrouillé en mode spectacle');
  } finally {
    await studio.post('/api/lock', { show_mode: false });
  }
});
