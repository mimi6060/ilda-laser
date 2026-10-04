// Crossings, hot beam and convergences (T-104): each new generator can be
// picked in the Effet tab, starts from its own values in tempo, draws lit
// beams above the audience, and the scissors' white flash is a checkbox.
import { test, expect, useStudio, openUi, openWorkspace, isLit } from '../studio';

const studio = useStudio();
const settings = async () => (await studio.state()).settings;

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('every crossing generator is selectable and draws beams above the audience', async ({ page }) => {
  await openWorkspace(page, 'creation');
  await page.locator('[data-kind="generator"]').click();
  const looks: [string, string, number, number][] = [
    ['Ciseaux (X)', 'scissor', 8, 2],
    ['Faisceau chaud', 'hot_beam', 1, 16],
    ['Faisceaux qui s’ouvrent', 'bundle_burst', 10, 4],
    ['V de faisceaux', 'beam_v', 10, 4],
  ];
  for (const [label, name, count, period] of looks) {
    await page.locator('#gen').selectOption({ label });
    await expect.poll(async () => (await settings()).content.generator).toBe(name);
    const p = (await settings()).content.params;
    expect([p.beat_sync, p.count, p.period_beats]).toEqual([true, count, period]);
    await expect.poll(async () => (await studio.frame()).points.filter(isLit).length).toBeGreaterThan(0);
    const lit = (await studio.frame()).points.filter(isLit);
    expect(lit.every(q => q[1] >= 0)).toBe(true);
  }
});

test('the scissors flash white on the crossing when asked', async ({ page }) => {
  await openWorkspace(page, 'creation');
  await page.locator('[data-kind="generator"]').click();
  await page.locator('#gen').selectOption({ label: 'Ciseaux (X)' });
  await expect(page.locator('#gCrossRow')).toBeVisible();
  await expect.poll(async () => (await settings()).content.generator).toBe('scissor');
  expect((await settings()).content.params.cross_flash).toBeFalsy();
  await page.locator('#gCrossFlash').check();
  await expect.poll(async () => (await settings()).content.params.cross_flash).toBe(true);
  await page.locator('#gen').selectOption({ label: 'V de faisceaux' });
  await expect(page.locator('#gCrossRow')).toBeHidden();
});
