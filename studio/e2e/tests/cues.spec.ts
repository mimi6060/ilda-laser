// Cue grid: page tabs, clicking a cue, AZERTY cue keys, and pages changed
// from outside the UI (MIDI / API) showing up in it.
import { test, expect, useStudio, openUi, focusPage } from '../studio';

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
  await page.locator('#bright').fill('30');
  await expect.poll(async () => (await studio.state()).settings.brightness).toBeCloseTo(0.3, 3);
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
  await page.locator('[data-kind="text"]').click();
  await page.locator('#text').fill('');
  await page.locator('#text').pressSequentially('az');
  await expect.poll(async () => (await studio.state()).settings.content.text).toBe('az');
  expect(await activeCue()).toBe(before);
  await expect(page.locator('#cues .cue.active')).toHaveCount(0);
});

// T-292: the server keeps `active_cue` after the look is changed by hand
// (or by a scene / the playlist). The page un-highlights the cue locally,
// but a reload (or a MIDI grid's LED feedback) still shows the old cue as
// playing.
test.fixme('T-292: changing the look by hand clears the active cue', async ({ page }) => {
  await page.locator('#cues .cue').first().click();
  await expect.poll(activeCue).toBe(pageCues(catalog.categories[0])[0].id);
  await page.locator('[data-kind="shape"]').click();
  await page.getByRole('button', { name: 'Carré', exact: true }).click();
  await expect.poll(async () => (await studio.state()).settings.content.shape).toBe('square');
  await expect(page.locator('#cues .cue.active')).toHaveCount(0);
  await expect.poll(activeCue).toBeNull();
  await page.reload();
  await openUi(page, studio);
  await expect(page.locator('#cues .cue.active')).toHaveCount(0);
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
