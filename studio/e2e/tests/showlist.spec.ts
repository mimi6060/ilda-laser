// Lecteur de shows (T-444): a list of saved shows that chain by
// themselves, from TIMELINE. Two 2 s shows: Lancer plays the first, the
// second follows within a frame (the next one is prepared ahead), the
// frame follows the second show, nothing is armed; Page suivante /
// précédente move along the list; the list survives a reload; in show
// mode it can be played but not edited; Échap stops it and nothing starts
// again after the reset. Preview only.
import { test, expect, useStudio, openUi, openWorkspace, focusPage, isLit, type Point } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface ShowListState {
  transport: 'stopped' | 'playing' | 'waiting' | 'suspended'; index: number | null; elapsed_s: number; remaining_s: number;
  list_remaining_s: number; next_ready: boolean; preparing: boolean; problem: string | null; gap_ms: number | null;
}
interface Frame { points: Point[]; armed: boolean; timeline: { name: string | null; playing: boolean; active: number[] }; showlist: ShowListState }
const frame = () => studio.get<Frame>('/api/frame');
const state = async () => (await studio.get<{ showlist: ShowListState; armed: boolean }>('/api/state'));

function show(name: string, len: number, cue: string) {
  return {
    name, time_base: 'seconds', tempo_map: [{ at_s: 0, bpm: 120, beats_per_bar: 4 }],
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len, source: { kind: 'cue', id: cue } }] }],
  };
}

test.beforeAll(async () => {
  const catalog = await studio.presets();
  expect(await studio.post('/api/shows', show('Court A', 2, catalog.presets[0].id))).toBe(200);
  expect(await studio.post('/api/shows', show('Court B', 2, catalog.presets[1].id))).toBe(200);
  expect(await studio.post('/api/shows', show('Long C', 60, catalog.presets[2].id))).toBe(200);
  expect(await studio.post('/api/shows', show('Long D', 60, catalog.presets[3].id))).toBe(200);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset');
  await studio.reset();
  await studio.post('/api/showlist/stop');
  await studio.post('/api/timeline/stop');
  await studio.post('/api/showlist/loop', { on: false });
  expect(await studio.post('/api/showlist', { items: [] })).toBe(200);
  await openUi(page, studio);
});

/** TIMELINE › Lecteur de shows: add a saved show from the menu. */
async function addShow(page: Page, name: string) {
  const add = page.locator('#slAdd');
  await add.focus();
  await expect(add.locator('option', { hasText: name })).toHaveCount(1);
  await add.selectOption(name);
  await expect(page.locator('#slList li .sl-name', { hasText: name }).first()).toBeVisible();
}

test('two short shows chain by themselves, the frame follows the second, nothing is armed; the list survives a reload', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await addShow(page, 'Court A');
  await addShow(page, 'Court B');
  await expect(page.locator('#slList li')).toHaveCount(2);
  await expect(page.locator('#slList li').nth(0).locator('.sl-dur')).toHaveText('00:02');

  await page.locator('#slPlay').click();
  await expect.poll(async () => (await state()).showlist.index).toBe(0);
  expect((await state()).showlist.transport).toBe('playing');
  await expect.poll(async () => (await frame()).showlist.next_ready).toBe(true);
  await expect(page.locator('#slNow')).toContainText('1/2 — Court A');
  await expect(page.locator('#slState')).toContainText('Prêt');
  const first = await frame();
  expect(first.timeline.name).toBe('Court A');
  expect(first.points.filter(isLit).length).toBeGreaterThan(0);

  // The second one follows by itself.
  await expect.poll(async () => (await state()).showlist.index, { timeout: 5_000 }).toBe(1);
  const f = await frame();
  expect(f.timeline.name).toBe('Court B');
  expect(f.timeline.playing).toBe(true);
  expect(f.timeline.active).toEqual([1]);
  expect(f.points.filter(isLit).length).toBeGreaterThan(0);
  expect(f.armed).toBe(false);
  // Measured on the engine's clock: less than 100 ms between the two shows.
  expect(f.showlist.gap_ms).not.toBeNull();
  expect(f.showlist.gap_ms!).toBeLessThan(100);
  await expect(page.locator('#slList li').nth(1)).toHaveClass(/current/);

  // At the end of the last one, the list stops.
  await expect.poll(async () => (await state()).showlist.transport, { timeout: 5_000 }).toBe('stopped');
  expect((await state()).armed).toBe(false);

  await page.reload();
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#slList li')).toHaveCount(2);
  await expect(page.locator('#slList li .sl-name').nth(1)).toContainText('Court B');
});

test('Page suivante and Page précédente move along the list in TIMELINE; a show launched by hand suspends it', async ({ page }) => {
  expect(await studio.post('/api/showlist', { items: [{ show: 'Long C' }, { show: 'Long D' }] })).toBe(200);
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#slList li')).toHaveCount(2);
  await page.locator('#slPlay').click();
  await expect.poll(async () => (await state()).showlist.index).toBe(0);
  await focusPage(page);
  await page.keyboard.press('PageDown');
  await expect.poll(async () => (await frame()).timeline.name).toBe('Long D');
  expect((await state()).showlist.index).toBe(1);
  await page.keyboard.press('PageUp');
  await expect.poll(async () => (await frame()).timeline.name).toBe('Long C');
  // The remaining time of the list, to the second.
  const st = (await state()).showlist;
  expect(st.list_remaining_s).toBeGreaterThan(115);
  expect(st.list_remaining_s).toBeLessThanOrEqual(120);
  await expect(page.locator('#slNow')).toContainText('liste : 0');

  // Another show from the Show menu: the list is suspended.
  await page.locator('#tlShow').focus();
  await page.locator('#tlShow').selectOption('Court A');
  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await state()).showlist.transport).toBe('suspended');
  await expect(page.locator('#slResume')).toBeVisible();
  await page.locator('#slResume').click();
  await expect.poll(async () => (await state()).showlist.transport).toBe('playing');
  expect((await frame()).timeline.name).toBe('Long C');
  expect((await state()).armed).toBe(false);
});

test('show mode locks the list but plays it; Échap stops it and nothing starts again after the reset', async ({ page }) => {
  expect(await studio.post('/api/showlist', { items: [{ show: 'Long C' }, { show: 'Long D' }] })).toBe(200);
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  expect(await studio.post('/api/showlist', { items: [] })).toBe(423);
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#slList li')).toHaveCount(2);
  await page.locator('#slPlay').click();
  await expect.poll(async () => (await state()).showlist.transport).toBe('playing');
  await page.locator('#slNext').click();
  await expect.poll(async () => (await state()).showlist.index).toBe(1);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);

  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await state()).showlist.transport).toBe('stopped');
  await expect.poll(async () => (await frame()).timeline.playing).toBe(false);
  await expect(page.locator('#slMsg')).toContainText("Arrêt d'urgence");
  expect(await studio.post('/api/showlist/play')).toBe(409);
  await studio.post('/api/estop/reset');
  await page.waitForTimeout(500);
  expect((await state()).showlist.transport).toBe('stopped');
  expect((await frame()).timeline.playing).toBe(false);
  expect((await state()).armed).toBe(false);
});
