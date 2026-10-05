// LIVE › Effets (T-146): the FX grid played on top of what plays, its
// Action faders, « Arrêter FX », a cell edited and undone, and show mode
// (playing allowed, editing refused). Preview only: nothing is armed.
import { test, expect, useStudio, openUi, reveal, isLit } from '../studio';

const studio = useStudio();
type FxLine = { active: number | null; waiting: boolean; level: number; action: number };
const fx = async () => (await studio.frame() as any).fx as { lines: FxLine[]; dest: string; click: string };
/** How far the lit points' mean x travels over ~2 s. */
async function sway() {
  const xs: number[] = [];
  const until = Date.now() + 2000;
  while (Date.now() < until) {
    const lit = (await studio.frame()).points.filter(isLit);
    if (lit.length) xs.push(lit.reduce((a, p) => a + p[0], 0) / lit.length);
  }
  return Math.max(...xs) - Math.min(...xs);
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'fx.stop' });
  await studio.post('/api/control', { id: 'fx.mode', value: 0 });
  for (let l = 1; l <= 4; l++) await studio.post('/api/control', { id: `fx.${l}.action`, value: 1 });
  await openUi(page, studio);
  await expect.poll(async () => (await studio.frame()).points.filter(isLit).length).toBeGreaterThan(0);
});

test('a click on a cell changes the frame; one effect per line; « Arrêter FX » stops all', async ({ page }) => {
  await reveal(page, '#fgGrid');
  await expect(page.locator('#fgGrid .fg-cell')).toHaveCount(32);
  // « Vague X » moves the figure sideways.
  const vague = page.locator('#fgGrid .fg-cell[data-control="fx.1.4"]');
  await expect(vague).toContainText('Vague X');
  const still = await sway();
  await vague.click();
  await expect.poll(async () => (await fx()).lines[0].active).toBe(3);
  await expect(vague).toHaveClass(/\bon\b/);
  expect(await sway()).toBeGreaterThan(still + 0.1);
  // Another cell of the same line replaces it.
  await page.locator('#fgGrid .fg-cell[data-control="fx.1.1"]').click();
  await expect.poll(async () => (await fx()).lines[0].active).toBe(0);
  await expect(vague).not.toHaveClass(/\bon\b/);
  // A second line adds up.
  await page.locator('#fgGrid .fg-cell[data-control="fx.2.5"]').click();
  await expect.poll(async () => (await fx()).lines.map(l => l.active)).toEqual([0, 4, null, null]);
  // Action of line 2 at 0 (sent like a fader from the page).
  await page.locator('#fgGrid [data-act="1"]').fill('0');
  await expect.poll(async () => (await fx()).lines[1].action).toBe(0);
  await page.locator('#fgStop').click();
  await expect.poll(async () => (await fx()).lines.every(l => l.active === null)).toBe(true);
  expect((await studio.frame()).armed).toBe(false);
});

test('a drop plays once and stops by itself', async ({ page }) => {
  await reveal(page, '#fgGrid');
  // « Choc »: a 2-beat drop (120 BPM: one second).
  await page.locator('#fgGrid .fg-cell[data-control="fx.3.1"]').click();
  await expect.poll(async () => (await fx()).lines[2].active, { timeout: 1000 }).toBe(0);
  await expect.poll(async () => (await fx()).lines[2].active, { timeout: 5000 }).toBe(null);
});

test('Flash: the effect plays while the cell is held', async ({ page }) => {
  await reveal(page, '#fgGrid');
  await page.locator('#fgClick button', { hasText: 'Flash' }).click();
  await expect.poll(async () => (await fx()).click).toBe('flash');
  const cell = page.locator('#fgGrid .fg-cell[data-control="fx.1.2"]');
  await cell.hover();
  await page.mouse.down();
  await expect.poll(async () => (await fx()).lines[0].active).toBe(1);
  await page.mouse.up();
  await expect.poll(async () => (await fx()).lines[0].active).toBe(null);
});

test('a cell edited from « Modifier » is undone; show mode plays but does not edit', async ({ page }) => {
  await reveal(page, '#fgGrid');
  await page.locator('#fgEditMode').click();
  await page.locator('#fgGrid .fg-cell[data-control="fx.4.1"]').click();
  await expect(page.locator('#fgEditor')).toBeVisible();
  expect((await fx()).lines[3].active).toBe(null);
  await page.locator('#fgName').fill('Mon zoom');
  await page.locator('#fgName').press('Enter');
  await page.locator('#fgSize').fill('150');
  await page.locator('#fgSize').dispatchEvent('change');
  await expect.poll(async () => (await studio.get('/api/fx')).cells[3][0]?.modifiers?.size).toBeCloseTo(1.5, 3);
  await expect(page.locator('#fgGrid .fg-cell[data-control="fx.4.1"]')).toContainText('Mon zoom');
  // Undo the size, then the cell itself.
  expect(await studio.post('/api/undo')).toBe(200);
  expect(await studio.post('/api/undo')).toBe(200);
  await expect.poll(async () => (await studio.get('/api/fx')).cells[3][0]).toBe(null);
  await page.locator('#fgEditMode').click();

  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  const cells = (await studio.get('/api/fx')).cells;
  expect(await studio.post('/api/fx', { cells })).toBe(423);
  await expect(page.locator('#liveTabs [data-panel-tab="fx"]')).toBeVisible();
  await expect(page.locator('#fgEditMode')).toBeHidden();
  await page.locator('#fgGrid .fg-cell[data-control="fx.1.1"]').click();
  await expect.poll(async () => (await fx()).lines[0].active).toBe(0);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
});
