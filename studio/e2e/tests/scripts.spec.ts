// Scripts (T-458): edited and run in CRÉATION › Scripts; a script changes
// the grid page through a command line; « armer » is refused and never
// arms; a loop is stopped by its budget; a project brought from elsewhere
// opens with its scripts disabled until approved. Preview only: nothing
// is armed, no device.
import { test, expect, useStudio, openUi, reveal } from '../studio';
import type { Page } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';

const studio = useStudio();
const page1 = async () => (await studio.controlValues()).cue_page;
const armed = async () => (await studio.state()).armed as boolean;

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/lock', { show_mode: false });
  await studio.post('/api/scripts/stop', {});
  expect(await studio.post('/api/scripts', { scripts: [] })).toBe(200);
  await studio.post('/api/scripts/run', {});
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

/** CRÉATION › Scripts: a new script with this source, saved. */
async function createScript(page: Page, name: string, source: string) {
  await reveal(page, '#scrAdd');
  await page.locator('#scrAdd').click();
  await page.locator('#scrName').fill(name);
  await page.locator('#scrSource').fill(source);
}

test('a script reads the state and changes the grid page; Cmd+Entrée runs it', async ({ page }) => {
  await createScript(page, 'Page 2', 'let e = etat()\njournal("page " + e.page)\ncommande("page 2")');
  await page.locator('#scrRun').click();
  await expect.poll(page1).toBe(2);
  await expect(page.locator('#scrConsole')).toContainText('page 1');
  await expect(page.locator('#scrList .scr-item', { hasText: 'Page 2' })).toContainText('prêt');
  // Again from the editor, with the keyboard.
  await studio.post('/api/control', { id: 'page.1' });
  await expect.poll(page1).toBe(1);
  await page.locator('#scrSource').focus();
  await page.keyboard.press('ControlOrMeta+Enter');
  await expect.poll(page1).toBe(2);
  expect(await armed()).toBe(false);
});

test('« armer » is refused with its line, at save and at run time; nothing arms', async ({ page }) => {
  await createScript(page, 'Armer', 'journal("x")\ncommande("armer")');
  await page.locator('#scrSave').click();
  await expect(page.locator('#scrErr')).toContainText('ligne 2 : commande refusée « armer »');
  await expect(page.locator('#scrState')).toContainText('Non enregistré');
  // Built at run time: saved, refused in the run, nothing applied.
  await page.locator('#scrSource').fill('commande("page 2")\ncommande("ar" + "mer")');
  await page.locator('#scrRun').click();
  await expect(page.locator('#scrErr')).toContainText('Ligne 2 : commande refusée « armer »');
  expect(await page1()).toBe(1);
  expect(await armed()).toBe(false);
});

test('a loop is stopped by its budget and the studio keeps running', async ({ page }) => {
  await createScript(page, 'Boucle', 'while true {}');
  await page.locator('#scrRun').click();
  await expect(page.locator('#scrList .scr-item', { hasText: 'Boucle' })).toContainText('arrêté : budget dépassé');
  await expect(page.locator('#scrErr')).toContainText('budget');
  // Another script still runs.
  await page.locator('#scrAdd').click();
  await page.locator('#scrName').fill('Bon');
  await page.locator('#scrSource').fill('commande("page 2")');
  await page.locator('#scrRun').click();
  await expect.poll(page1).toBe(2);
  expect(await armed()).toBe(false);
});

test('a project from elsewhere opens with its scripts disabled until approved', async ({ page }) => {
  await createScript(page, 'Page 2', 'commande("page 2")');
  await page.locator('#scrSave').click();
  await expect(page.locator('#scrState')).toContainText('Scripts enregistrés');
  expect(await studio.post('/api/project/save-as', { path: 'Scripts e2e' })).toBe(200);
  // The file changed outside this studio: another list.
  const file = path.join(studio.dataDir, 'projects', 'Scripts e2e.lsproj');
  const p = JSON.parse(fs.readFileSync(file, 'utf8'));
  p.scripts[0].source = 'journal("autre")\ncommande("page 2")';
  fs.writeFileSync(file, JSON.stringify(p));
  expect(await studio.post('/api/project/open', { path: 'Scripts e2e' })).toBe(200);
  await expect(page.locator('#scrTrustBanner')).toBeVisible();
  await expect(page.locator('#scrTrustText')).toHaveText('Ce projet contient 1 script. Ils sont désactivés.');
  const r = await fetch(studio.url + '/api/scripts');
  const list = (await r.json()) as { scripts: { id: number }[]; trusted: boolean };
  expect(list.trusted).toBe(false);
  expect(await studio.post('/api/scripts/run', { id: list.scripts[0].id })).toBe(409);
  await page.locator('#scrTrustYes').click();
  await expect(page.locator('#scrTrustBanner')).toBeHidden();
  await studio.post('/api/control', { id: 'page.1' });
  await expect.poll(page1).toBe(1);
  expect(await studio.post('/api/scripts/run', { id: list.scripts[0].id })).toBe(200);
  await expect.poll(page1).toBe(2);
  expect(await armed()).toBe(false);
});
