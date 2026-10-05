// « Préparer » (T-274): a cue loaded blind into the « Préparé » view
// changes /api/preview-frame, never /api/frame; « Envoyer » (button or
// Entrée du pavé numérique) plays it like a click on its cell.
import { test, expect, useStudio, openUi, focusPage } from '../studio';

const studio = useStudio();
type Prepared = { rev: number; cue: null | { id: string; name: string; scale: number; brightness: number; rotation_speed: number; color: number[] } };
type PreviewFrame = { prepared: Prepared; points: number[][] };
const preview = () => studio.get<PreviewFrame>('/api/preview-frame');
const cell = (page: import('@playwright/test').Page, i: number) => page.locator('#cues .cue[data-id]').nth(i);

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/prepared/clear');
  await studio.post('/api/running/stop');
  // A still square on the output, so its frame can be compared point for point.
  const st = await studio.state();
  expect(await studio.post('/api/settings', { ...st.settings, content: { kind: 'shape', shape: 'square' }, rotation_speed: 0 })).toBe(200);
  await openUi(page, studio);
});

test('Préparer: a clicked cell goes to the prepared view, not the output; Envoyer plays it', async ({ page }) => {
  const before = await studio.frame();
  expect(before.points.length).toBeGreaterThan(0);
  await expect(page.locator('#prepPane')).toBeHidden();
  await page.locator('#prepBtn').click();
  await expect(page.locator('#prepBtn')).toHaveClass(/active/);
  await expect(page.locator('#prepPane')).toBeVisible();
  await expect(page.locator('#outTag')).toBeVisible();
  await expect(page.locator('#outTag')).toHaveText('Sortie');
  await expect(page.locator('#prepPane .prep-head b')).toHaveText('Préparé');
  await expect(page.locator('#prepSend')).toBeDisabled();

  const target = cell(page, 2);
  const id = await target.getAttribute('data-id');
  await target.click();
  await expect.poll(async () => (await preview()).prepared.cue?.id).toBe(id);
  await expect(target).toHaveClass(/prepared/);
  await expect(page.locator('#prepName')).not.toHaveText('');
  await expect.poll(async () => (await preview()).points.length).toBeGreaterThan(0);
  // The output did not move: same points, nothing playing.
  const after = await studio.frame();
  expect(after.points).toEqual(before.points);
  const st = await studio.controlValues();
  expect(st.active_cue).toBeNull();
  await expect(target).not.toHaveClass(/active/);

  // An adjustment reaches the prepared look only.
  await page.locator('#prepScale').fill('25');
  await expect.poll(async () => (await preview()).prepared.cue?.scale).toBeCloseTo(0.25, 2);
  expect((await studio.frame()).points).toEqual(before.points);

  await page.locator('#prepSend').click();
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(id);
  await expect.poll(async () => (await preview()).prepared.cue).toBeNull();
  await expect(target).toHaveClass(/active/);
  expect((await studio.state()).settings.scale).toBeCloseTo(0.25, 2);
  expect((await studio.state()).armed).toBe(false);
});

test('Alt+click prepares without the mode, and Entrée du pavé numérique sends', async ({ page }) => {
  const target = cell(page, 4);
  const id = await target.getAttribute('data-id');
  await target.click({ modifiers: ['Alt'] });
  await expect.poll(async () => (await preview()).prepared.cue?.id).toBe(id);
  await expect(page.locator('#prepBtn')).not.toHaveClass(/active/);
  await expect(page.locator('#prepPane')).toBeVisible();
  expect((await studio.controlValues()).active_cue).toBeNull();
  // « Vider » forgets it, « Préparé » goes away with the mode off.
  await page.locator('#prepClear').click();
  await expect.poll(async () => (await preview()).prepared.cue).toBeNull();
  await expect(page.locator('#prepPane')).toBeHidden();

  await target.click({ modifiers: ['Alt'] });
  await expect.poll(async () => (await preview()).prepared.cue?.id).toBe(id);
  await focusPage(page);
  await page.keyboard.press('NumpadEnter');
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(id);
  await expect.poll(async () => (await preview()).prepared.cue).toBeNull();
});
