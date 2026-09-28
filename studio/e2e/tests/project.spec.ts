// Project files (T-286): save, change, open restores; unsaved-changes
// prompt; Cmd+S; paths outside the projects folder refused; opening never
// arms nor touches calibration. Preview only, temporary --data-dir.
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi, focusPage } from '../studio';

// Data from before projects existed: imported into « Sans titre » on the first start.
const studio = useStudio({ files: { 'scenes.json': JSON.stringify([{ name: 'Ancienne', settings: {}, duration_secs: 5 }]) } });

const project = () => studio.get('/api/project');
const sceneNames = async () => (await studio.state()).scenes.map((s: { name: string }) => s.name);
const projectFile = (name: string) => path.join(studio.dataDir, 'projects', `${name}.lsproj`);

test('the first start imports scenes.json into « Sans titre » and keeps it', async ({ page }) => {
  expect(existsSync(projectFile('Sans titre'))).toBe(true);
  expect(existsSync(path.join(studio.dataDir, 'scenes.json'))).toBe(true);
  const p = await project();
  expect(p).toMatchObject({ name: 'Sans titre', modified: false, recent: ['Sans titre'] });
  await openUi(page, studio);
  await expect(page.locator('#projName')).toHaveText('Sans titre');
});

test('save as, change, reload the page, open again: everything comes back, still disarmed', async ({ page }) => {
  await openUi(page, studio);
  expect(await studio.post('/api/scenes/save', { name: 'Intro', duration_secs: 6 })).toBe(200);
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 128 })).toBe(200);
  expect(await studio.post('/api/calibration', { offset_x: 0.2, offset_y: 0, scale_x: 1, scale_y: 1, rotation_deg: 0 })).toBe(200);
  await expect(page.locator('#projName')).toHaveText('Sans titre •');

  await page.locator('#projBtn').click();
  await page.locator('[data-proj="save-as"]').click();
  await page.locator('#projPath').fill('Mon show');
  await page.locator('#projOk').click();
  await expect(page.locator('#projDialog')).toBeHidden();
  await expect(page.locator('#projName')).toHaveText('Mon show');
  const saved = JSON.parse(readFileSync(projectFile('Mon show'), 'utf8'));
  expect(saved.format_version).toBe(1);
  expect(saved.playlist).toEqual(['Ancienne', 'Intro']);
  expect(saved.calibration).toBeUndefined();

  // Change things.
  expect(await studio.post('/api/scenes/delete', { name: 'Intro' })).toBe(200);
  expect(await studio.post('/api/scenes/save', { name: 'Autre', duration_secs: 3 })).toBe(200);
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 90 })).toBe(200);
  expect(await studio.post('/api/calibration', { offset_x: -0.3, offset_y: 0, scale_x: 1, scale_y: 1, rotation_deg: 0 })).toBe(200);
  await expect(page.locator('#projName')).toHaveText('Mon show •');

  await page.reload();
  await openUi(page, studio);
  await page.locator('#projBtn').click();
  await page.locator('#projRecent [data-recent="Mon show"]').click();
  await expect(page.locator('#projAsk')).toBeVisible();
  await page.locator('#askDiscard').click();
  await expect(page.locator('#projName')).toHaveText('Mon show');

  expect(await sceneNames()).toEqual(['Ancienne', 'Intro']);
  await expect(page.locator('#sceneList')).toContainText('Intro');
  await expect(page.locator('#sceneList')).not.toContainText('Autre');
  const st = await studio.state();
  expect(st.tempo.bpm).toBe(128);
  expect(st.armed).toBe(false);
  expect(st.calibration.offset_x).toBeCloseTo(-0.3, 5); // calibration is not part of a project
  expect((await project()).modified).toBe(false);
});

test('Cancel in the unsaved-changes prompt keeps the current state', async ({ page }) => {
  await openUi(page, studio);
  expect(await studio.post('/api/scenes/save', { name: 'Pas enregistrée', duration_secs: 3 })).toBe(200);
  await expect(page.locator('#projName')).toHaveText('Mon show •');
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="new"]').click();
  await page.locator('#askCancel').click();
  expect(await sceneNames()).toContain('Pas enregistrée');
  await expect(page.locator('#projName')).toHaveText('Mon show •');
});

test('Cmd+S saves the open project', async ({ page }) => {
  await openUi(page, studio);
  await expect(page.locator('#projName')).toHaveText('Mon show •');
  await focusPage(page);
  await page.keyboard.press('ControlOrMeta+s');
  await expect(page.locator('#projName')).toHaveText('Mon show');
  const saved = JSON.parse(readFileSync(projectFile('Mon show'), 'utf8'));
  expect(saved.scenes.map((s: { name: string }) => s.name)).toContain('Pas enregistrée');
});

test('paths outside the projects folder are refused', async ({ page }) => {
  for (const bad of ['../calibration', '/etc/passwd', 'a/b', '..']) {
    const r = await fetch(`${studio.url}/api/project/open`, { method: 'POST', body: JSON.stringify({ path: bad }) });
    expect(r.status, bad).toBe(400);
  }
  expect(await studio.post('/api/project/save-as', { path: '../scenes' })).toBe(400);
  expect(existsSync(path.join(studio.dataDir, 'scenes.lsproj'))).toBe(false);

  await openUi(page, studio);
  await focusPage(page);
  await page.keyboard.press('ControlOrMeta+o');
  await expect(page.locator('#projDialog')).toBeVisible();
  await expect(page.locator('#projFiles')).toContainText('Mon show');
  await page.locator('#projPath').fill('../../ailleurs');
  await page.locator('#projOk').click();
  await expect(page.locator('#projError')).toContainText('chemin refusé');
  await page.locator('#projCancel').click();
  await expect(page.locator('#projName')).toHaveText('Mon show');
});

test('an invalid project file changes nothing and says why in French', async () => {
  const before = await sceneNames();
  const { writeFileSync } = await import('node:fs');
  writeFileSync(projectFile('Cassé'), JSON.stringify({ format_version: 1, scenes: [{ name: 'X', settings: {}, duration_secs: 1 }, { name: 'X', settings: {}, duration_secs: 1 }] }));
  const r = await fetch(`${studio.url}/api/project/open`, { method: 'POST', body: JSON.stringify({ path: 'Cassé' }) });
  expect(r.status).toBe(400);
  expect(await r.text()).toContain('scène en double');
  expect(await sceneNames()).toEqual(before);
  expect((await project()).name).toBe('Mon show');
  expect((await studio.state()).armed).toBe(false);
});

test('New empties the project, never arms', async () => {
  expect(await studio.post('/api/project/new')).toBe(200);
  const p = await project();
  expect(p).toMatchObject({ name: 'Sans titre', file: null, modified: false });
  expect(await sceneNames()).toEqual([]);
  expect((await studio.state()).armed).toBe(false);
  expect(await studio.post('/api/project/save')).toBe(409);
});
