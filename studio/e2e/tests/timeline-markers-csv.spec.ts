// Timeline markers as CSV (T-443): export the loaded show's markers (a
// download), import an audio editor's marker list in *Ajouter* then in
// *Remplacer*, and one Cmd+Z undoes an import. The sample file is our own,
// written by hand. Preview only: nothing here arms the laser.
import { readFileSync } from 'node:fs';
import { test, expect, useStudio, openUi, openWorkspace, type Page } from '../studio';

const studio = useStudio();

interface Marker { at: number; name: string; color: number[] }
const serverMarkers = async () => (await studio.get<{ show: { markers: Marker[] } | null }>('/api/timeline')).show!.markers;
const summary = (ms: Marker[]) => ms.map(m => `${m.at.toFixed(3)} ${m.name}`);

test.beforeEach(async ({ page }) => {
  await studio.post('/api/timeline/stop');
  await studio.post('/api/estop/reset');
  await studio.reset();
  await openUi(page, studio);
});

test.afterEach(async () => {
  expect((await studio.state()).armed, 'importing markers never arms').toBe(false);
});

async function seed(name: string) {
  expect(await studio.post('/api/timeline/new', { name, time_base: 'seconds' })).toBe(200);
  const { show } = await studio.get('/api/timeline');
  show.markers = [
    { at: 2, name: 'Intro', color: [255, 176, 32] },
    { at: 10.5, name: 'Drop, « fort »', color: [77, 163, 255] },
  ];
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
}

async function openEditor(page: Page, name: string) {
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe(name);
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.markers.length)).toBe(2);
}

async function importCsv(page: Page, name: string, text: string) {
  await page.locator('#tlCsvBtn').click();
  await page.locator('#tlCsvImport').click();
  await page.locator('#tlCsvFile').setInputFiles({ name, mimeType: 'text/csv', buffer: Buffer.from(text, 'utf8') });
  await expect(page.locator('#tlCsvDlg')).toBeVisible();
}

// First: this file's studio has no show loaded yet.
test('the Marqueurs menu is greyed without a show', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  const loaded = (await studio.get('/api/timeline')).show;
  test.skip(!!loaded, 'a show is already loaded');
  await expect(page.locator('#tlCsvBtn')).toBeDisabled();
});

// A DAW's marker list (our own sample): `;` separator, decimal comma, BOM,
// \r\n, a region with an end and a length, a colour column left empty,
// a bar-based time and a line that can't be read.
const DAW = '\uFEFF#;Name;Start;End;Length;Color\r\n'
  + 'M1;Couplet <b>1</b>;4,000;;;\r\n'
  + 'R1;Refrain;20,250;36,250;16,000;00FF00\r\n'
  + 'M2;Pont;5.1.00;;;\r\n'
  + 'M3;Intro;2,0004;;;\r\n'
  + 'M4;Fin;xx;;;\r\n';

test('export downloads the markers, import adds then replaces, Cmd+Z undoes', async ({ page }) => {
  await seed('Marqueurs CSV e2e');
  await openEditor(page, 'Marqueurs CSV e2e');

  // Export: the download is the show's markers in seconds.
  await page.locator('#tlCsvBtn').click();
  const [download] = await Promise.all([page.waitForEvent('download'), page.locator('#tlCsvExport').click()]);
  expect(download.suggestedFilename()).toBe('Marqueurs CSV e2e-marqueurs.csv');
  const exported = readFileSync(await download.path(), 'utf8');
  expect(exported).toBe('#,Name,Start,Color\r\nM1,Intro,2.000,FFB020\r\nM2,"Drop, « fort »",10.500,4DA3FF\r\n');
  const original = await serverMarkers();

  // Ajouter: two new markers, the duplicate « Intro » left out, two lines reported.
  await importCsv(page, 'daw.csv', DAW);
  await expect(page.locator('#tlCsvSummary')).toHaveText('« daw.csv » : 3 marqueurs lus · 2 lignes ignorées');
  await page.locator('#tlCsvSkipped summary').click();
  await expect(page.locator('#tlCsvSkippedList li')).toHaveText(['ligne 4 : temps en mesures : exporter en secondes', 'ligne 6 : temps illisible']);
  expect(await page.locator('#tlCsvSkippedList b').count(), 'names are text, never markup').toBe(0);
  await page.locator('#tlCsvAppend').click();
  await expect(page.locator('#tlEdMsg')).toContainText('2 marqueurs importés');
  await expect.poll(async () => summary(await serverMarkers())).toEqual(['2.000 Intro', '4.000 Couplet <b>1</b>', '10.500 Drop, « fort »', '20.250 Refrain']);
  const refrain = (await serverMarkers()).find(m => m.name === 'Refrain')!;
  expect(refrain.color).toEqual([0, 255, 0]);

  // One Cmd+Z: back to the two markers.
  await page.locator('#tlUndo').click();
  await expect.poll(async () => summary(await serverMarkers())).toEqual(summary(original));

  // Re-import our own export in *Remplacer*: the same list, to the millisecond.
  await importCsv(page, 'retour.csv', exported);
  await expect(page.locator('#tlCsvSummary')).toHaveText('« retour.csv » : 2 marqueurs lus');
  await expect(page.locator('#tlCsvSkipped')).toBeHidden();
  await page.locator('#tlCsvReplace').click();
  await expect(page.locator('#tlEdMsg')).toContainText('2 marqueurs importés (Cmd+Z pour annuler)');
  const back = await serverMarkers();
  expect(back.map(m => [m.name, m.color])).toEqual(original.map(m => [m.name, m.color]));
  back.forEach((m, i) => expect(Math.abs(m.at - original[i].at)).toBeLessThan(0.001));

  // *Remplacer* with the DAW file, then Cmd+Z from the keyboard.
  await importCsv(page, 'daw.csv', DAW);
  await page.locator('#tlCsvReplace').click();
  await expect.poll(async () => summary(await serverMarkers())).toEqual(['2.000 Intro', '4.000 Couplet <b>1</b>', '20.250 Refrain']);
  await page.locator('#tlCanvas').focus();
  await page.keyboard.press('ControlOrMeta+z');
  await expect.poll(async () => summary(await serverMarkers())).toEqual(summary(original));
});

test('a binary file or a cancelled import leaves the show alone', async ({ page }) => {
  await seed('Marqueurs refus e2e');
  await openEditor(page, 'Marqueurs refus e2e');
  const before = await serverMarkers();

  await page.locator('#tlCsvBtn').click();
  await page.locator('#tlCsvImport').click();
  await page.locator('#tlCsvFile').setInputFiles({ name: 'image.csv', mimeType: 'text/csv', buffer: Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 13, 1, 2]) });
  await expect(page.locator('#tlEdMsg')).toContainText('fichier binaire');
  await expect(page.locator('#tlCsvDlg')).toBeHidden();

  await importCsv(page, 'annule.csv', 'Name,Start\nA,1\nB,2\n');
  await page.locator('#tlCsvCancel').click();
  await expect(page.locator('#tlCsvDlg')).toBeHidden();
  expect(await serverMarkers()).toEqual(before);
  await expect(page.locator('#tlUndo')).toBeDisabled();
});
