// Cue grid: page tabs, clicking a cue, AZERTY cue keys, and pages changed
// from outside the UI (MIDI / API) showing up in it.
import { test, expect, useStudio, openUi, reveal, focusPage } from '../studio';

const studio = useStudio();
let catalog: Awaited<ReturnType<typeof studio.presets>>;
const pageCues = (category: string) => catalog.presets.filter(p => p.category === category);
const activeCue = async () => (await studio.controlValues()).active_cue;
const cuePage = async () => (await studio.controlValues()).cue_page; // 1-based

test.beforeAll(async () => { catalog = await studio.presets(); });

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('shows one tab per category and the first page of cues', async ({ page }) => {
  const tabs = page.locator('#cueTabs button');
  await expect(tabs).toHaveCount(catalog.categories.length);
  const first = catalog.categories[0];
  await expect(tabs.first()).toHaveText(`${first} (${pageCues(first).length})`);
  await expect(tabs.first()).toHaveClass(/active/);
  await expect(page.locator('#cues .cue')).toHaveCount(pageCues(first).length);
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
  const tab = page.locator('#cueTabs button', { hasText: category });
  await tab.click();
  await expect(page.locator('#cueTabs button', { hasText: category })).toHaveClass(/active/);
  await expect.poll(cuePage).toBe(3);
  await expect(page.locator('#cues .cue')).toHaveCount(cues.length);
  await expect(page.locator('#cues .cue').first()).toContainText(cues[0].name);

  await page.locator('#cues .cue').nth(4).click();
  await expect.poll(activeCue).toBe(cues[4].id);
  await expect(page.locator('#cues .cue').nth(4)).toHaveClass(/active/);
  // The page stays where the operator put it.
  await expect(page.locator('#cueTabs button', { hasText: category })).toHaveClass(/active/);
  expect(await cuePage()).toBe(3);
});

test('an AZERTY key plays the matching cue of the current page', async ({ page }) => {
  const category = catalog.categories[1];
  const cues = pageCues(category);
  await page.locator('#cueTabs button', { hasText: category }).click();
  await expect.poll(cuePage).toBe(2);
  await focusPage(page);

  await page.keyboard.press('z'); // 2nd key on the AZERTY row
  await expect.poll(activeCue).toBe(cues[1].id);
  await expect(page.locator(`#cues .cue[data-id="${cues[1].id}"]`)).toHaveClass(/active/);

  await page.keyboard.press('q'); // first key of the second row = 11th cue
  await expect.poll(activeCue).toBe(cues[10].id);
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
  await expect(page.locator('#cueTabs button', { hasText: category })).toHaveClass(/active/);
  await expect(page.locator('#cues .cue').first()).toContainText(pageCues(category)[0].name);

  // And a grid cell played from outside is highlighted.
  expect(await studio.post('/api/control', { id: 'grid.5.1.2' })).toBe(200);
  await expect.poll(activeCue).toBe(pageCues(category)[1].id);
  await expect(page.locator('#cues .cue').nth(1)).toHaveClass(/active/);
});
