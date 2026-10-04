// Textures (T-107): the starry sky, lightning and fat beams can be picked in
// the Effet tab, start from their own values in tempo, and draw above the
// audience. Lightning is lit only 2 to 4 frames per beat, so only its
// settings and its position are checked.
import { test, expect, useStudio, openUi, openWorkspace, isLit } from '../studio';

const studio = useStudio();
const settings = async () => (await studio.state()).settings;

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('every texture is selectable and draws above the audience', async ({ page }) => {
  await openWorkspace(page, 'creation');
  await page.locator('[data-kind="generator"]').click();
  const looks: [string, string, number, number, boolean][] = [
    ['Ciel étoilé', 'starfield', 6, 4, true],
    ['Éclairs', 'lightning', 8, 1, false],
    ['Faisceaux épais', 'fat_beams', 5, 1, true],
  ];
  for (const [label, name, count, steps, steady] of looks) {
    await page.locator('#gen').selectOption({ label });
    await expect.poll(async () => (await settings()).content.generator).toBe(name);
    const p = (await settings()).content.params;
    expect([p.beat_sync, p.count, p.steps_per_beat]).toEqual([true, count, steps]);
    if (steady) {
      await expect.poll(async () => (await studio.frame()).points.filter(isLit).length).toBeGreaterThan(0);
    }
    const lit = (await studio.frame()).points.filter(isLit);
    expect(lit.every(q => q[1] >= 0)).toBe(true);
  }
});
