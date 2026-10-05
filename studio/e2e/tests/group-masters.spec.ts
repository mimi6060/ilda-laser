// Group masters (T-420): zone groups are made in RÉGLAGES › Zones de
// projection, each gets a fader in LIVE › Calques › Maîtres de groupe that
// only dims its zones, « Tout à 100 % » puts them back. Two test zones
// (left and right halves) show the same look, so their light can be
// compared in one frame. Preview only: the studio stays disarmed.
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';

const studio = useStudio();

interface Group { n: number; name: string; zones: number[]; master: number; control: string }
const groups = async () => (await studio.get<{ groups: Group[] }>('/api/groups')).groups;
const peak = (pts: Point[]) => pts.reduce((m, p) => Math.max(m, p[2], p[3], p[4]), 0);

/** The brightest channel on the left and on the right half, one frame. */
async function halves() {
  const pts = ((await studio.frame()).points as Point[]).filter(isLit);
  return { left: peak(pts.filter(p => p[0] < -0.02)), right: peak(pts.filter(p => p[0] > 0.02)) };
}

const ZONES = {
  zones: [
    { id: 1, name: 'Gauche', geometry: { size_x: 0.5, pos_x: -0.5 } },
    { id: 2, name: 'Droite', geometry: { size_x: 0.5, pos_x: 0.5 } },
  ],
  default_zones: [1, 2],
  groups: [],
};

test.beforeEach(async ({ page }) => {
  await studio.reset();
  expect(await studio.post('/api/projection', ZONES)).toBe(200);
  await openUi(page, studio);
});

test.afterAll(async () => {
  await studio.post('/api/projection', { zones: [{ id: 1, name: 'Principale' }], default_zones: [1] });
});

test('two groups made in the settings, Côtés dimmed to 50 % from LIVE, then Tout à 100 %', async ({ page }) => {
  // No group: the row is hidden.
  await expect(page.locator('#groupMasters')).toBeHidden();
  await expect.poll(async () => { const h = await halves(); return h.left > 0 && Math.abs(h.left - h.right) < 1e-3; }).toBe(true);

  // RÉGLAGES › Zones de projection › Groupes: Centre (zone 1), Côtés (zone 2).
  await reveal(page, '#pzPanel summary');
  await page.locator('#pzPanel summary').click();
  await reveal(page, '#pzGroupAdd');
  await page.locator('#pzGroupAdd').click();
  await expect(page.locator('#pzGroups .pz-group')).toHaveCount(1);
  await page.locator('#pzGroupAdd').click();
  await expect(page.locator('#pzGroups .pz-group')).toHaveCount(2);
  await page.locator('#pzGroupName0').fill('Centre');
  await page.locator('#pzGroupName0').press('Enter');
  await expect.poll(async () => (await groups())[0]?.name).toBe('Centre');
  await page.locator('#pzGroupName1').fill('Côtés');
  await page.locator('#pzGroupName1').press('Enter');
  await expect.poll(async () => (await groups())[1]?.name).toBe('Côtés');
  await page.locator('#pzGroups .pz-group[data-group="1"] input[data-zone="1"]').check();
  await expect.poll(async () => (await groups())[0].zones).toEqual([1]);
  await page.locator('#pzGroups .pz-group[data-group="2"] input[data-zone="2"]').check();
  await expect.poll(async () => (await groups()).map(g => [g.name, g.zones, g.master])).toEqual([['Centre', [1], 1], ['Côtés', [2], 1]]);
  await expect(page.locator('#pzGroups .pz-group[data-group="2"] .gm-level')).toHaveText('Maître : 100 % (LIVE)');

  // LIVE: one fader per group, with its name.
  await reveal(page, '#groupMasters');
  await expect(page.locator('#groupMasters')).toBeVisible();
  await expect(page.locator('#gmRow .layer .t')).toHaveText(['Centre', 'Côtés']);
  const cotes = page.locator('#gmRow .layer[data-group="2"] input[type=range]');
  await expect(cotes).toHaveAttribute('data-control', 'group.2.master');

  // Côtés at 50 %: the right zone dims, the left one doesn't.
  await cotes.fill('50');
  await expect.poll(async () => (await studio.controlValues()).values['group.2.master']).toBe(0.5);
  await expect(page.locator('#gmRow .layer[data-group="2"] .dv')).toHaveText('50 %');
  await expect.poll(async () => { const h = await halves(); return h.left > 0 && h.right > 0 && h.right < h.left * 0.75; }).toBe(true);
  await expect(page.locator('#pzGroups .pz-group[data-group="2"] .gm-level')).toHaveText('Maître : 50 % (LIVE)');

  // « Réinitialiser le direct » leaves the group masters alone.
  expect(await studio.post('/api/control', { id: 'master.reset' })).toBe(200);
  expect((await studio.controlValues()).values['group.2.master']).toBe(0.5);

  // Centre at 0: the left zone dark, the right one still at its level.
  await page.locator('#gmRow .layer[data-group="1"] input[type=range]').fill('0');
  await expect.poll(async () => { const h = await halves(); return h.left === 0 && h.right > 0; }).toBe(true);
  await expect(page.locator('#gmRow .layer[data-group="1"]')).toHaveClass(/off/);

  // « Tout à 100 % »: both halves alike again.
  await page.locator('#gmAllFull').click();
  await expect.poll(async () => (await groups()).map(g => g.master)).toEqual([1, 1]);
  await expect.poll(async () => { const h = await halves(); return h.left > 0 && Math.abs(h.left - h.right) < 1e-3; }).toBe(true);
  await expect(cotes).toHaveValue('100');

  // A master can't go above 100 %, and nothing armed.
  expect(await studio.post('/api/control', { id: 'group.2.master', value: 5 })).toBe(200);
  expect((await groups())[1].master).toBe(1);
  expect((await studio.state()).armed).toBe(false);
});

test('a group deleted in the settings takes its fader away', async ({ page }) => {
  const two = { ...ZONES, groups: [{ name: 'Centre', zones: [1] }, { name: 'Côtés', zones: [2] }] };
  expect(await studio.post('/api/projection', two)).toBe(200);
  expect(await studio.post('/api/control', { id: 'group.2.master', value: 0.4 })).toBe(200);
  await reveal(page, '#groupMasters');
  await expect(page.locator('#gmRow .layer')).toHaveCount(2);
  await expect(page.locator('#gmRow .layer[data-group="2"] .dv')).toHaveText('40 %');

  await reveal(page, '#pzPanel summary');
  await page.locator('#pzPanel summary').click();
  await reveal(page, '#pzGroups .pz-group[data-group="1"] .del');
  await page.locator('#pzGroups .pz-group[data-group="1"] .del').click();
  // Côtés is now group 1 and keeps its level.
  await expect.poll(async () => (await groups()).map(g => [g.name, g.master])).toEqual([['Côtés', 0.4]]);
  await expect(page.locator('#gmRow .layer')).toHaveCount(1);
  await expect(page.locator('#gmRow .layer[data-group="1"] .t')).toHaveText('Côtés');

  // The last one deleted: the row hides.
  await page.locator('#pzGroups .pz-group[data-group="1"] .del').click();
  await expect.poll(async () => (await groups()).length).toBe(0);
  await expect(page.locator('#groupMasters')).toBeHidden();
  expect((await studio.state()).armed).toBe(false);
});
