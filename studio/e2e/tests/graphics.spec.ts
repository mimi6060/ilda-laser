// Graphics (T-108): the countdown and the 3D objects can be picked in the
// Effet tab and start from their own values in tempo; the object is chosen
// from its own list. The countdown goes black once it has counted down, so
// only its settings are checked.
import { test, expect, useStudio, openUi, openWorkspace, isLit } from '../studio';

const studio = useStudio();
const settings = async () => (await studio.state()).settings;

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('the countdown and the 3D objects are selectable with their own values', async ({ page }) => {
  await openWorkspace(page, 'creation');
  await page.locator('[data-kind="generator"]').click();

  await page.locator('#gen').selectOption({ label: 'Compte à rebours' });
  await expect.poll(async () => (await settings()).content.generator).toBe('countdown');
  let p = (await settings()).content.params;
  expect([p.beat_sync, p.count, p.steps_per_beat]).toEqual([true, 4, 0.25]);
  await expect(page.locator('#gSteps')).toHaveValue('0.25');
  await expect(page.locator('#gSolidRow')).toBeHidden();

  await page.locator('#gen').selectOption({ label: 'Objet 3D' });
  await expect.poll(async () => (await settings()).content.generator).toBe('wire3d');
  p = (await settings()).content.params;
  expect([p.beat_sync, p.a, p.period_beats]).toEqual([true, 0, 8]);
  await expect(page.locator('#gSolidRow')).toBeVisible();
  await page.locator('#gSolid').selectOption({ label: 'Icosaèdre' });
  await expect.poll(async () => (await settings()).content.params.a).toBe(2);
  await expect.poll(async () => (await studio.frame()).points.filter(isLit).length).toBeGreaterThan(0);
  const pts = (await studio.frame()).points;
  expect(pts.every(q => Math.abs(q[0]) <= 1 && Math.abs(q[1]) <= 1)).toBe(true);
});
