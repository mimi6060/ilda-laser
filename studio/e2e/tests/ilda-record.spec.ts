// Recording as an ILDA animation (T-442): what plays live for 2 s at
// 25 images/s, and a whole show rendered offline (twice: the same bytes,
// calibration and output safety chain baked in). The files land in the
// ILDA library and download from the dialog. Show mode refuses a start.
// Preview only: the laser stays disarmed throughout.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import type { Page } from '@playwright/test';
import { readFileSync } from 'node:fs';

const studio = useStudio();

/** Image sections of an ILDA format 5 file (until the 0-record header). */
function sections(bytes: Buffer): number {
  let at = 0, n = 0;
  while (at + 32 <= bytes.length) {
    expect(bytes.subarray(at, at + 4).toString('ascii')).toBe('ILDA');
    expect(bytes[at + 7]).toBe(5);
    const records = bytes.readUInt16BE(at + 24);
    if (records === 0) break;
    at += 32 + records * 8;
    n++;
  }
  return n;
}

async function recordAndDownload(page: Page, name: string): Promise<Buffer> {
  await page.locator('#recName').fill(name);
  await page.locator('#recStart').click();
  await expect(page.locator('#recMsg')).toContainText('Fichier prêt', { timeout: 20_000 });
  await expect(page.locator('#recMsg')).toContainText(`${name}.ild`);
  const download = page.waitForEvent('download');
  await page.locator('#recDownload').click();
  const file = await download;
  expect(file.suggestedFilename()).toBe(`${name}.ild`);
  return readFileSync((await file.path())!);
}

test.beforeAll(async () => {
  const catalog = await studio.presets();
  const show = {
    name: 'Enregistrement e2e',
    time_base: 'seconds',
    tempo_map: [{ at_s: 0, bpm: 120, beats_per_bar: 4 }],
    tracks: [{ name: 'Piste 1', layer: 1, events: [
      { id: 1, start: 0, len: 1, source: { kind: 'cue', id: catalog.presets[0].id } },
      { id: 2, start: 1, len: 1, source: { kind: 'cue', id: catalog.presets[1].id } },
    ] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
});

test.beforeEach(async () => {
  await studio.post('/api/lock', { show_mode: false });
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
});

test('2 s of what plays live, at 25 images/s: 50 images in the library, never armed', async ({ page }) => {
  const catalog = await studio.presets();
  expect(await studio.post('/api/presets/play', { id: catalog.presets[0].id })).toBe(200);
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="media"]').click();
  await page.locator('#ildaRecord').click();
  await expect(page.locator('#recDialog')).toBeVisible();
  await expect(page.locator('#recSource')).toHaveValue('live');
  // The safety note is always there, and never calls the file safe.
  await expect(page.locator('#recSafety')).toContainText('chaîne de sécurité');
  await expect(page.locator('#recSafety')).toContainText('arrêt d\'urgence matériel');
  await page.locator('#recSeconds').fill('2');
  await page.locator('#recFps').fill('25');
  const armed: boolean[] = [];
  const watch = setInterval(async () => { try { armed.push((await studio.frame()).armed); } catch { /* closing */ } }, 100);
  let bytes: Buffer;
  try {
    bytes = await recordAndDownload(page, 'e2e direct');
  } finally {
    clearInterval(watch);
  }
  await expect(page.locator('#recMsg')).toContainText('50 images');
  expect(sections(bytes)).toBe(50);
  expect(bytes.length).toBeGreaterThan(50 * 32);
  expect(armed.length).toBeGreaterThan(0);
  expect(armed.every(a => a === false)).toBe(true);
  expect((await studio.frame()).armed).toBe(false);
  // In the library, as a cue of the « ILDA » page.
  await page.locator('#recClose').click();
  await expect(page.locator('#ildaLib .ildaitem[data-file="e2e direct.ild"]')).toBeVisible();
  const lib = await studio.get('/api/media/ilda');
  expect(lib.files.find((f: { file: string }) => f.file === 'e2e direct.ild').frames).toBe(50);
});

test('a show exported twice gives the same bytes, calibration baked in; show mode refuses a start', async ({ page }) => {
  expect(await studio.post('/api/calibration', { offset_x: 0.2, offset_y: 0, scale_x: 1, scale_y: 1, rotation_deg: 0 })).toBe(200);
  try {
    await openUi(page, studio);
    await openWorkspace(page, 'timeline');
    await page.locator('#tlShow').selectOption('Enregistrement e2e');
    await expect(page.locator('#tlRecord')).toBeEnabled();
    await page.locator('#tlRecord').click();
    await expect(page.locator('#recDialog')).toBeVisible();
    await expect(page.locator('#recSource')).toHaveValue('timeline');
    await expect(page.locator('#recSeconds')).toBeDisabled();
    await expect(page.locator('#recSeconds')).toHaveValue('2.0');
    const a = await recordAndDownload(page, 'e2e show a');
    const b = await recordAndDownload(page, 'e2e show b');
    expect(sections(a)).toBe(50);
    expect(a.equals(b)).toBe(true);
    expect((await studio.frame()).armed).toBe(false);

    // Show mode: a recording is a file write, refused with the reason.
    expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
    await page.locator('#recName').fill('e2e verrou');
    await page.locator('#recStart').click();
    await expect(page.locator('#recMsg')).toContainText('mode spectacle');
    await expect(page.locator('#recMsg')).toHaveClass(/err/);
    const lib = await studio.get('/api/media/ilda');
    expect(lib.files.some((f: { file: string }) => f.file === 'e2e verrou.ild')).toBe(false);
  } finally {
    await studio.post('/api/lock', { show_mode: false });
    await studio.post('/api/calibration', { offset_x: 0, offset_y: 0, scale_x: 1, scale_y: 1, rotation_deg: 0 });
  }
});

test('invalid parameters are refused with a French message', async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="media"]').click();
  await page.locator('#ildaRecord').click();
  await page.locator('#recSeconds').fill('700');
  await page.locator('#recStart').click();
  await expect(page.locator('#recMsg')).toContainText('Durée');
  await page.locator('#recSeconds').fill('1');
  await page.locator('#recName').fill('../ailleurs');
  await page.locator('#recStart').click();
  await expect(page.locator('#recMsg')).toContainText('Nom de fichier invalide');
  await page.locator('#recName').fill('ok');
  await page.locator('#recSd').fill('230');
  await page.locator('#recStart').click();
  await expect(page.locator('#recMsg')).toContainText('réservés');
  expect((await studio.frame()).armed).toBe(false);
});
