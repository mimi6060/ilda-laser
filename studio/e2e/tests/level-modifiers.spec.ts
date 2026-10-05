// T-144: the « Direct » tabs Calque and Cue drive the same controls on one
// layer or on one cue on show; « Enregistrer dans le cue » keeps a cue's
// own modifiers for its next start; « Lissage » glides a jump.
// Preview only: nothing here arms the laser.
import { test, expect, useStudio, openUi, reveal, centroid } from '../studio';

const studio = useStudio();
type Mods = { size: number; pos_x: number; [k: string]: unknown };
type LevelFrame = {
  armed: boolean;
  points: [number, number, number, number, number][];
  cues: { active: { cue: string }[] };
  cue_mods: Record<string, Mods>;
  layers: { mixer: { layers: { modifiers: Mods }[]; smoothing: { enabled: boolean; mass: number } } };
};
const frame = () => studio.get<LevelFrame>('/api/frame');

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  for (const n of [1, 2, 3, 4]) expect(await studio.post('/api/control', { id: `layer.${n}.reset` })).toBe(200);
  await studio.post('/api/control', { id: 'smoothing.on', value: 0 });
  await studio.post('/api/control', { id: 'smoothing.inertia', value: 1 });
  await openUi(page, studio);
});

test('a cue level size, stored in the cue, comes back at its next start', async ({ page }) => {
  const cell = page.locator('#cues .cue').first();
  const id = (await cell.getAttribute('data-id'))!;
  await reveal(page, '#directTabs');
  await page.locator('#directTabs [data-scope="cue"]').click();
  await expect(page.locator('#directScopeHint')).toContainText('Aucun cue en cours');
  await cell.click();
  await expect.poll(async () => (await frame()).cues.active.map(c => c.cue)).toContain(id);
  await expect(page.locator('#directCue')).toHaveValue(id);

  await page.locator('#mSize').fill('50');
  await expect.poll(async () => (await frame()).cue_mods[id]?.size).toBeCloseTo(0.5, 3);
  expect((await studio.live()).size).toBe(1);
  await expect(page.locator('#directTabs [data-scope="cue"]')).toHaveClass(/lvl-on/);
  await page.locator('#directStore').click();
  await expect(page.locator('#directScopeHint')).toContainText('Enregistré');

  // A tweak not stored is forgotten at the next start.
  await page.locator('#mSize').fill('150');
  await expect.poll(async () => (await frame()).cue_mods[id]?.size).toBeCloseTo(1.5, 3);
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await expect.poll(async () => (await frame()).cues.active.length).toBe(0);
  await cell.click();
  await expect.poll(async () => (await frame()).cue_mods[id]?.size).toBeCloseTo(0.5, 3);
  await expect(page.locator('#mSizeV')).toHaveText('50 %');
  // The master tab shows the master again.
  await page.locator('#directTabs [data-scope="master"]').click();
  await expect(page.locator('#mSizeV')).toHaveText('100 %');
  expect((await frame()).armed).toBe(false);
});

test('the Calque tab moves one layer only', async ({ page }) => {
  await reveal(page, '#directTabs');
  await page.locator('#directTabs [data-scope="layer"]').click();
  await page.locator('#directLayer').selectOption('2');
  await page.locator('#mPosX').fill('40');
  await expect.poll(async () => (await frame()).layers.mixer.layers[1].modifiers.pos_x).toBeCloseTo(0.4, 3);
  expect((await frame()).layers.mixer.layers[0].modifiers.pos_x).toBe(0);
  expect((await studio.live()).pos_x).toBe(0);
  await expect(page.locator('#directTabs [data-scope="layer"]')).toHaveClass(/lvl-on/);

  // Layer 1 holds the manual look: it moves.
  await page.locator('#directLayer').selectOption('1');
  await expect(page.locator('#mPosXV')).toHaveText('0 %');
  await page.locator('#mPosX').fill('40');
  await expect.poll(async () => centroid((await frame()).points).x).toBeCloseTo(0.4, 1);
  await page.locator('#directTabs [data-scope="master"]').click();
  await expect(page.locator('#mPosXV')).toHaveText('0 %');
  expect((await frame()).armed).toBe(false);
});

test('Lissage glides a jump instead of jumping', async ({ page }) => {
  await reveal(page, '#smoothOn');
  await page.locator('#smoothOn').check();
  await expect.poll(async () => (await frame()).layers.mixer.smoothing.enabled).toBe(true);
  await page.locator('#smoothInertia').fill('400');
  await expect.poll(async () => (await frame()).layers.mixer.smoothing.mass).toBeCloseTo(4, 2);
  await expect(page.locator('#smoothInertiaV')).toHaveText('× 4.00');

  await page.locator('#mPosX').fill('40');
  await expect.poll(async () => (await studio.live()).pos_x).toBeCloseTo(0.4, 3);
  // The stored value jumped; the drawing is still on its way.
  expect(centroid((await frame()).points).x).toBeLessThan(0.35);
  await expect.poll(async () => centroid((await frame()).points).x).toBeCloseTo(0.4, 1);
});
