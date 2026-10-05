// Notes, tags and colour on cues; the grid filter (T-430). The filter
// dims the cells that don't match and never moves them: same places on
// screen, same keys, same APC40 pads, still playable. Preview only.
import { test, expect, useStudio, openUi, focusPage, openWorkspace } from '../studio';
import { Apc } from '../midi';

const studio = useStudio({ midiTest: true });
const apc = new Apc(studio);
type Grid = { pages: { cells: (string | null)[] }[] };
let cells: string[] = [];
const slots = async () => (await studio.get('/api/cues')).slots as Record<string, { notes?: string; tags?: string[]; color?: number[] }>;
const activeCue = async () => (await studio.controlValues()).active_cue;
const rects = (page: import('@playwright/test').Page) => page.locator('#cues > *').evaluateAll(els => els.map(e => {
  const r = e.getBoundingClientRect();
  return [r.x, r.y, r.width, r.height].map(Math.round).join(',');
}));

test.beforeAll(async () => {
  const g = await studio.get<Grid>('/api/grid');
  cells = g.pages[0].cells.filter((c): c is string => !!c);
});

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

async function openMenu(page: import('@playwright/test').Page, id: string) {
  await page.locator(`#cues .cue[data-id="${id}"]`).click({ button: 'right' });
  await expect(page.locator('#cueMenu')).toBeVisible();
}

test('tags, notes and colour from « Propriétés du cue »', async ({ page }) => {
  const [a, b] = cells;
  await openMenu(page, a);
  await page.locator('#cueMenuTagInput').fill('Intro');
  await page.keyboard.press('Enter');
  await page.locator('#cueMenuTagInput').fill('calme,');
  await expect(page.locator('#cueMenuTags .tag-chip')).toHaveCount(2);
  // The same tag again (case, accents) is ignored.
  await page.locator('#cueMenuTagInput').fill('ÍNTRO');
  await page.keyboard.press('Enter');
  await expect(page.locator('#cueMenuTags .tag-chip')).toHaveCount(2);
  await page.locator('#cueMenuNotes').fill('seulement avec <b>brouillard</b>');
  await page.locator('#cueMenuCellColor').evaluate((el: HTMLInputElement) => { el.value = '#ff0040'; el.dispatchEvent(new Event('change')); });
  await expect.poll(async () => (await slots())[a]?.color).toEqual([255, 0, 64]);
  await expect.poll(async () => (await slots())[a]?.tags).toEqual(['Intro', 'calme']);
  await expect.poll(async () => (await slots())[a]?.notes).toBe('seulement avec <b>brouillard</b>');
  // While typing, the letters were not cue keys.
  expect(await activeCue()).toBeNull();
  await page.locator('#cueMenuClose').click();

  const cell = page.locator(`#cues .cue[data-id="${a}"]`);
  await expect(cell.locator('.note')).toHaveCount(1);
  await expect(cell).toHaveAttribute('title', /seulement avec <b>brouillard<\/b>/);
  expect(await cell.locator('b').count()).toBe(0); // escaped, never HTML
  expect(await cell.evaluate(el => getComputedStyle(el).getPropertyValue('--cat').trim())).toBe('#ff0040');
  // « Aucune » gives the category colour back.
  await openMenu(page, a);
  await page.locator('#cueMenuCellColorNone').click();
  await expect.poll(async () => (await slots())[a]?.color ?? null).toBeNull();
  await page.locator('#cueMenuClose').click();

  expect(await studio.post('/api/cues/slot', { id: b, tags: ['intro'] })).toBe(200);
  const tags = await studio.get('/api/cues/tags');
  expect(tags).toEqual([{ tag: 'calme', count: 1 }, { tag: 'Intro', count: 2 }]);
});

test('the filter dims without moving, and dimmed cells still play (click, key, pad)', async ({ page }) => {
  const [a, b, c] = cells;
  expect(await studio.post('/api/cues/slot', { id: a, tags: ['intro', 'calme'] })).toBe(200);
  expect(await studio.post('/api/cues/slot', { id: b, tags: ['Intro'], notes: 'avec brouillard' })).toBe(200);
  await page.reload();
  await openWorkspace(page, 'live');
  // The bar opens from « Filtrer » (it takes no row from the grid unused).
  await expect(page.locator('#cueFilter')).toBeHidden();
  const before = await rects(page);
  await page.locator('#cueFilterBtn').click();
  await expect(page.locator('#cueFilterTags button')).toHaveCount(2);

  await page.locator('#cueFilterTags button', { hasText: 'Intro' }).click();
  await expect(page.locator('#cueFilterCount')).toHaveText(`2 / ${cells.length}`);
  await expect(page.locator('#cueFilterBadge')).toHaveText(`2 / ${cells.length}`);
  const filtered = await rects(page);
  await expect(page.locator(`#cues .cue[data-id="${c}"]`)).toHaveClass(/dimmed/);
  await expect(page.locator(`#cues .cue[data-id="${a}"]`)).not.toHaveClass(/dimmed/);
  expect(Number(await page.locator(`#cues .cue[data-id="${c}"]`).evaluate(el => getComputedStyle(el).opacity))).toBeLessThan(0.5);

  // Toutes / Au moins une.
  await page.locator('#cueFilterTags button', { hasText: 'calme' }).click();
  await expect(page.locator('#cueFilterCount')).toHaveText(`1 / ${cells.length}`);
  await page.locator('#cueFilterMode button', { hasText: 'Au moins une' }).click();
  await expect(page.locator('#cueFilterCount')).toHaveText(`2 / ${cells.length}`);
  // Text: name, note or tag, without accents or case.
  await page.locator('#cueFilterText').fill('BROUILLARD');
  await expect(page.locator('#cueFilterCount')).toHaveText(`1 / ${cells.length}`);
  expect(await rects(page)).toEqual(filtered);

  // The dimmed third cell plays from its key (E), a click and its pad.
  await expect(page.locator(`#cues .cue[data-id="${c}"]`)).toHaveClass(/dimmed/);
  await focusPage(page);
  await page.keyboard.press('KeyE');
  await expect.poll(activeCue).toBe(c);
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await expect.poll(activeCue).toBeNull();
  await page.locator(`#cues .cue[data-id="${c}"]`).click();
  await expect.poll(activeCue).toBe(c);
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await expect.poll(activeCue).toBeNull();
  await apc.pressPad(0, 2);
  await apc.releasePad(0, 2);
  await expect.poll(activeCue).toBe(c);

  // Remembered in this browser only; the output never saw it.
  await page.reload();
  await openWorkspace(page, 'live');
  await expect(page.locator(`#cues .cue[data-id="${c}"]`)).toHaveClass(/dimmed/);
  await expect(page.locator('#cueFilter')).toBeVisible();
  expect(await rects(page)).toEqual(filtered);
  await page.locator('#cueFilterClear').click();
  await expect(page.locator('#cues .cue.dimmed')).toHaveCount(0);
  expect(await rects(page)).toEqual(filtered);
  // Closing the bar drops the filter and gives the row back.
  await page.locator('#cueFilterBtn').click();
  await expect(page.locator('#cueFilter')).toBeHidden();
  expect(await rects(page)).toEqual(before);
});

test('notes, tags and colour are saved with the project', async ({ page }) => {
  const [a] = cells;
  expect(await studio.post('/api/cues/slot', { id: a, notes: 'n', tags: ['drop'], color: [0, 128, 255] })).toBe(200);
  expect(await studio.post('/api/project/save-as', { path: 'Etiquettes' })).toBe(200);
  expect(await studio.post('/api/cues/slot', { id: a })).toBe(200);
  expect((await slots())[a]).toBeUndefined();
  expect(await studio.post('/api/project/open', { path: 'Etiquettes' })).toBe(200);
  expect((await slots())[a]).toMatchObject({ notes: 'n', tags: ['drop'], color: [0, 128, 255] });
  await page.reload();
  await openWorkspace(page, 'live');
  expect(await page.locator(`#cues .cue[data-id="${a}"]`).evaluate(el => getComputedStyle(el).getPropertyValue('--cat').trim())).toBe('#0080ff');
  expect(await studio.post('/api/cues/slot', { id: a })).toBe(200);
});

test('a timeline event takes notes and tags, and shows them on hover', async ({ page }) => {
  expect(await studio.post('/api/timeline/new', { name: 'Notes', time_base: 'seconds' })).toBe(200);
  const { show } = await studio.get('/api/timeline');
  show.tracks[0].events = [{ id: 1, start: 0, len: 4, source: { kind: 'cue', id: cells[0] } }];
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe('Notes');
  await page.evaluate(() => (window as any).tlDebug.setView(0, 100));
  const v = await page.evaluate(() => (window as any).tlDebug.view());
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  const x = box.x + (2 - v.t0) * v.pps, y = box.y + v.top + v.lane / 2;
  await expect(page.locator('#tlNoteRow')).toBeHidden();
  await page.mouse.click(x, y);
  await expect(page.locator('#tlNoteRow')).toBeVisible();
  await page.locator('#tlEvNotes').fill('avec brouillard');
  await page.keyboard.press('Enter');
  await page.locator('#tlEvTags').fill('Drop, drop, montée');
  await page.keyboard.press('Enter');
  const ev = async () => (await studio.get('/api/timeline')).show.tracks[0].events[0];
  await expect.poll(async () => (await ev()).notes).toBe('avec brouillard');
  await expect.poll(async () => (await ev()).tags).toEqual(['Drop', 'montée']);
  await page.mouse.move(x + 5, y);
  await expect(page.locator('#tlCanvas')).toHaveAttribute('title', /avec brouillard/);
});
