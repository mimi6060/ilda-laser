// Cue grid: page tabs, clicking a cue, AZERTY cue keys, and pages changed
// from outside the UI (MIDI / API) showing up in it. T-272: a fixed 8×5
// grid with empty cells, pages on F5–F12 / Alt+1…9, cue keys by e.code.
import { test, expect, useStudio, openUi, reveal, focusPage } from '../studio';

const studio = useStudio();
let catalog: Awaited<ReturnType<typeof studio.presets>>;
const pageCues = (category: string) => catalog.presets.filter(p => p.category === category);
const activeCue = async () => (await studio.controlValues()).active_cue;
const cuePage = async () => (await studio.controlValues()).cue_page; // 1-based
type Grid = { cols: number; rows: number; default: boolean; page: number; pages: { name: string; cells: (string | null)[]; hidden: number }[] };
const getGrid = () => studio.get<Grid>('/api/grid');
// The first pages are the categories, in order, each holding its first 40 cues.
const tab = (page: import('@playwright/test').Page, i: number) => page.locator('#cueTabs button').nth(i);

test.beforeAll(async () => { catalog = await studio.presets(); });

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('shows one tab per page (categories first) and the first page of cues', async ({ page }) => {
  const tabs = page.locator('#cueTabs button');
  const grid = await getGrid();
  // T-110, T-112, T-462: « Festival », « Festival évolutifs » and
  // « Objets 3D », the last three categories, came later: they follow the
  // overflow pages, so the older pages keep their numbers.
  const early = catalog.categories.slice(0, -3);
  expect(grid.pages.slice(0, early.length).map(p => p.name)).toEqual(early);
  const names = grid.pages.map(p => p.name);
  expect(names.indexOf('Festival')).toBeGreaterThan(names.indexOf('Faisceaux 2'));
  expect(names.indexOf('Festival évolutifs')).toBe(names.indexOf('Festival') + 1);
  expect(names.indexOf('Objets 3D')).toBe(names.indexOf('Festival évolutifs') + 1);
  await expect(tabs).toHaveCount(grid.pages.length);
  const first = catalog.categories[0];
  await expect(tabs.first().locator('.pn')).toHaveText(`${first} (40)`);
  await expect(tabs.first().locator('kbd')).toHaveText('F5');
  await expect(tabs.nth(8).locator('kbd')).toHaveText('Alt+9');
  await expect(tabs.first()).toHaveClass(/active/);
  // A fixed 8×5 page: 40 cells, the rest of the category on a later page.
  await expect(page.locator('#cues .cue')).toHaveCount(40);
  const more = grid.pages.findIndex(p => p.name === `${first} 2`);
  expect(more).toBeGreaterThanOrEqual(early.length);
  expect(grid.pages[more].cells.filter(Boolean)).toEqual(pageCues(first).slice(40).map(p => p.id));
  await expect(page.locator('#cues .cue').first()).toContainText(pageCues(first)[0].name);
  await expect(page.locator('#cues .cue').first().locator('.key')).toHaveText('A');
});

test('clicking a cue plays it', async ({ page }) => {
  const cue = pageCues(catalog.categories[0])[3];
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
  await expect.poll(activeCue).toBe(cue.id);
  await expect(page.locator(`#cues .cue[data-id="${cue.id}"]`)).toHaveClass(/active/);
  const st = await studio.state();
  expect(st.settings.content.kind).toBe('generator');
  // The Effet panel shows the cue's generator.
  await expect(page.locator('[data-kind="generator"]')).toHaveClass(/active/);
  await expect(page.locator('#gen')).toHaveValue(st.settings.content.generator);
});

test('a cue keeps the operator\'s look brightness', async ({ page }) => {
  await reveal(page, '#bright');
  await page.locator('#bright').fill('30');
  await expect.poll(async () => (await studio.state()).settings.brightness).toBeCloseTo(0.3, 3);
  await reveal(page, '#cues');
  await page.locator('#cues .cue').nth(1).click();
  await expect.poll(activeCue).toBe(pageCues(catalog.categories[0])[1].id);
  expect((await studio.state()).settings.brightness).toBeCloseTo(0.3, 3);
});

test('a page tab shows that page, and a click plays that page\'s cue', async ({ page }) => {
  const category = catalog.categories[2];
  const cues = pageCues(category);
  await tab(page, 2).click();
  await expect(tab(page, 2)).toHaveClass(/active/);
  await expect.poll(cuePage).toBe(3);
  await expect(page.locator('#cues .cue')).toHaveCount(Math.min(40, cues.length));
  await expect(page.locator('#cues .cue').first()).toContainText(cues[0].name);

  await page.locator('#cues .cue').nth(4).click();
  await expect.poll(activeCue).toBe(cues[4].id);
  await expect(page.locator('#cues .cue').nth(4)).toHaveClass(/active/);
  // The page stays where the operator put it.
  await expect(tab(page, 2)).toHaveClass(/active/);
  expect(await cuePage()).toBe(3);
});

test('a cue key plays the cell under the same finger of the current page', async ({ page }) => {
  const category = catalog.categories[1];
  const cues = pageCues(category);
  await tab(page, 1).click();
  await expect.poll(cuePage).toBe(2);
  await focusPage(page);

  // Playwright types on a QWERTY layout: W is where the AZERTY Z is (2nd key).
  await page.keyboard.press('KeyW');
  await expect.poll(activeCue).toBe(cues[1].id);
  await expect(page.locator(`#cues .cue[data-id="${cues[1].id}"]`)).toHaveClass(/active/);
  await expect(page.locator('#cues .cue').nth(1).locator('.key')).toHaveText('Z');

  await page.keyboard.press('KeyA'); // first key of the second row (AZERTY Q) = 11th cue
  await expect.poll(activeCue).toBe(cues[10].id);
});

test('T-272: AZERTY « a » and QWERTY « q » (same e.code KeyQ) play the same cell', async ({ page }) => {
  const cues = pageCues(catalog.categories[0]);
  await focusPage(page);
  const press = (key: string) => page.evaluate(k => {
    for (const type of ['keydown', 'keyup']) document.dispatchEvent(new KeyboardEvent(type, { key: k, code: 'KeyQ', bubbles: true }));
  }, key);
  await press('a'); // AZERTY
  await expect.poll(activeCue).toBe(cues[0].id);
  await press('a'); // toggles it off
  await expect.poll(activeCue).toBeNull();
  await press('q'); // QWERTY
  await expect.poll(activeCue).toBe(cues[0].id);
});

test('T-272: F6 and Alt+2 show page 2; F1–F4 still switch workspaces', async ({ page }) => {
  await focusPage(page);
  await page.keyboard.press('F6');
  await expect.poll(cuePage).toBe(2);
  await expect(tab(page, 1)).toHaveClass(/active/);
  await page.keyboard.press('F5');
  await expect.poll(cuePage).toBe(1);
  await page.keyboard.press('Alt+Digit2');
  await expect.poll(cuePage).toBe(2);
  await expect(tab(page, 1)).toHaveClass(/active/);
  // Letters now play page 2's cues.
  await page.keyboard.press('KeyQ');
  await expect.poll(activeCue).toBe(pageCues(catalog.categories[1])[0].id);
  await page.keyboard.press('F2');
  await expect(page.locator('#wsTabs [data-ws-tab="timeline"]')).toHaveClass(/active/);
  expect(await cuePage()).toBe(2);
  await page.keyboard.press('F1');
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);
});

test('T-272: the grid stays 8×5 at any width; empty cells keep their place and do nothing', async ({ page }) => {
  const cues = pageCues(catalog.categories[1]); // Tunnels: fewer than 40
  expect(cues.length).toBeLessThan(40);
  await tab(page, 1).click();
  for (const width of [1440, 1100]) {
    await page.setViewportSize({ width, height: 900 });
    const cells = page.locator('#cues [data-slot]');
    await expect(cells).toHaveCount(40);
    const tops = await cells.evaluateAll(els => els.map(e => Math.round(e.getBoundingClientRect().top)));
    expect(new Set(tops).size, `${width}px`).toBe(5);
  }
  await expect(page.locator('#cues .cue-empty')).toHaveCount(40 - cues.length);
  const empty = page.locator('#cues .cue-empty').first();
  await empty.click({ force: true });
  await page.waitForTimeout(200);
  expect(await activeCue()).toBeNull();
  // The key of an empty cell plays nothing either (cell 40 has no key; cell 26 = N).
  await focusPage(page);
  if (cues.length < 26) {
    await page.keyboard.press('KeyN');
    await page.waitForTimeout(200);
    expect(await activeCue()).toBeNull();
  }
});

test('T-272: a 6×4 grid keeps the cues in order and is saved', async ({ page }) => {
  await page.locator('#gridMenuBtn').click();
  await page.locator('#gridCols').fill('6');
  await page.locator('#gridCols').press('Tab');
  await page.locator('#gridRows').fill('4');
  await page.locator('#gridRows').press('Tab');
  await expect.poll(async () => { const g = await getGrid(); return [g.cols, g.rows]; }).toEqual([6, 4]);
  await expect(page.locator('#cues [data-slot]')).toHaveCount(24);
  const first = pageCues(catalog.categories[0]);
  const shown = await page.locator('#cues .cue').evaluateAll(els => els.map(e => (e as HTMLElement).dataset.id));
  expect(shown).toEqual(first.slice(0, 24).map(p => p.id));
  // Every cue of the category is still on a page, in order.
  const grid = await getGrid();
  const all = grid.pages.flatMap(p => p.cells).filter(id => first.some(c => c.id === id));
  expect(all).toEqual(first.map(p => p.id));
  // Saved: a reload shows the same size.
  await page.reload();
  await openUi(page, studio);
  await expect(page.locator('#cues [data-slot]')).toHaveCount(24);
});

test('T-272: a new page is empty, a cue dragged onto it plays from there', async ({ page }) => {
  await page.locator('#gridMenuBtn').click();
  await page.locator('#gridPageName').fill('Final');
  await page.locator('#gridPageNew').click();
  const active = page.locator('#cueTabs button.active');
  await expect(active.locator('.pn')).toHaveText('Final (0)');
  await expect(page.locator('#cues')).toHaveAttribute('data-empty', 'Page vide');
  await expect(page.locator('#cues .cue-empty')).toHaveCount(40);
  const final = (await getGrid()).pages.findIndex(p => p.name === 'Final');
  // « Réorganiser »: drag the first cue of page 1 onto the Final tab.
  await tab(page, 0).click();
  await page.locator('#gridEdit').click();
  const cue = pageCues(catalog.categories[0])[0];
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).dragTo(tab(page, final));
  await expect.poll(async () => (await getGrid()).pages[final].cells[0]).toBe(cue.id);
  // While editing, a click doesn't play.
  expect(await activeCue()).toBeNull();
  await page.locator('#gridEdit').click();
  await tab(page, final).click();
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
  await expect.poll(activeCue).toBe(cue.id);
  // Renamed, then removed from the page through « Propriétés du cue ».
  await page.locator('#gridPageName').fill('Rappel');
  await page.locator('#gridPageRename').click();
  await expect(page.locator('#cueTabs button.active .pn')).toHaveText('Rappel (1)');
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click({ button: 'right' });
  await page.locator('#cueMenuRemove').click();
  await expect(page.locator('#cues .cue')).toHaveCount(0);
});

test('T-272: in show mode the grid can\'t be edited but pages still change', async ({ page }) => {
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  await expect(page.locator('#gridMenuBtn')).toBeHidden();
  expect(await studio.post('/api/grid', { cols: 6, rows: 4 })).toBe(423);
  expect(await studio.post('/api/grid/page', { action: 'new' })).toBe(423);
  await focusPage(page);
  await page.keyboard.press('F7');
  await expect.poll(cuePage).toBe(3);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
});

test('cue keys are ignored while typing in a text field', async ({ page }) => {
  const before = await activeCue();
  await reveal(page, '[data-kind="text"]');
  await page.locator('[data-kind="text"]').click();
  await page.locator('#text').fill('');
  await page.locator('#text').pressSequentially('az');
  await expect.poll(async () => (await studio.state()).settings.content.text).toBe('az');
  expect(await activeCue()).toBe(before);
  await expect(page.locator('#cues .cue.active')).toHaveCount(0);
  // Also in LIVE, where cue letters are live: typing a scene name plays nothing.
  await reveal(page, '#sceneName');
  await page.locator('#sceneName').pressSequentially('az');
  await expect(page.locator('#sceneName')).toHaveValue('az');
  expect(await activeCue()).toBe(before);
  await expect(page.locator('#cues .cue.active')).toHaveCount(0);
});

// T-292: picking another drawing by hand stops the cue on the server too,
// so a reload (or a MIDI grid's LED feedback) no longer shows it playing.
test('T-292: changing the look by hand clears the active cue', async ({ page }) => {
  await page.locator('#cues .cue').first().click();
  await expect.poll(activeCue).toBe(pageCues(catalog.categories[0])[0].id);
  await reveal(page, '[data-kind="shape"]');
  await page.locator('[data-kind="shape"]').click();
  await page.getByRole('button', { name: 'Carré', exact: true }).click();
  await expect.poll(async () => (await studio.state()).settings.content.shape).toBe('square');
  await expect(page.locator('#cues .cue.active')).toHaveCount(0);
  await expect.poll(activeCue).toBeNull();
  await page.reload();
  await openUi(page, studio);
  await expect(page.locator('#cues .cue.active')).toHaveCount(0);
});

test('T-292: a size edit or a master modifier keeps the cue playing', async ({ page }) => {
  const cue = pageCues(catalog.categories[0])[2];
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
  await expect.poll(activeCue).toBe(cue.id);
  await expect(page.locator('#gen')).toHaveValue((await studio.state()).settings.content.generator);
  await reveal(page, '#scale');
  await page.locator('#scale').fill('40');
  await expect.poll(async () => (await studio.state()).settings.scale).toBeCloseTo(0.4, 3);
  expect(await studio.post('/api/control', { id: 'master.size', value: 1.2 })).toBe(200);
  expect(await activeCue()).toBe(cue.id);
  await expect(page.locator(`#cues .cue[data-id="${cue.id}"]`)).toHaveClass(/active/);
});

test('T-292: a scene stops the cue', async ({ page }) => {
  await reveal(page, '#sceneName');
  await page.locator('#sceneName').fill('Fond');
  await page.locator('#sceneSave').click();
  await expect(page.locator('#sceneList .scene', { hasText: 'Fond' })).toBeVisible();
  await page.locator('#cues .cue').first().click();
  await expect.poll(activeCue).toBe(pageCues(catalog.categories[0])[0].id);
  await page.locator('#sceneList .scene', { hasText: 'Fond' }).locator('button').first().click();
  await expect.poll(activeCue).toBeNull();
  await expect(page.locator('#cues .cue.active')).toHaveCount(0);
  await studio.post('/api/scenes/delete', { name: 'Fond' });
});

test('T-293: a cue played through the API updates the Effet panel', async ({ page }) => {
  const category = catalog.categories[0];
  expect(await studio.post('/api/control', { id: 'grid.1.1.3' })).toBe(200);
  await expect.poll(activeCue).toBe(pageCues(category)[2].id);
  const st = await studio.state();
  await expect(page.locator('[data-kind="generator"]')).toHaveClass(/active/);
  await expect(page.locator('#gen')).toHaveValue(st.settings.content.generator);
  await expect(page.locator('#gCount')).toHaveValue(String(st.settings.content.params.count));
  // And the next slider move edits that cue, not the look from before.
  await reveal(page, '#bright');
  await page.locator('#bright').fill('70');
  await expect.poll(async () => (await studio.state()).settings.brightness).toBeCloseTo(0.7, 3);
  expect((await studio.state()).settings.content).toEqual(st.settings.content);
  expect(await activeCue()).toBe(pageCues(category)[2].id);
});

test('a page changed from outside (MIDI/API) shows up in the UI', async ({ page }) => {
  const category = catalog.categories[4];
  expect(await studio.post('/api/control', { id: 'page.5' })).toBe(200);
  await expect(tab(page, 4)).toHaveClass(/active/);
  await expect(page.locator('#cues .cue').first()).toContainText(pageCues(category)[0].name);

  // And a grid cell played from outside is highlighted.
  expect(await studio.post('/api/control', { id: 'grid.5.1.2' })).toBe(200);
  await expect.poll(activeCue).toBe(pageCues(category)[1].id);
  await expect(page.locator('#cues .cue').nth(1)).toHaveClass(/active/);
});
