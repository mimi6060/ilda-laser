// TIMELINE empty state and built-in example shows (T-124 to T-129): with no
// show, the editor still looks like a timeline (ruler, four empty tracks)
// with « Nouveau show » and « Ouvrir un exemple »; an example opens as an
// unsaved copy (nothing written), follows the live tempo, plays on two
// sections, and saves only under a name of its own. Preview only: nothing
// here arms the laser.
import { test, expect, useStudio, openUi, openWorkspace, isLit, type Point } from '../studio';

const studio = useStudio();
test.describe.configure({ mode: 'serial' });

const timeline = () => studio.get<{ state: any; show: any }>('/api/timeline');
const frame = () => studio.get<{ points: Point[]; timeline: any; tempo: { bpm: number } }>('/api/frame');
const showNames = async () => (await studio.get<{ name: string }[]>('/api/shows')).map(s => s.name);

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test.afterEach(async () => {
  expect((await studio.state()).armed, 'examples never arm').toBe(false);
});

test('no show: a ruler, four empty tracks, Nouveau show and Ouvrir un exemple', async ({ page }) => {
  expect((await timeline()).show).toBeNull();
  const projectModified = (await studio.get('/api/project')).modified;
  await openWorkspace(page, 'timeline');
  const empty = page.locator('#tlEmpty');
  await expect(empty).toBeVisible();
  await expect(empty.locator('#tlEmptyNew')).toHaveText('Nouveau show');
  await expect(empty.locator('#tlEmptyNew')).toHaveClass(/primary/);
  await expect(empty.locator('#tlEmptyEx')).toHaveText('Ouvrir un exemple');
  await expect(empty).toContainText('Glissez des cues depuis la liste de gauche sur une piste');
  await expect(page.locator('#tlHeads .tl-head.empty')).toHaveCount(4);
  // The canvas draws the ruler and the four lanes (88 px of ruler, markers, audio + 4 × 34).
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  expect(Math.round(box.height)).toBe(88 + 4 * 34);
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.stats().draws)).toBeGreaterThan(0);
  await page.screenshot({ path: 'test-results/timeline-empty.png' });

  // « Nouveau show » opens the name row with a free name, ready for Entrée.
  await page.locator('#tlEmptyNew').click();
  await expect(page.locator('#tlNewName')).toBeVisible();
  await expect(page.locator('#tlNewName')).toHaveValue('Nouveau show');
  await expect(page.locator('#tlNewName')).toBeFocused();
  // Nothing created on disk just by visiting or opening the row.
  expect(await showNames()).toEqual([]);
  expect((await studio.get('/api/project')).modified, 'the project is not touched').toBe(projectModified);
});

test('an example opens as a copy, follows the tempo and plays two sections', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await page.locator('#tlEmptyEx').click();
  const dlg = page.locator('#tlExDialog');
  await expect(dlg).toBeVisible();
  await expect(dlg.locator('.tl-ex')).toHaveCount(6);
  await expect(dlg).toContainText('Montée 16 mesures → drop');
  await expect(dlg).toContainText('150 BPM conseillé · 32 mesures');
  await dlg.locator('.tl-ex', { hasText: 'Attaque hardstyle' }).click();
  await expect(dlg).toBeHidden();
  await expect.poll(async () => (await timeline()).state.name).toBe('Attaque hardstyle');
  const { state, show } = await timeline();
  expect([state.example, state.modified, state.time_base]).toEqual([true, false, 'beats']);
  expect(show.tracks).toHaveLength(4);
  expect(show.markers.map((m: any) => m.name)).toContain('DROP');
  await expect(page.locator('#tlEmpty')).toBeHidden();
  await expect(page.locator('#tlHeads .tl-head:not(.empty)')).toHaveCount(4);
  await expect(page.locator('#tlModified')).toContainText('exemple');
  expect(await showNames(), 'nothing written').not.toContain('Attaque hardstyle');

  // Tempo conseillé: one click sets the live tempo the show follows.
  await expect(page.locator('#tlTempoHint')).toHaveText('Tempo conseillé : 150 BPM');
  await page.locator('#tlTempoHint').click();
  await expect.poll(async () => (await frame()).tempo.bpm).toBe(150);
  await expect(page.locator('#tlTempoHint')).toBeHidden();

  // Section 1 (bar 1, red stabs) then section 4 (bar 25, the drop).
  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await frame()).timeline.active.length, { timeout: 10_000 }).toBeGreaterThan(0);
  const first = (await frame()).timeline.active;
  expect(await studio.post('/api/timeline/seek', { position: 96.1 })).toBe(200);
  await expect.poll(async () => (await frame()).timeline.active.length).toBe(2);
  const drop = (await frame()).timeline.active;
  expect(drop).not.toEqual(first);
  await expect.poll(async () => (await frame()).points.some(isLit)).toBe(true);
  await page.locator('#tlFit').click();
  await page.screenshot({ path: 'test-results/timeline-example.png' });

  // Enregistrer asks for a name of its own; the copy is then a plain show.
  page.once('dialog', d => d.accept('Ma copie hardstyle'));
  await page.locator('#tlSave').click();
  await expect.poll(showNames).toContain('Ma copie hardstyle');
  await expect.poll(async () => (await timeline()).state.example).toBe(false);
  await expect(page.locator('#tlModified')).toHaveText('');
  await studio.post('/api/timeline/stop');
});

test('Échap stops the laser and closes the examples', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await page.locator('#tlExamples').click();
  await expect(page.locator('#tlExDialog')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('#tlExDialog')).toBeHidden();
  expect(await studio.post('/api/estop/reset')).toBe(200);
});
