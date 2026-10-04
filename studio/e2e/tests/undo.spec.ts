// Annuler / Rétablir / Historique (T-285): the studio's history of edits.
// Cmd+Z after deleting a scene brings it back as it was; playing a cue is
// not an edit; a click in « Historique » undoes down to that entry; show
// mode refuses it. Preview only: nothing here arms.
import { test, expect, useStudio, openUi, reveal, focusPage, type Page } from '../studio';

const studio = useStudio();
type Scene = { name: string; duration_secs: number; settings: unknown };
const scenes = async () => (await studio.state()).scenes as Scene[];
const history = async () => (await studio.state()).history as { undo: number; redo: number; undo_label: string | null };

test.beforeEach(async ({ page }) => {
  await studio.reset();
  for (const sc of await scenes()) await studio.post('/api/scenes/delete', { name: sc.name });
  await openUi(page, studio);
});

async function saveScene(page: Page, name: string) {
  await reveal(page, '#sceneName'); // LIVE › Scènes
  await page.locator('#sceneName').fill(name);
  await page.locator('#sceneDur').fill('6');
  await page.locator('#sceneSave').click();
  await expect(page.locator('#sceneList .scene', { hasText: name })).toBeVisible();
}

test('Cmd+Z brings a deleted scene back as it was, Cmd+Maj+Z deletes it again', async ({ page }) => {
  await saveScene(page, 'Oops');
  const saved = (await scenes()).find(s => s.name === 'Oops');
  expect(saved).toBeTruthy();
  // Deleted from the list: hold the button (T-283).
  await page.locator('#sceneList .scene', { hasText: 'Oops' }).getByTitle(/^Supprimer/).hover();
  await page.mouse.down();
  await expect(page.locator('#sceneList .scene', { hasText: 'Oops' })).toHaveCount(0);
  await page.mouse.up();
  await expect.poll(async () => (await scenes()).some(s => s.name === 'Oops')).toBe(false);
  await expect(page.locator('#undoBtn')).toBeEnabled();
  await expect(page.locator('#undoBtn')).toHaveAttribute('title', /Scène « Oops » supprimée/);

  await focusPage(page);
  await page.keyboard.press('ControlOrMeta+z');
  await expect(page.locator('#lockToast')).toHaveText('Annulé : Scène « Oops » supprimée');
  await expect.poll(async () => (await scenes()).find(s => s.name === 'Oops')).toEqual(saved);
  await expect(page.locator('#sceneList .scene', { hasText: 'Oops' })).toBeVisible();
  await expect(page.locator('#redoBtn')).toBeEnabled();

  await page.keyboard.press('ControlOrMeta+Shift+z');
  await expect.poll(async () => (await scenes()).some(s => s.name === 'Oops')).toBe(false);
});

test('playing a cue is not an edit', async ({ page }) => {
  const before = await history();
  await page.locator('#cues .cue').first().click();
  await expect.poll(async () => (await studio.frame()).active_cue).not.toBeNull();
  expect(await studio.post('/api/control', { id: 'master.reset' })).toBe(200);
  expect(await history()).toEqual(before);
});

test('a click in « Historique » undoes down to that entry, and redo rows come back', async ({ page }) => {
  for (const n of ['Un', 'Deux', 'Trois']) await saveScene(page, n);
  await page.locator('#historyBtn').click();
  await expect(page.locator('#historyPanel')).toBeVisible();
  const rows = page.locator('#historyList .h-row');
  await expect(rows.first()).toContainText('Scène « Trois » enregistrée');
  await expect(rows.nth(1)).toContainText('Scène « Deux » enregistrée');
  await rows.filter({ hasText: 'Scène « Un » enregistrée' }).click();
  await expect.poll(async () => (await scenes()).length).toBe(0);
  await expect(page.locator('#historyRedo .h-row')).toHaveCount(3);
  await expect(page.locator('#historyRedo .h-row').first()).toContainText('Trois');
  // Redo down to « Deux »: Un and Deux are back.
  await page.locator('#historyRedo .h-row', { hasText: 'Deux' }).click();
  await expect.poll(async () => (await scenes()).map(s => s.name)).toEqual(['Un', 'Deux']);
  // A new edit empties what could be redone.
  await page.locator('h1').click();
  await expect(page.locator('#historyPanel')).toBeHidden();
  await saveScene(page, 'Quatre');
  await expect.poll(async () => (await history()).redo).toBe(0);
});

test('show mode refuses undo, and hides the buttons', async ({ page }) => {
  await saveScene(page, 'Garde');
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  await expect(page.locator('#undoBtn')).toBeHidden();
  await focusPage(page);
  await page.keyboard.press('ControlOrMeta+z');
  await expect(page.locator('#lockToast')).toHaveText('Verrouillé en mode spectacle');
  expect(await studio.post('/api/undo')).toBe(423);
  expect((await scenes()).map(s => s.name)).toContain('Garde');
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
});
