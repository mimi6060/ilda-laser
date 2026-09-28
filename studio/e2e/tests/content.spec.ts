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

test('« Éventail balayé » sweeps a beam fan with the tempo, above the audience', async ({ page }) => {
  await page.locator('[data-kind="generator"]').click();
  await page.locator('#gen').selectOption({ label: 'Éventail balayé' });
  await expect.poll(async () => (await settings()).content.generator).toBe('fan_sweep');
  // Picking a fan starts it in tempo from its own starting values.
  const p = (await settings()).content.params;
  expect(p.beat_sync).toBe(true);
  expect(p.a).toBeCloseTo(0.35, 3);
  expect(p.count).toBe(8);
  await expect(page.locator('#gSyncPanel')).toBeVisible();
  await expect(page.locator('#gAH')).toContainText('Amplitude');
  await page.locator('#gEasing').selectOption('trapezoid');
  await expect.poll(async () => (await settings()).content.params.easing).toBe('trapezoid');

  const lit = async () => (await points()).filter(isLit);
  await expect.poll(async () => (await lit()).length).toBeGreaterThan(8 * 10);
  expect((await lit()).every(q => q[1] >= -1e-6)).toBe(true);
  // The fan moves between two reads (the centre of its beams shifts).
  const centre = (pts: Point[]) => pts.reduce((s, q) => s + q[0], 0) / pts.length;
  const first = centre(await lit());
  await expect.poll(async () => Math.abs(centre(await lit()) - first), { timeout: 5000 }).toBeGreaterThan(0.02);
});

test('« Tunnel de faisceaux » draws a turning cone of beams on a true circle', async ({ page }) => {
  await page.locator('[data-kind="generator"]').click();
  await page.locator('#gen').selectOption({ label: 'Tunnel de faisceaux' });
  await expect.poll(async () => (await settings()).content.generator).toBe('finger_tunnel');
  // Picking it starts it in tempo, at the medium speed (1/16 turn per beat).
  const p = (await settings()).content.params;
  expect(p.beat_sync).toBe(true);
  expect(p.a).toBeCloseTo(1 / 16, 6);
  expect(p.count).toBe(12);
  await expect(page.locator('#gTurnsRow')).toBeVisible();
  await expect(page.locator('#gTurns')).toHaveValue('0.0625');
  await expect(page.locator('#gSnapRow')).toBeHidden();
  await expect(page.locator('#gAH')).toContainText('Tours par temps');
  await page.locator('#gTurns').selectOption({ label: 'Rapide (1/4)' });
  await expect.poll(async () => (await settings()).content.params.a).toBe(0.25);
  await expect(page.locator('#gAV')).toHaveText('0.250 tour/temps');

  // 12 beams, all at the look size (0.5) from the tunnel's centre (0, 0.5).
  const lit = async () => (await points()).filter(isLit);
  const beams = (pts: Point[]) => new Set(pts.map(q => `${q[0].toFixed(3)},${q[1].toFixed(3)}`)).size;
  await expect.poll(async () => beams(await lit())).toBe(12);
  const pts = await lit();
  for (const q of pts) expect(Math.abs(Math.hypot(q[0], q[1] - 0.5) - 0.5)).toBeLessThan(0.01);
  // A quarter turn per beat: the first beam moves between two reads.
  const first = pts[0];
  await expect.poll(async () => {
    const q = (await lit())[0];
    return Math.hypot(q[0] - first[0], q[1] - first[1]);
  }, { timeout: 5000 }).toBeGreaterThan(0.02);
});

test('« Soleil levant » only lights rays above the horizon; polygon tunnel can snap', async ({ page }) => {
  await page.locator('[data-kind="generator"]').click();
  await page.locator('#gen').selectOption({ label: 'Soleil levant' });
  await expect.poll(async () => (await settings()).content.generator).toBe('sunburst');
  await expect.poll(async () => (await points()).filter(isLit).length).toBeGreaterThan(12 * 10);
  expect((await points()).filter(isLit).every(q => q[1] > 0)).toBe(true);

  await page.locator('#gen').selectOption('polygon_tunnel');
  await expect(page.locator('#gSnapRow')).toBeVisible();
  await expect(page.locator('#gTurnsRow')).toBeHidden();
  await page.locator('#gSnap').check();
  await expect.poll(async () => (await settings()).content.params.snap).toBe(true);
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
