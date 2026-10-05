// Laser text (T-439): single-stroke fonts, accents, text options.
import { test, expect, useStudio, openUi, openWorkspace, isLit, centroid } from '../studio';

const studio = useStudio();
const settings = async () => (await studio.state()).settings;
const lit = async () => (await studio.frame()).points.filter(isLit);

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
});

test('Relief draws « Été » with its accent, and the segment font warns', async ({ page }) => {
  await page.locator('[data-kind="text"]').click();
  await expect(page.locator('#textPanel')).toBeVisible();
  await page.locator('#text').fill('Été');
  await expect.poll(async () => (await settings()).content.text).toBe('Été');
  // The segment font has no accents: the panel says so.
  await expect(page.locator('#textAccentWarn')).toBeVisible();

  await page.locator('#textFont').selectOption('relief');
  await expect.poll(async () => (await settings()).content.style?.font).toBe('relief');
  await expect(page.locator('#textAccentWarn')).toBeHidden();
  await expect.poll(async () => (await lit()).length).toBeGreaterThan(20);
  const pts = await lit();
  expect(pts.every(p => Math.abs(p[0]) <= 1 && Math.abs(p[1]) <= 1)).toBe(true);
});

test('alignment, spacing and fit change the drawing', async ({ page }) => {
  await studio.post('/api/settings', { ...(await settings()), content: { kind: 'text', text: 'Bonjour', style: { font: 'hershey-simple' } } });
  await expect(page.locator('#textFont')).toHaveValue('hershey-simple');
  await page.locator('[data-talign="left"]').click();
  await expect.poll(async () => (await settings()).content.style.align).toBe('left');
  // Left: the first lit point is on the left edge of the frame.
  await expect.poll(async () => Math.min(...(await lit()).map(p => p[0]))).toBeLessThan(-0.98);

  await page.locator('#textFit').check();
  await expect.poll(async () => (await settings()).content.style.fit_width).toBeCloseTo(1.6, 3);
  await expect(page.locator('#textFitW')).toBeEnabled();
  await page.locator('#textFitW').fill('25');
  await expect.poll(async () => (await settings()).content.style.fit_width).toBeCloseTo(0.5, 3);
  await expect.poll(async () => {
    const xs = (await lit()).map(p => p[0]);
    return Math.max(...xs) - Math.min(...xs);
  }).toBeLessThanOrEqual(0.5 + 1e-3);

  await page.locator('#textSpacing').fill('1');
  await expect.poll(async () => (await settings()).content.style.spacing).toBe(1);
});

test('a text posted to the API is drawn on the next frame', async () => {
  const look = await settings();
  expect(await studio.post('/api/settings', { ...look, content: { kind: 'text', text: 'ÇA', style: { font: 'hershey-double' } } })).toBe(200);
  // Laid out at once: within a couple of 60 fps frames.
  await expect.poll(async () => (await lit()).length, { timeout: 500, intervals: [20] }).toBeGreaterThan(20);
});

test('scrolling moves the text with the tempo clock', async ({ page }) => {
  const look = await settings();
  await studio.post('/api/settings', {
    ...look, scale: 0.15,
    content: { kind: 'text', text: 'BONJOUR', style: { font: 'relief', scroll: { enabled: true, speed: 0.25, per: 'beat' } } },
  });
  await expect(page.locator('#textScroll')).toBeChecked();
  await expect(page.locator('#textSpeed')).toBeEnabled();
  const first = centroid(await lit()).x;
  await expect.poll(async () => Math.abs(centroid(await lit()).x - first)).toBeGreaterThan(0.05);
  // Nothing lit outside the frame.
  expect((await lit()).every(p => Math.abs(p[0]) <= 1)).toBe(true);
  // The unit control writes the style.
  await page.locator('#textPer').selectOption('second');
  await expect.poll(async () => (await settings()).content.style.scroll.per).toBe('second');
});

test('the figure editor text tool lists the fonts', async ({ page }) => {
  const fonts = await studio.get<{ id: string }[]>('/api/fonts');
  expect(fonts.map(f => f.id)).toEqual(['segments', 'hershey-simple', 'hershey-double', 'relief']);
  await page.locator('#creationTabs [data-ctab="figures"]').click();
  await expect(page.locator('#figTextFont option')).toHaveCount(4);
});
