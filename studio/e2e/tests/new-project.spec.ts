// « Nouveau projet » (T-451): choose what is taken from the open project;
// calibration and safety always stay (byte for byte); templates give an
// untitled copy; never arms. Preview only, temporary --data-dir.
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi } from '../studio';

const studio = useStudio();

const project = () => studio.get('/api/project');
const sceneNames = async () => (await studio.state()).scenes.map((s: { name: string }) => s.name);
const palettes = async () => (await studio.get('/api/palettes')).user as { name: string }[];
const machineFile = (name: string) => path.join(studio.dataDir, name);

test('Nouveau projet… with only « Palettes » ticked keeps the palette, not the scene', async ({ page }) => {
  expect(await studio.post('/api/calibration', { offset_x: 0.15, offset_y: 0, scale_x: 1, scale_y: 1, rotation_deg: 0 })).toBe(200);
  // The safety settings as they are (written to safety.json): never loosened.
  expect(await studio.post('/api/safety', (await studio.get('/api/safety')).settings)).toBe(200);
  expect(await studio.post('/api/palettes', [{ name: 'Maison', colors: [[255, 0, 0], [0, 0, 255]] }])).toBe(200);
  expect(await studio.post('/api/scenes/save', { name: 'Intro', duration_secs: 4 })).toBe(200);
  const before = ['calibration.json', 'safety.json'].map(f => existsSync(machineFile(f)) ? readFileSync(machineFile(f)) : null);
  expect(before.every(b => b !== null)).toBe(true);

  await openUi(page, studio);
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="new"]').click();
  // The open project is modified: the existing question comes first.
  await expect(page.locator('#projAsk')).toBeVisible();
  await page.locator('#askDiscard').click();
  const dlg = page.locator('#newProjDialog');
  await expect(dlg).toBeVisible();
  await expect(dlg.locator('h3')).toHaveText('Nouveau projet');
  await expect(dlg).toContainText('Toujours conservés : calibration, sécurité, sorties (réglages de cette machine)');
  // The page's defaults: MIDI, palettes and venue ticked, pages not.
  await expect(dlg.locator('input[value="midi"]')).toBeChecked();
  await expect(dlg.locator('input[value="palettes"]')).toBeChecked();
  await expect(dlg.locator('input[value="pages"]')).not.toBeChecked();
  await expect(dlg.locator('input[value="venue"]')).toBeChecked();
  await dlg.locator('input[value="midi"]').uncheck();
  await dlg.locator('input[value="venue"]').uncheck();
  await page.locator('#newProjOk').click();
  await expect(dlg).toBeHidden();

  expect(await sceneNames()).not.toContain('Intro');
  expect((await palettes()).map(p => p.name)).toContain('Maison');
  expect(await project()).toMatchObject({ name: 'Sans titre', file: null });
  expect((await studio.state()).armed).toBe(false);
  const after = ['calibration.json', 'safety.json'].map(f => existsSync(machineFile(f)) ? readFileSync(machineFile(f)) : null);
  expect(after).toEqual(before);
  expect((await studio.state()).calibration.offset_x).toBeCloseTo(0.15, 5);

  // The choice is remembered by the page.
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="new"]').click();
  await expect(dlg.locator('input[value="midi"]')).not.toBeChecked();
  await expect(dlg.locator('input[value="palettes"]')).toBeChecked();
  await page.locator('#newProjCancel').click();
  await expect(dlg).toBeHidden();
});

test('an empty body keeps nothing, as before', async () => {
  expect(await studio.post('/api/palettes', [{ name: 'Seule', colors: [[0, 255, 0]] }])).toBe(200);
  expect(await studio.post('/api/project/new')).toBe(200);
  expect((await palettes()).map(p => p.name)).not.toContain('Seule');
});

test('save as template, then new from it: same content, untitled', async ({ page }) => {
  expect(await studio.post('/api/project/new')).toBe(200);
  expect(await studio.post('/api/scenes/save', { name: 'Modèle A', duration_secs: 5 })).toBe(200);
  expect(await studio.post('/api/palettes', [{ name: 'Tournée', colors: [[255, 255, 0]] }])).toBe(200);

  await openUi(page, studio);
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="save-template"]').click();
  await expect(page.locator('#tplDialog')).toBeVisible();
  await page.locator('#tplName').fill('Tournée');
  await page.locator('#tplOk').click();
  await expect(page.locator('#tplDialog')).toBeHidden();
  expect(existsSync(path.join(studio.dataDir, 'templates', 'Tournée.lsproj'))).toBe(true);
  expect((await studio.get('/api/project/templates')).templates).toEqual(['Tournée']);

  // Same name again: asked first.
  page.once('dialog', d => { expect(d.message()).toBe('Un modèle porte déjà ce nom : le remplacer ?'); d.accept(); });
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="save-template"]').click();
  await page.locator('#tplName').fill('Tournée');
  await page.locator('#tplOk').click();
  await expect(page.locator('#tplDialog')).toBeHidden();

  // Something else, then « Nouveau depuis un modèle ».
  expect(await studio.post('/api/project/new')).toBe(200);
  expect(await sceneNames()).not.toContain('Modèle A');
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="new-template"]').click();
  await page.locator('#projTemplates [data-template="Tournée"]').click();
  await expect.poll(sceneNames).toContain('Modèle A');
  expect((await palettes()).map(p => p.name)).toContain('Tournée');
  expect(await project()).toMatchObject({ name: 'Sans titre', file: null });
  expect((await studio.state()).armed).toBe(false);

  // Deleting, with confirmation.
  page.once('dialog', d => d.accept());
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="new-template"]').click();
  await page.locator('#projTemplates [data-template-delete="Tournée"]').click();
  await expect(page.locator('#projTemplates')).toContainText('Aucun modèle');
  expect((await studio.get('/api/project/templates')).templates).toEqual([]);
});

test('template names with ../ are refused, unknown ones are 404', async () => {
  for (const bad of ['../x', '../projects/y', 'a/b']) {
    const r = await fetch(`${studio.url}/api/project/save-template`, { method: 'POST', body: JSON.stringify({ name: bad }) });
    expect(r.status).toBe(400);
  }
  const r = await fetch(`${studio.url}/api/project/new`, { method: 'POST', body: JSON.stringify({ template: 'Inconnu' }) });
  expect(r.status).toBe(404);
});
