// Fixed frame rate for video capture (T-463): RÉGLAGES › Sorties et
// projecteurs › Cadence fixe. Every frame gets the same number of points
// (30 000 points/s at 25 im/s = 1 200), whatever plays; off, the size
// follows the content again. Turning it on is refused armed.
// Preview only: the studio drives the fake output (no --device).
import { test, expect, useStudio, openUi, reveal } from '../studio';

const studio = useStudio();
const LOOKS = [
  { kind: 'shape', shape: 'square' },
  { kind: 'text', text: 'LASER STUDIO CAPTATION VIDEO 2026' },
  { kind: 'shape', shape: 'circle' },
];
const fixedView = async () => (await studio.get('/api/outputs/scan_limits')).fixed;
const points = async () => (await studio.frame()).points.length;

async function play(content: object) {
  expect(await studio.post('/api/settings', { ...(await studio.state()).settings, content, brightness: 1 })).toBe(200);
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/arm', { on: false });
  expect(await studio.post('/api/outputs/fixed_rate', { fps: null })).toBe(200);
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/outputs/fixed_rate', { fps: null });
});

test('25 im/s gives every look the same size, off lets it vary again', async ({ page }) => {
  // Off: three looks of very different sizes.
  const sizes = new Set<number>();
  for (const look of LOOKS) {
    await play(look);
    await expect.poll(points).not.toBe(0);
    await page.waitForTimeout(200);
    sizes.add(await points());
  }
  expect(sizes.size).toBeGreaterThan(1);

  await reveal(page, '#ofrChoice');
  await expect(page.locator('#ofrChoice')).toHaveValue('');
  await page.locator('#ofrChoice').selectOption('25');
  await expect.poll(async () => (await fixedView()).fps).toBe(25);
  await expect(page.locator('#ofrInfo')).toContainText('1 200 points par image');
  await expect(page.locator('#stFps')).toHaveText('25 im/s fixe');

  for (const look of LOOKS) {
    await play(look);
    // A few frames of each: always 1 200 points.
    for (let i = 0; i < 4; i++) {
      await expect.poll(points).toBe(1200);
      await page.waitForTimeout(80);
    }
  }
  const st = (await studio.state()).fixed_rate;
  expect(st.target_fps).toBe(25);
  expect(st.points).toBe(1200);
  expect(st.deviation_pct).toBeLessThan(1);
  await expect(page.locator('#ofrInfo')).toContainText('cadence mesurée 25,00 im/s');

  // Off: the sizes follow the content again.
  await page.locator('#ofrChoice').selectOption('');
  await expect.poll(async () => (await fixedView()).fps).toBeNull();
  await expect.poll(async () => (await studio.state()).fixed_rate).toBeNull();
  await expect(page.locator('#stFps')).not.toContainText('fixe');
  const after = new Set<number>();
  for (const look of LOOKS) {
    await play(look);
    await page.waitForTimeout(200);
    after.add(await points());
  }
  expect(after.size).toBeGreaterThan(1);
});

test('a free value, out of range refused, and armed only turning off applies', async ({ page }) => {
  await reveal(page, '#ofrChoice');
  await page.locator('#ofrChoice').selectOption('other');
  await expect(page.locator('#ofrValue')).toBeEnabled();
  await page.locator('#ofrValue').fill('75');
  await page.locator('#ofrApply').click();
  await expect(page.locator('#ofrError')).toContainText('20 à 60');
  expect((await fixedView()).fps).toBeNull();
  await page.locator('#ofrValue').fill('29.97');
  await page.locator('#ofrApply').click();
  await expect.poll(async () => (await fixedView()).fps).toBeCloseTo(29.97, 2);
  await expect.poll(points).toBe(1001);

  // Armed: changing it is refused (the laser stays armed), off applies.
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await page.locator('#ofrChoice').selectOption('50');
  await expect(page.locator('#ofrError')).toContainText('Désarmez');
  expect((await fixedView()).fps).toBeCloseTo(29.97, 2);
  expect((await studio.state()).armed).toBe(true);
  await page.locator('#ofrChoice').selectOption('');
  await expect.poll(async () => (await fixedView()).fps).toBeNull();
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
});
