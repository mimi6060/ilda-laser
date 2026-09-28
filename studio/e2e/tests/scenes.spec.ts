// Scenes: save → play → delete, and the playlist advancing on its own.
import { test, expect, useStudio, openUi, reveal, type Page } from '../studio';

const studio = useStudio();
const scenes = async () => (await studio.state()).scenes as { name: string; duration_secs: number }[];
const shape = async () => (await studio.state()).settings.content.shape;
const row = (page: Page, name: string) => page.locator('#sceneList .scene', { hasText: name });

async function pickShape(page: Page, label: string, id: string) {
  await reveal(page, '[data-kind="shape"]'); // CRÉATION
  await page.locator('[data-kind="shape"]').click();
  await page.getByRole('button', { name: label, exact: true }).click();
  await expect.poll(shape).toBe(id);
}

async function saveScene(page: Page, name: string, seconds?: number) {
  await reveal(page, '#sceneName'); // LIVE › Scènes
  await page.locator('#sceneName').fill(name);
  if (seconds !== undefined) await page.locator('#sceneDur').fill(String(seconds));
  await page.locator('#sceneSave').click();
  await expect(row(page, name)).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  for (const sc of await scenes()) await studio.post('/api/scenes/delete', { name: sc.name });
  await openUi(page, studio);
});

test('an empty list explains what to do', async ({ page }) => {
  await reveal(page, '#sceneList');
  await expect(page.locator('#sceneList')).toContainText('donnez-lui un nom');
  // Saving without a name does nothing and puts the cursor in the name field.
  await page.locator('#sceneSave').click();
  await expect(page.locator('#sceneName')).toBeFocused();
  expect(await scenes()).toEqual([]);
});

test('save, play, then delete a scene', async ({ page }) => {
  await pickShape(page, 'Carré', 'square');
  await saveScene(page, 'Carré vert');
  await expect(page.locator('#sceneName')).toHaveValue('');
  await expect(row(page, 'Carré vert')).toContainText('8 s');
  expect(await scenes()).toEqual([expect.objectContaining({ name: 'Carré vert', duration_secs: 8 })]);

  // Change the look, then play the scene back.
  await pickShape(page, 'Étoile', 'star');
  await reveal(page, '#sceneList');
  await row(page, 'Carré vert').getByRole('button', { name: '▶' }).click();
  await expect.poll(shape).toBe('square');
  await expect(page.locator('#shapes button[data-shape="square"]')).toHaveClass(/active/);

  await row(page, 'Carré vert').getByTitle('Supprimer').click();
  await expect(page.locator('#sceneList .scene')).toHaveCount(0);
  expect(await scenes()).toEqual([]);
});

test('the playlist steps through the scenes, and Stop ends it', async ({ page }) => {
  await pickShape(page, 'Cercle', 'circle');
  await saveScene(page, 'Un', 0.5);
  await pickShape(page, 'Triangle', 'triangle');
  await saveScene(page, 'Deux', 0.5);
  expect((await scenes()).map(s => s.duration_secs)).toEqual([0.5, 0.5]);

  await page.locator('#plStart').click();
  await expect.poll(async () => (await studio.state()).playlist).not.toBeNull();
  // It advances on its own, and the look follows.
  await expect.poll(async () => (await studio.state()).playlist).toBe(1);
  await expect.poll(shape).toBe('triangle');
  await expect.poll(async () => (await studio.state()).playlist).toBe(0);
  await expect.poll(shape).toBe('circle');
  // The playing scene is highlighted in the list.
  await expect(page.locator('#sceneList .scene.playing')).toHaveCount(1);

  await page.locator('#plStop').click();
  await expect.poll(async () => (await studio.state()).playlist).toBeNull();
  await expect(page.locator('#sceneList .scene.playing')).toHaveCount(0);
});

// T-293: the page follows the look when the playlist (or MIDI / the API)
// changes it, so the next slider move edits the scene on stage instead of
// sending the pre-playlist look back.
test('T-293: a slider move during the playlist keeps the scene on stage', async ({ page }) => {
  await pickShape(page, 'Cercle', 'circle');
  await saveScene(page, 'Un', 60);
  await pickShape(page, 'Triangle', 'triangle');
  await saveScene(page, 'Deux', 60);
  await pickShape(page, 'Étoile', 'star');
  await reveal(page, '#plStart');
  await page.locator('#plStart').click();
  await expect.poll(shape).toBe('circle'); // first scene
  await expect(page.locator('#shapes button[data-shape="circle"]')).toHaveClass(/active/);
  await reveal(page, '#scale');
  await page.locator('#scale').fill('80');
  await expect.poll(async () => (await studio.state()).settings.scale).toBeCloseTo(0.8, 3);
  expect(await shape()).toBe('circle');
});

test('touching the look by hand takes over from the playlist', async ({ page }) => {
  await saveScene(page, 'Seule', 0.5);
  await page.locator('#plStart').click();
  await expect.poll(async () => (await studio.state()).playlist).toBe(0);
  await reveal(page, '#shapes');
  await page.getByRole('button', { name: 'Croix', exact: true }).click();
  await expect.poll(async () => (await studio.state()).playlist).toBeNull();
  expect(await shape()).toBe('cross');
});
