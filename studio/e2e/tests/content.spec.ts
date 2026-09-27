// Content and appearance: shapes, text, effects and the look sliders
// change what the studio renders (/api/state and /api/frame).
import { test, expect, useStudio, openUi, extent, isLit, type Point } from '../studio';

const studio = useStudio();
const settings = async () => (await studio.state()).settings;
const points = async () => (await studio.frame()).points;
/** Largest distance from the centre among lit points. */
const radius = (pts: Point[]) => Math.max(0, ...pts.filter(isLit).map(p => Math.hypot(p[0], p[1])));

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('starts on a green circle, drawn in the preview', async ({ page }) => {
  const s = await settings();
  expect(s.content).toEqual({ kind: 'shape', shape: 'circle' });
  await expect(page.locator('#shapes button[data-shape="circle"]')).toHaveClass(/active/);
  const pts = await points();
  expect(pts.filter(isLit).length).toBeGreaterThan(10);
  // A circle: every lit point sits at the same distance from the centre.
  const r = pts.filter(isLit).map(p => Math.hypot(p[0], p[1]));
  expect(Math.max(...r) - Math.min(...r)).toBeLessThan(0.02);
  await expect(page.locator('#stPoints')).not.toHaveText('0');
});

test('clicking a shape changes the state and the frame', async ({ page }) => {
  await page.getByRole('button', { name: 'Carré', exact: true }).click();
  await expect.poll(async () => (await settings()).content).toEqual({ kind: 'shape', shape: 'square' });
  await expect(page.locator('#shapes button[data-shape="square"]')).toHaveClass(/active/);
  await expect(page.locator('#shapes button[data-shape="circle"]')).not.toHaveClass(/active/);
  // The square's corners stick out further than the circle did (0.5).
  await expect.poll(async () => radius(await points())).toBeGreaterThan(0.65);

  await page.getByRole('button', { name: 'Ligne', exact: true }).click();
  await expect.poll(async () => (await settings()).content.shape).toBe('line');
});

test('text content follows what is typed', async ({ page }) => {
  await page.locator('[data-kind="text"]').click();
  await expect(page.locator('#textPanel')).toBeVisible();
  await expect(page.locator('#shapePanel')).toBeHidden();
  await expect.poll(async () => (await settings()).content).toEqual({ kind: 'text', text: 'HELLO' });
  const hello = (await points()).length;

  await page.locator('#text').fill('LASER STUDIO');
  await expect.poll(async () => (await settings()).content.text).toBe('LASER STUDIO');
  // More letters, more points.
  await expect.poll(async () => (await points()).length).toBeGreaterThan(hello);
});

test('the Effet tab plays a generator and its selector switches it', async ({ page }) => {
  await page.locator('[data-kind="generator"]').click();
  await expect(page.locator('#genPanel')).toBeVisible();
  await expect.poll(async () => (await settings()).content.kind).toBe('generator');
  await page.locator('#gen').selectOption('rose');
  await expect.poll(async () => (await settings()).content.generator).toBe('rose');
  await page.locator('#gCount').fill('5');
  await expect.poll(async () => (await settings()).content.params.count).toBe(5);
  await expect(page.locator('#gCountV')).toHaveText('5');
  expect((await points()).filter(isLit).length).toBeGreaterThan(10);
});

test('look size slider scales the drawing', async ({ page }) => {
  expect(extent(await points())).toBeCloseTo(0.5, 1);
  await page.locator('#scale').fill('100');
  await expect(page.locator('#scaleV')).toHaveText('100 %');
  await expect.poll(async () => (await settings()).scale).toBeCloseTo(1.0, 3);
  await expect.poll(async () => extent(await points())).toBeGreaterThan(0.95);

  await page.locator('#scale').fill('20');
  await expect.poll(async () => extent(await points())).toBeLessThan(0.25);
});

test('look brightness slider dims the frame', async ({ page }) => {
  await page.locator('#bright').fill('20');
  await expect(page.locator('#brightV')).toHaveText('20 %');
  await expect.poll(async () => (await settings()).brightness).toBeCloseTo(0.2, 3);
  await expect.poll(async () => Math.max(...(await points()).map(p => Math.max(p[2], p[3], p[4]))))
    .toBeLessThanOrEqual(0.2 + 1e-3);
});

test('colour picker changes the colour of the points', async ({ page }) => {
  await page.locator('#color').fill('#ff0000');
  await expect.poll(async () => (await settings()).color).toEqual([255, 0, 0]);
  await expect.poll(async () => {
    const lit = (await points()).filter(isLit);
    return lit.length > 0 && lit.every(p => p[2] > 0 && p[3] === 0 && p[4] === 0);
  }).toBe(true);
});

test('look rotation slider sets the rotation speed', async ({ page }) => {
  await page.getByRole('button', { name: 'Carré', exact: true }).click();
  await page.locator('#rot').fill('180');
  await expect(page.locator('#rotV')).toHaveText('180 °/s');
  await expect.poll(async () => (await settings()).rotation_speed).toBe(180);
  // A spinning square: its first point moves between two frames.
  const a = (await points()).find(isLit)!;
  await expect.poll(async () => {
    const b = (await points()).find(isLit)!;
    return Math.hypot(a[0] - b[0], a[1] - b[1]);
  }).toBeGreaterThan(0.01);
});
