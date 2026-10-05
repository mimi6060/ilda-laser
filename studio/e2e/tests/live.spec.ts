// « Direct » panel: master live modifiers on top of any look or cue.
import { test, expect, useStudio, openUi, reveal, focusPage, extent, centroid, isLit } from '../studio';

const studio = useStudio();
const live = () => studio.live();
const values = async () => (await studio.controlValues()).values;
const points = async () => (await studio.frame()).points;

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('rotation presets set the Z rotation speed and highlight the button', async ({ page }) => {
  const presets = page.locator('#rotPresets button');
  await expect(presets.filter({ hasText: 'Stop' })).toHaveClass(/active/);

  await presets.filter({ hasText: 'Moyen' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(90);
  await expect(presets.filter({ hasText: 'Moyen' })).toHaveClass(/active/);
  await expect(presets.filter({ hasText: 'Stop' })).not.toHaveClass(/active/);

  await presets.filter({ hasText: 'Rapide' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(270);
  await expect(presets.filter({ hasText: 'Rapide' })).toHaveClass(/active/);

  await presets.filter({ hasText: 'Stop' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(0);
});

test('a rotation preset spins the drawing', async ({ page }) => {
  await reveal(page, '#shapes');
  await page.getByRole('button', { name: 'Carré', exact: true }).click();
  await expect.poll(async () => (await studio.state()).settings.content.shape).toBe('square');
  await reveal(page, '#rotPresets');
  await page.locator('#rotPresets button', { hasText: 'Rapide' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(270);
  const a = (await points()).find(isLit)!;
  await expect.poll(async () => {
    const b = (await points()).find(isLit)!;
    return Math.hypot(a[0] - b[0], a[1] - b[1]);
  }).toBeGreaterThan(0.01);
});

test('Synchro tempo keeps the preset step, in turns per bar', async ({ page }) => {
  await page.locator('#rotPresets button', { hasText: 'Moyen' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(90);
  await page.locator('#rotSync').check();
  await expect.poll(async () => (await live()).rot_sync).toBe(true);
  expect((await live()).rot_speed[2]).toBe(1); // Moyen = 1 turn per bar
  await expect(page.locator('#rotPresets button', { hasText: 'Moyen' })).toHaveClass(/active/);
  await page.locator('#rotSync').uncheck();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(90);
});

test('master size slider scales the output, on /api/live and /api/control-values', async ({ page }) => {
  await expect(page.locator('#mSizeV')).toHaveText('100 %');
  await expect.poll(async () => extent(await points())).toBeCloseTo(0.5, 1);
  await page.locator('#mSize').fill('150');
  await expect(page.locator('#mSizeV')).toHaveText('150 %');
  await expect.poll(async () => (await live()).size).toBeCloseTo(1.5, 3);
  expect((await values())['master.size']).toBeCloseTo(1.5, 3);
  await expect.poll(async () => extent(await points())).toBeCloseTo(0.75, 1);

  await page.locator('#mSize').fill('50');
  await expect.poll(async () => extent(await points())).toBeCloseTo(0.25, 1);
});

test('position X moves the drawing', async ({ page }) => {
  await page.locator('#mPosX').fill('40');
  await expect(page.locator('#mPosXV')).toHaveText('40 %');
  await expect.poll(async () => (await live()).pos_x).toBeCloseTo(0.4, 3);
  await expect.poll(async () => centroid(await points()).x).toBeCloseTo(0.4, 1);
});

test('master brightness dims every point', async ({ page }) => {
  const before = Math.max(...(await points()).map(p => p[3]));
  await page.locator('#mBright').fill('50');
  await expect.poll(async () => (await live()).brightness).toBeCloseTo(0.5, 3);
  await expect.poll(async () => Math.max(...(await points()).map(p => p[3]))).toBeCloseTo(before * 0.5, 2);
});

test('Réinitialiser brings every master modifier back to neutral', async ({ page }) => {
  await page.locator('#mSize').fill('150');
  await page.locator('#rotPresets button', { hasText: 'Lent' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(30);
  await expect.poll(async () => (await live()).size).toBeCloseTo(1.5, 3);
  await page.locator('#liveReset').click();
  await expect.poll(async () => (await live()).size).toBe(1);
  expect((await live()).rot_speed[2]).toBe(0);
  await expect(page.locator('#mSizeV')).toHaveText('100 %');
  await expect(page.locator('#rotPresets button', { hasText: 'Stop' })).toHaveClass(/active/);
});

test('Inverser reverses the rotation only while held (button and < key)', async ({ page }) => {
  const btn = page.locator('#rotReverse');
  await btn.hover();
  await page.mouse.down();
  await expect.poll(async () => (await live()).rot_reverse).toBe(true);
  await page.mouse.up();
  await expect.poll(async () => (await live()).rot_reverse).toBe(false);

  await focusPage(page);
  await page.keyboard.down('<');
  await expect.poll(async () => (await live()).rot_reverse).toBe(true);
  await page.keyboard.up('<');
  await expect.poll(async () => (await live()).rot_reverse).toBe(false);
});

test('a live change made elsewhere (MIDI/API) moves the slider', async ({ page }) => {
  expect(await studio.post('/api/control', { id: 'master.size', value: 0.8 })).toBe(200);
  await expect(page.locator('#mSizeV')).toHaveText('80 %');
  await expect(page.locator('#mSize')).toHaveValue('80');
});

// ---------- T-143: the full « Direct » panel ----------
const square = async (page: import('@playwright/test').Page) => {
  await reveal(page, '#shapes');
  await page.getByRole('button', { name: 'Carré', exact: true }).click();
  await expect.poll(async () => (await studio.state()).settings.content.shape).toBe('square');
  await reveal(page, '#mSize');
};
const same = (a: number[][], b: number[][]) => a.length === b.length && a.every((p, i) => p.every((v, j) => Math.abs(v - b[i][j]) < 1e-4));
const pause = (ms: number) => new Promise(r => setTimeout(r, ms));

test('every Direct slider reaches /api/control-values and the frame; double-click = neutral', async ({ page }) => {
  await square(page);
  const base = await points();
  // [slider, control id, slider value, native value, moves the drawing]
  const cases: [string, string, string, number, boolean][] = [
    ['#mSize', 'master.size', '150', 1.5, true],
    ['#mSizeX', 'master.size_x', '50', 0.5, true],
    ['#mSizeY', 'master.size_y', '-100', -1, true],
    ['#mPosX', 'master.pos_x', '40', 0.4, true],
    ['#mPosY', 'master.pos_y', '-30', -0.3, true],
    ['#mTiltX', 'master.rot_x.angle', '60', 60, true],
    ['#mTiltY', 'master.rot_y.angle', '45', 45, true],
    ['#mTiltZ', 'master.rot_z.angle', '30', 30, true],
    ['#mSpinX', 'master.rot_x.speed', '200', 200, true],
    ['#mSpinY', 'master.rot_y.speed', '200', 200, true],
    ['#mSpinZ', 'master.rot_z.speed', '200', 200, true],
    ['#mSpeed', 'master.speed', '200', 2, false],
    ['#mBright', 'master.brightness', '50', 0.5, true],
  ];
  for (const [sel, id, v, native, moves] of cases) {
    const neutral = (await values())[id] as number;
    await page.locator(sel).fill(v);
    await expect.poll(async () => (await values())[id], id).toBeCloseTo(native, 3);
    expect(await page.locator(sel).getAttribute('data-control')).toBe(id);
    expect(await page.locator(sel).getAttribute('title')).toBe(id);
    if (moves) await expect.poll(async () => same(await points(), base), `${id} moves the frame`).toBe(false);
    else await expect.poll(async () => (await studio.frame()).live.speed, `${id} in the frame`).toBeCloseTo(native, 3);
    await page.locator('label', { has: page.locator(sel + 'V') }).dblclick(); // T-431: neutral from the name
    await expect.poll(async () => (await values())[id], `${id} back to neutral`).toBeCloseTo(neutral, 3);
  }
});

test('right click on « Taille » brings it back to 100 %, also through the MIDI menu', async ({ page }) => {
  await page.locator('#mSize').fill('150');
  await expect.poll(async () => (await live()).size).toBeCloseTo(1.5, 3);
  await page.locator('label', { has: page.locator('#mSizeV') }).click({ button: 'right' });
  await expect.poll(async () => (await live()).size).toBe(1);
  await expect(page.locator('#mSizeV')).toHaveText('100 %');
  await expect(page.locator('#midiMenu')).toBeHidden();

  await page.locator('#mSize').fill('60');
  await expect.poll(async () => (await live()).size).toBeCloseTo(0.6, 3);
  await page.locator('#mSize').click({ button: 'right' });
  await expect(page.locator('#midiMenuNeutral')).toBeVisible();
  await page.locator('#midiMenuNeutral').click();
  await expect.poll(async () => (await live()).size).toBe(1);
  // Buttons have no neutral value: the entry hides.
  await page.locator('#rotReverse').click({ button: 'right' });
  await expect(page.locator('#midiMenuNeutral')).toBeHidden();
});

test('zoom −/+, speed presets ¼…4, Tout réinitialiser', async ({ page }) => {
  await page.locator('#zoomIn').click();
  await expect.poll(async () => (await live()).size).toBeCloseTo(1.1, 3);
  await page.locator('#zoomOut').click();
  await page.locator('#zoomOut').click();
  await expect.poll(async () => (await live()).size).toBeCloseTo(0.9, 3);
  for (const [label, v] of [['¼', 0.25], ['½', 0.5], ['2', 2], ['4', 4], ['1', 1]] as const) {
    await page.locator('#speedPresets button', { hasText: label }).click();
    await expect.poll(async () => (await live()).speed).toBeCloseTo(v, 3);
    await expect(page.locator('#speedPresets button', { hasText: label })).toHaveClass(/active/);
  }
  await page.locator('#speedPresets button', { hasText: '½' }).click();
  await expect(page.locator('#liveReset')).toHaveText('Tout réinitialiser');
  await page.locator('#liveReset').click();
  await expect.poll(async () => (await live()).speed).toBe(1);
  expect((await live()).size).toBe(1);
});

test('position pad: drag sets X and Y together, double-click centres', async ({ page }) => {
  await page.locator('#posPad').scrollIntoViewIfNeeded();
  const box = (await page.locator('#posPad').boundingBox())!;
  // Upper right quarter: +X, +Y.
  await page.mouse.move(box.x + box.width * 0.75, box.y + box.height * 0.25);
  await page.mouse.down();
  await page.mouse.up();
  await expect.poll(async () => (await live()).pos_x).toBeCloseTo(0.5, 1);
  expect((await live()).pos_y).toBeCloseTo(0.5, 1);
  await expect(page.locator('#mPosXV')).toHaveText('50 %');
  await page.locator('#posPad').dblclick({ position: { x: 10, y: 10 } });
  await expect.poll(async () => (await live()).pos_x).toBe(0);
  expect((await live()).pos_y).toBe(0);
});

test('Rapide then Inverser held: the drawing turns the other way while held', async ({ page }) => {
  await square(page);
  await page.locator('#rotPresets button', { hasText: 'Rapide' }).click();
  await expect.poll(async () => (await live()).rot_speed[2]).toBe(270);
  await expect(page.locator('#mSpinZV')).toHaveText('270 °/s');
  // Sign of the turn of the first lit point over ~120 ms.
  const turn = async () => {
    const a = (await points()).find(isLit)!;
    await pause(120);
    const b = (await points()).find(isLit)!;
    let d = Math.atan2(b[1], b[0]) - Math.atan2(a[1], a[0]);
    d = Math.atan2(Math.sin(d), Math.cos(d));
    return Math.sign(d);
  };
  const forward = await turn();
  expect(forward).not.toBe(0);
  await page.locator('#rotReverse').hover();
  await page.mouse.down();
  await expect.poll(async () => (await live()).rot_reverse).toBe(true);
  expect(await turn()).toBe(-forward);
  await page.mouse.up();
  await expect.poll(async () => (await live()).rot_reverse).toBe(false);
  expect(await turn()).toBe(forward);
});

test('Sync tempo shows rotation speeds in turns per bar', async ({ page }) => {
  await page.locator('#rotPresets button', { hasText: 'Moyen' }).click();
  await page.locator('#rotSync').check();
  await expect(page.locator('#mSpinZV')).toHaveText('1 tr/mesure');
  await expect(page.locator('#mSpinZ')).toHaveAttribute('max', '4');
  await page.locator('#rotSync').uncheck();
  await expect(page.locator('#mSpinZV')).toHaveText('90 °/s');
});

test('an external POST /api/control moves the sliders in under 500 ms', async ({ page }) => {
  for (const [id, value, sel, shown] of [
    ['master.size_x', -0.5, '#mSizeXV', '-50 %'],
    ['master.rot_z.angle', 90, '#mTiltZV', '90 °'],
    ['master.rot_y.speed', -180, '#mSpinYV', '-180 °/s'],
    ['master.brightness', 0.3, '#mBrightV', '30 %'],
  ] as const) {
    expect(await studio.post('/api/control', { id, value })).toBe(200);
    await expect(page.locator(sel), id).toHaveText(shown, { timeout: 500 });
  }
  await expect(page.locator('#mBright')).toHaveValue('30', { timeout: 500 });
});

test('Direct panel at 1280×800: no horizontal scroll, Calque/Cue greyed', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await reveal(page, '#mSize');
  const fits = await page.locator('#livePanel').evaluate(el => el.scrollWidth <= el.clientWidth);
  expect(fits).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await expect(page.locator('#directTabs button', { hasText: 'Maître' })).toHaveClass(/active/);
  await expect(page.locator('#directTabs button', { hasText: 'Calque' })).toBeDisabled();
  await expect(page.locator('#directTabs button', { hasText: 'Cue' })).toBeDisabled();
  await page.locator('#posPad').scrollIntoViewIfNeeded();
  await expect(page.locator('#posPad')).toBeVisible();
});
