// Autosave and crash recovery (T-287): a change is autosaved after the
// delay (1 s here, allowed by --test-hooks), killing the studio then
// starting it again offers « Récupérer », which restores the change and
// stays disarmed; an unwritable autosave folder only shows a warning.
// Preview only, temporary --data-dir.
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi, reveal, Studio } from '../studio';

const prefs = JSON.stringify({ autosave: { delay_s: 1 } });
const studio = useStudio({ testHooks: true, files: { 'prefs.json': prefs } });

const project = () => studio.get('/api/project');
const bpm = async () => (await studio.frame()).tempo.bpm;
const autosaves = (s: Studio) => {
  try { return readdirSync(path.join(s.dataDir, 'autosave')).filter(f => /^Sans titre-\d{8}-\d{6}\.lsproj$/.test(f)); } catch { return []; }
};

test.describe.configure({ mode: 'serial' });

test('a change is autosaved after the delay and the status line says when', async ({ page }) => {
  await openUi(page, studio);
  expect((await project()).autosave).toMatchObject({ enabled: true, delay_s: 1, error: null, offer: null });
  expect(autosaves(studio)).toEqual([]);
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 133 })).toBe(200);
  await expect.poll(() => autosaves(studio).length, { timeout: 10_000 }).toBeGreaterThan(0);
  const file = path.join(studio.dataDir, 'autosave', autosaves(studio)[0]);
  expect(JSON.parse(readFileSync(file, 'utf8')).tempo.bpm).toBe(133);
  await expect(page.locator('#autosaveStatus')).toHaveText(/^Sauvegarde auto \d\d:\d\d$/, { timeout: 5_000 });
  await expect(page.locator('#autosaveStatus')).not.toHaveClass(/err/);
});

test('killing the studio then starting again offers « Récupérer », which restores the change, disarmed', async ({ page }) => {
  await studio.kill();
  await studio.start();
  expect(await bpm()).toBe(120); // the tempo is not in the working copy: lost without recovery
  expect((await project()).autosave.offer).toMatchObject({ project: 'Sans titre' });
  await page.goto(studio.url + '/');
  await expect(page.locator('#recoverAsk')).toBeVisible();
  await expect(page.locator('#recoverAsk h3')).toHaveText('Récupérer le travail non enregistré ?');
  await expect(page.locator('#recoverWhen')).toContainText('Sans titre');
  await page.locator('#recoverOk').click();
  await expect(page.locator('#recoverAsk')).toBeHidden();
  await expect.poll(bpm).toBe(133);
  await expect(page.locator('#projName')).toHaveText('Sans titre •');
  expect((await project()).autosave.offer).toBeNull();
  const st = await studio.state();
  expect(st.armed).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
});

test('« Ignorer » keeps the working copy and is not asked again', async ({ page }) => {
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 141 })).toBe(200);
  await expect.poll(async () => {
    const files = autosaves(studio).sort();
    const last = files.at(-1);
    return last ? JSON.parse(readFileSync(path.join(studio.dataDir, 'autosave', last), 'utf8')).tempo.bpm : null;
  }, { timeout: 10_000 }).toBe(141);
  await studio.kill();
  await studio.start();
  await page.goto(studio.url + '/');
  await expect(page.locator('#recoverAsk')).toBeVisible();
  await page.locator('#recoverIgnore').click();
  await expect(page.locator('#recoverAsk')).toBeHidden();
  expect(await bpm()).toBe(120);
  await studio.kill();
  await studio.start();
  expect((await project()).autosave.offer).toBeNull();
  expect((await studio.state()).armed).toBe(false);
});

test('RÉGLAGES › Sauvegarde auto turns it off and sets the delay', async ({ page }) => {
  await openUi(page, studio);
  await reveal(page, '#autosavePanel > summary');
  await page.locator('#autosavePanel > summary').click();
  await expect(page.locator('#asEnabled')).toBeChecked();
  await expect(page.locator('#asDelay')).toHaveValue('1');
  await page.locator('#asDelay').fill('45');
  await page.locator('#asDelay').press('Tab');
  await expect.poll(async () => (await project()).autosave.delay_s).toBe(45);
  await page.locator('#asEnabled').uncheck();
  await expect.poll(async () => (await project()).autosave.enabled).toBe(false);
  await expect(page.locator('#asDelay')).toBeDisabled();
  const saved = JSON.parse(readFileSync(path.join(studio.dataDir, 'prefs.json'), 'utf8'));
  expect(saved.autosave).toMatchObject({ enabled: false, delay_s: 45 });
  await page.locator('#asEnabled').check();
  await expect.poll(async () => (await project()).autosave.enabled).toBe(true);
});

test('an unwritable autosave folder shows an orange warning and the studio keeps running', async ({ page }) => {
  // A file where the folder should be: every autosave fails.
  const broken = new Studio({ testHooks: true, files: { 'prefs.json': prefs, autosave: 'pas un dossier' } });
  try {
    await broken.start();
    await page.goto(broken.url + '/');
    expect(await broken.post('/api/control', { id: 'tempo.bpm', value: 150 })).toBe(200);
    await expect.poll(async () => (await broken.get('/api/project')).autosave.error, { timeout: 10_000 }).toBeTruthy();
    await expect(page.locator('#autosaveStatus')).toHaveText('⚠ Échec de la sauvegarde auto', { timeout: 5_000 });
    await expect(page.locator('#autosaveStatus')).toHaveClass(/err/);
    // Still running: the engine draws and the API answers.
    expect((await broken.frame()).points.length).toBeGreaterThan(0);
    expect((await broken.state()).armed).toBe(false);
  } finally {
    await broken.stop();
    broken.cleanup();
  }
});
