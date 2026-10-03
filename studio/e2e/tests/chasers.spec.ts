// Chasers, kick stabs and beam strobes (T-103): a chaser cue lights a
// different beam half a beat later, the Effet tab's « Forme du chaser »
// and the « Rythme » block edit the look, and holding S strobes it.
import { test, expect, useStudio, openUi, openWorkspace, reveal, focusPage, isLit, type Point } from '../studio';

const studio = useStudio();
const settings = async () => (await studio.state()).settings;

/** x of the brightest lit beam (the chase head). */
function head(points: Point[]) {
  const lit = points.filter(isLit);
  const peak = Math.max(...lit.map(p => p[2] + p[3] + p[4]));
  return lit.find(p => p[2] + p[3] + p[4] === peak)![0];
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('a chaser cue lights another beam half a beat later', async ({ page }) => {
  const catalog = await studio.presets();
  const cue = catalog.presets.find(p => p.name === 'Chaser → · vert')!;
  expect(cue.category).toBe('Faisceaux');
  await page.locator('#cueTabs button', { hasText: 'Faisceaux' }).click();
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(cue.id);
  await expect.poll(async () => (await studio.frame()).points.filter(isLit).length).toBeGreaterThan(0);
  // One step per 1/2 beat: half a beat (or a bit more) later, the head
  // has moved to another beam of the same fixed fan.
  const a = await studio.frame();
  await page.waitForTimeout(60_000 / a.tempo.bpm / 2 + 20);
  const b = await studio.frame();
  expect(b.tempo.beat - a.tempo.beat).toBeLessThan(3);
  expect(Math.abs(head(a.points) - head(b.points))).toBeGreaterThan(0.05);
  // Every beam stays above the audience.
  expect(b.points.filter(isLit).every(p => p[1] > 0)).toBe(true);
});

test('the Effet tab picks a chaser and its shape', async ({ page }) => {
  await openWorkspace(page, 'creation');
  await page.locator('[data-kind="generator"]').click();
  await page.locator('#gen').selectOption({ label: 'Chaser' });
  await expect.poll(async () => (await settings()).content.generator).toBe('chase_fan');
  const p = (await settings()).content.params;
  expect([p.beat_sync, p.count, p.b, p.steps_per_beat]).toEqual([true, 8, 2, 2]);
  await expect(page.locator('#gChaseRow')).toBeVisible();
  await page.locator('#gChase').selectOption({ label: 'Centre → bords' });
  await expect.poll(async () => (await settings()).content.params.a).toBe(3);
  await page.locator('#gen').selectOption('rose');
  await expect(page.locator('#gChaseRow')).toBeHidden();
});

test('the Rythme block gates and strobes any look, and S holds a strobe', async ({ page }) => {
  await openWorkspace(page, 'creation');
  await reveal(page, '#rGate');
  await expect(page.locator('#rGateLen')).toBeDisabled();
  await page.locator('#rGate').selectOption('beat');
  await expect.poll(async () => (await settings()).gate).toBe('beat');
  await expect(page.locator('#rGateLen')).toBeEnabled();
  await page.locator('#rGateLen').fill('0.25');
  await expect.poll(async () => (await settings()).gate_beats).toBeCloseTo(0.25, 3);
  await page.locator('#rDecay').check();
  await expect.poll(async () => (await settings()).gate_decay).toBe(true);
  await page.locator('#rStrobe').selectOption('2');
  await expect.poll(async () => (await settings()).strobe_div).toBe(2);
  await page.locator('#rDuty').fill('30');
  await expect.poll(async () => (await settings()).strobe_duty).toBeCloseTo(0.3, 3);
  // A gated look goes dark between beats: some frames are dark.
  let dark = 0;
  for (let i = 0; i < 20 && !dark; i++) {
    if (!(await studio.frame()).points.some(isLit)) dark++;
    await page.waitForTimeout(37);
  }
  expect(dark).toBeGreaterThan(0);

  // Hold S: strobe 1/4 while held, then the look's own strobe again.
  await focusPage(page);
  await page.keyboard.down('s');
  await expect.poll(async () => (await settings()).strobe_div).toBe(4);
  await expect(page.locator('#rStrobe')).toHaveValue('4');
  await page.keyboard.up('s');
  await expect.poll(async () => (await settings()).strobe_div).toBe(2);
});
