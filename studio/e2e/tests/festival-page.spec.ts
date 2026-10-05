// « Festival » cue page (T-110): its own tab after the older pages, 40
// cues tagged with their moment of a track (a coloured dot), and clicking
// cues plays them in tempo, above the audience. Then « Festival
// évolutifs » (T-112 to T-123).
import { test, expect, useStudio, openUi, isLit } from '../studio';

const studio = useStudio();

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('the Festival page plays three cues, each tagged and above the audience', async ({ page }) => {
  const catalog = await studio.presets();
  const festival = catalog.presets.filter(p => p.category === 'Festival');
  expect(festival.length).toBeGreaterThanOrEqual(40);
  expect(new Set(festival.map(p => p.name)).size).toBe(festival.length);
  expect(festival.every(p => p.tag)).toBe(true);

  await page.locator('#cueTabs button .pn', { hasText: /^Festival \(/ }).click();
  await expect(page.locator('#cues .cue')).toHaveCount(40);
  // Every cell shows its moment as a dot, with its name in French.
  await expect(page.locator('#cues .cue .ftag')).toHaveCount(40);
  await expect(page.locator('#cues .cue .ftag[data-tag="drop"]').first()).toHaveAttribute('title', 'Drop');
  await expect(page.locator('#cues .cue .ftag[data-tag="build"]').first()).toHaveAttribute('title', 'Montée');

  for (const name of ['Intro – éventail blanc', 'Break – plafond bleu', 'Montée – tunnel qui se ferme']) {
    const cue = festival.find(p => p.name === name)!;
    await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
    await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(cue.id);
    const st = await studio.state();
    expect(st.settings.content.kind).toBe('generator');
    expect(st.settings.content.params.beat_sync).toBe(true);
    // Once the engine has drawn it (the previous look may still be in the
    // frame right after the click), every lit point is above the audience.
    await expect.poll(async () => {
      const lit = (await studio.frame()).points.filter(isLit);
      return lit.length > 0 ? Math.min(...lit.map(q => q[1])) : -1;
    }, { message: name }).toBeGreaterThanOrEqual(0);
  }
});

// « Festival évolutifs » (T-112 to T-123): the next tab, twelve cues that
// evolve by themselves, each tagged and saying its length in beats.
test('the evolving page plays a cue that evolves over its beats, above the audience', async ({ page }) => {
  const catalog = await studio.presets();
  const evolving = catalog.presets.filter(p => p.category === 'Festival évolutifs');
  expect(evolving.length).toBe(12);
  expect(evolving.every(p => p.tag && / · (16|32) temps$/.test(p.name))).toBe(true);

  await page.locator('#cueTabs button .pn', { hasText: /^Festival évolutifs \(/ }).click();
  await expect(page.locator('#cues .cue')).toHaveCount(12);
  await expect(page.locator('#cues .cue .ftag')).toHaveCount(12);

  const cue = evolving.find(p => p.name.startsWith('Montée – chaser qui accélère'))!;
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(cue.id);
  expect((await studio.state()).settings.content.kind).toBe('evolving');
  // The engine plays it from its launch, over its 32 beats.
  type Progress = { length: number };
  await expect.poll(async () => ((await studio.frame()) as unknown as { evolving: Progress[] }).evolving[0]?.length).toBe(32);
  await expect.poll(async () => {
    const lit = (await studio.frame()).points.filter(isLit);
    return lit.length > 0 ? Math.min(...lit.map(q => q[1])) : -1;
  }).toBeGreaterThanOrEqual(0);
});
