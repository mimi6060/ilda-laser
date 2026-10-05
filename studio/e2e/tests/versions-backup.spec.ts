// Named versions and format versions (T-288), copy to a second location
// (T-450): save a version and open it as a copy without touching the
// project; a project from a newer version opens read only; with a backup
// folder every save is written there too, identical; a missing folder
// warns and the save still works; low disk space shows a banner.
// Preview only, temporary --data-dir and backup folder.
import { existsSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test, expect, useStudio, openUi, reveal } from '../studio';

const future = JSON.stringify({ format_version: 99, name: 'Futur', scenes: [{ name: 'Du futur', settings: {}, duration_secs: 4 }], section_du_futur: { x: 1 } });
const studio = useStudio({ files: { 'projects/Futur.lsproj': future } });

const project = () => studio.get('/api/project');
const projectFile = (name: string) => path.join(studio.dataDir, 'projects', `${name}.lsproj`);
const sceneNames = async () => (await studio.state()).scenes.map((s: { name: string }) => s.name);

test.describe.configure({ mode: 'serial' });

test('the data folder gets a format manifest', async () => {
  const m = JSON.parse(readFileSync(path.join(studio.dataDir, 'format.json'), 'utf8'));
  expect(m.format_version).toBe(1);
  expect(m.files['scenes.json']).toBe(1);
  expect(m.files['safety.json']).toBe(1);
});

test('« Enregistrer une version… » then « Ouvrir comme copie » leaves the project untouched, disarmed', async ({ page }) => {
  await openUi(page, studio);
  expect(await studio.post('/api/scenes/save', { name: 'Avant', duration_secs: 5 })).toBe(200);
  expect(await studio.post('/api/project/save-as', { path: 'Festival' })).toBe(200);
  await expect(page.locator('#projName')).toHaveText('Festival');

  await page.locator('#projBtn').click();
  await page.locator('[data-proj="version-save"]').click();
  await expect(page.locator('#verDialog')).toBeVisible();
  await page.locator('#verName').fill('avant festival');
  await page.locator('#verSave').click();
  await expect(page.locator('#verList .ver')).toHaveCount(1);
  await expect(page.locator('#verList .ver .n')).toHaveText('avant festival');
  await page.locator('#verClose').click();

  // Changed and saved after the version.
  expect(await studio.post('/api/scenes/save', { name: 'Après', duration_secs: 5 })).toBe(200);
  expect(await studio.post('/api/project/save', {})).toBe(200);
  const saved = readFileSync(projectFile('Festival'));
  expect(await sceneNames()).toContain('Après');

  await page.locator('#projBtn').click();
  await page.locator('[data-proj="versions"]').click();
  await page.locator('#verList .ver [data-ver-open]').click();
  await expect(page.locator('#projName')).toHaveText('Sans titre •');
  await expect.poll(sceneNames).not.toContain('Après');
  expect(await sceneNames()).toContain('Avant');
  expect(readFileSync(projectFile('Festival')).equals(saved)).toBe(true);
  expect((await studio.state()).armed).toBe(false);
  // « Enregistrer sous… » offers a name for the copy.
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="save-as"]').click();
  await expect(page.locator('#projPath')).toHaveValue('Festival avant festival');
  await page.locator('#projCancel').click();
});

test('a project from a newer version opens read only, « Enregistrer » disabled', async ({ page }) => {
  await openUi(page, studio);
  const before = readFileSync(projectFile('Futur'));
  expect(await studio.post('/api/project/open', { path: 'Futur' })).toBe(200);
  await expect(page.locator('#projName')).toContainText('(lecture seule)');
  expect(await project()).toMatchObject({ read_only: true, file_format: 99 });
  await page.locator('#projBtn').click();
  await expect(page.locator('[data-proj="save"]')).toBeDisabled();
  await page.keyboard.press('Escape');
  expect(await studio.post('/api/project/save', {})).toBe(409);
  expect(readFileSync(projectFile('Futur')).equals(before)).toBe(true);
  expect((await studio.state()).armed).toBe(false);
});

test('with a backup folder every save is copied there, identical; a missing folder warns, the save still works', async ({ page }) => {
  const parent = mkdtempSync(path.join(tmpdir(), 'laser-studio-e2e-copie-'));
  try {
    await openUi(page, studio);
    expect(await studio.post('/api/project/save-as', { path: 'Copie test' })).toBe(200);
    await reveal(page, '#backupPanel > summary');
    await page.locator('#backupPanel > summary').click();
    // Inside the data folder: refused.
    await page.locator('#bkDir').fill(studio.dataDir);
    await page.locator('#bkDir').press('Enter');
    await expect(page.locator('#bkError')).toHaveText('Doit être hors du dossier de données');
    await page.locator('#bkDir').fill(path.join(parent, 'absent'));
    await page.locator('#bkDir').press('Enter');
    await expect(page.locator('#bkError')).toHaveText('Dossier introuvable');
    await page.locator('#bkDir').fill(parent);
    await page.locator('#bkDir').press('Enter');
    await expect(page.locator('#bkError')).toHaveText('');
    await page.locator('#bkEnabled').check();
    await expect.poll(async () => (await studio.get('/api/backup')).settings.enabled).toBe(true);

    expect(await studio.post('/api/project/save', {})).toBe(200);
    const copy = path.join(realDir(parent), 'Laser Studio', 'Copie test.lsproj');
    await expect.poll(() => existsSync(copy)).toBe(true);
    expect(readFileSync(copy).equals(readFileSync(projectFile('Copie test')))).toBe(true);
    await expect(page.locator('#backupStatus')).toHaveText(/^Copie \d\d:\d\d$/, { timeout: 5_000 });
    await page.locator('#bkNow').click();
    await expect(page.locator('#bkResult')).toHaveText(/^Copie faite à \d\d:\d\d$/);

    // The folder goes (a key pulled out): saving works, an orange warning stays.
    rmSync(parent, { recursive: true, force: true });
    expect(await studio.post('/api/scenes/save', { name: 'Sans clé', duration_secs: 5 })).toBe(200);
    expect(await studio.post('/api/project/save', {})).toBe(200);
    expect(JSON.parse(readFileSync(projectFile('Copie test'), 'utf8')).scenes.map((s: { name: string }) => s.name)).toContain('Sans clé');
    await expect(page.locator('#backupStatus')).toHaveText('⚠ Copie de sauvegarde indisponible', { timeout: 5_000 });
    await expect(page.locator('#backupStatus')).toHaveClass(/err/);
    expect(existsSync(parent)).toBe(false); // never recreated
    expect((await studio.state()).storage.backup_ok).toBe(false);
  } finally {
    rmSync(parent, { recursive: true, force: true });
  }
});

test('low disk space (threshold raised) shows a banner that can be closed', async ({ page }) => {
  const parent = mkdtempSync(path.join(tmpdir(), 'laser-studio-e2e-copie-'));
  try {
    await openUi(page, studio);
    expect(await studio.post('/api/backup', { enabled: true, dir: parent, min_free_gb: 100000 })).toBe(200);
    await expect(page.locator('#spaceWarn')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('#spaceWarnText')).toContainText('Espace disque faible');
    await page.locator('#spaceWarnClose').click();
    await expect(page.locator('#spaceWarn')).toBeHidden();
    await expect(page.locator('#backupStatus')).toHaveText('⚠ Espace disque faible');
    expect(await studio.post('/api/backup', { enabled: false })).toBe(200);
    await expect(page.locator('#backupStatus')).toBeHidden({ timeout: 5_000 });
    expect(readdirSync(parent)).toEqual([]);
  } finally {
    rmSync(parent, { recursive: true, force: true });
  }
});

/** macOS temp folders are symlinks (/var → /private/var): the studio stores the real path. */
function realDir(p: string) {
  return realpathSync(p);
}
