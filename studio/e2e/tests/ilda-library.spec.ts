// ILDA media library (T-011): CRÉATION › Médiathèque. An ILDA file
// generated here (our own bytes, no third-party file) is imported by drag
// and drop or « choisir », listed, played as a cue of the « ILDA » page
// through the normal look path, kept across a restart, deleted; a damaged
// file is refused (or, found at start-up, set aside with a warning); the
// look on show exports as a format 5 file named for the ShowNET SD card,
// which the studio reads back. Preview only: the laser stays disarmed.
import { test, expect, useStudio, openUi, openWorkspace, isLit, extent, type Studio } from '../studio';
import type { Page } from '@playwright/test';
import { readFileSync } from 'node:fs';

type Pt = { x: number; y: number; r: number; g: number; b: number; blank?: boolean };

/** An ILDA format 5 file (2D, true colour), written from the public spec. */
function ildaFile(frames: Pt[][]): Buffer {
  const header = (records: number, n: number, total: number) => {
    const h = Buffer.alloc(32);
    h.write('ILDA', 0, 'ascii'); h[7] = 5;
    h.write('e2e', 8, 'ascii'); h.write('tests', 16, 'ascii');
    h.writeUInt16BE(records, 24); h.writeUInt16BE(n, 26); h.writeUInt16BE(total, 28);
    return h;
  };
  const parts: Buffer[] = [];
  frames.forEach((pts, n) => {
    parts.push(header(pts.length, n, frames.length));
    pts.forEach((p, i) => {
      const rec = Buffer.alloc(8);
      rec.writeInt16BE(Math.round(p.x * 32767), 0); rec.writeInt16BE(Math.round(p.y * 32767), 2);
      rec[4] = (p.blank ? 0x40 : 0) | (i === pts.length - 1 ? 0x80 : 0);
      rec[5] = p.b; rec[6] = p.g; rec[7] = p.r;
      parts.push(rec);
    });
  });
  parts.push(header(0, frames.length, frames.length));
  return Buffer.concat(parts);
}

// Four images of a green square of half-size 0.5, turning a little each time.
const SQUARE = ildaFile([0, 1, 2, 3].map(k => {
  const a = k * 0.2, c = Math.cos(a) * 0.5, s = Math.sin(a) * 0.5;
  const corners = [[-c + s, -s - c], [c + s, s - c], [c - s, s + c], [-c - s, -s + c]];
  const pts: Pt[] = [{ x: corners[0][0], y: corners[0][1], r: 0, g: 0, b: 0, blank: true }];
  for (const [x, y] of [...corners, corners[0]]) pts.push({ x, y, r: 0, g: 255, b: 0 });
  return pts;
}));

async function openMedia(page: Page, studio: Studio) {
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="media"]').click();
  await expect(page.locator('#ildaMedia')).toBeVisible();
  // The rights reminder is always on screen.
  await expect(page.locator('#ildaRights')).toContainText('dont vous avez les droits');
}

test.describe('médiathèque', () => {
  const studio = useStudio();

  test.beforeEach(async () => {
    await studio.reset();
    await studio.post('/api/control', { id: 'cue.stop_all' });
  });

  test('an imported file is listed, plays as a cue, survives a restart and is deleted, never armed', async ({ page }) => {
    await openMedia(page, studio);
    await expect(page.locator('#ildaLib')).toContainText('Aucun fichier ILDA');

    // A damaged file: a clear message, nothing stored.
    await page.locator('#ildaFile').setInputFiles({ name: 'abime.ild', mimeType: 'application/octet-stream', buffer: Buffer.from('ILDA\0\0\0\x05broken') });
    await expect(page.locator('#ildaMsg')).toContainText('Import impossible');
    await expect(page.locator('#ildaMsg')).toHaveClass(/err/);
    expect((await studio.get('/api/media/ilda')).files).toEqual([]);

    // Dropped on the drop zone like a file from the Finder.
    const drop = await page.evaluateHandle(bytes => {
      const dt = new DataTransfer();
      dt.items.add(new File([new Uint8Array(bytes)], 'Mon carré.ild'));
      return dt;
    }, [...SQUARE]);
    await page.locator('#ildaDrop').dispatchEvent('drop', { dataTransfer: drop });
    await expect(page.locator('#ildaMsg')).toContainText('« Mon carré.ild » importé : 4 images');
    const row = page.locator('#ildaLib .ildaitem[data-file="Mon carré.ild"]');
    await expect(row).toContainText('4 images');

    // A cue of the « ILDA » page.
    const presets = await studio.presets();
    expect(presets.categories).toContain('ILDA');
    expect(presets.presets.find(p => p.id === 'ilda:Mon carré.ild')).toMatchObject({ name: 'Mon carré', category: 'ILDA' });

    // « Jouer »: the look path, the file's own points and colour, disarmed.
    await row.locator('button.play').click();
    await expect(page.locator('#ildaMsg')).toContainText('joué');
    await expect.poll(async () => (await studio.state()).settings.content).toEqual({ kind: 'ilda', file: 'Mon carré.ild', fps: 25 });
    await expect.poll(async () => {
      const f = await studio.frame();
      const lit = f.points.filter(isLit);
      return lit.length > 0 && lit.every(p => p[3] > 0 && p[2] === 0 && p[4] === 0) ? extent(f.points) : 0;
    }).toBeGreaterThan(0.45); // the square at its own size (scale 1)
    const frame = await studio.frame();
    expect(frame.armed).toBe(false);
    expect(extent(frame.points)).toBeLessThanOrEqual(0.71);

    // The look panel shows the file and its rate; the rate is editable.
    await page.locator('#creationTabs [data-ctab="look"]').click();
    await expect(page.locator('#ildaPanel')).toBeVisible();
    await expect(page.locator('#ildaNote')).toContainText('Mon carré.ild');
    await page.locator('#ildaFps').fill('10');
    await expect.poll(async () => (await studio.state()).settings.content.fps).toBe(10);
    expect((await studio.frame()).armed).toBe(false);

    // The same bytes again: the same file, not a copy.
    await page.locator('#creationTabs [data-ctab="media"]').click();
    await page.locator('#ildaFile').setInputFiles({ name: 'Mon carré.ild', mimeType: 'application/octet-stream', buffer: SQUARE });
    await expect(page.locator('#ildaMsg')).toContainText('déjà dans la médiathèque');
    await expect(page.locator('#ildaLib .ildaitem')).toHaveCount(1);

    // Kept across a restart: read again (on its own thread) at start-up.
    await studio.restart();
    await expect.poll(async () => (await studio.get('/api/media/ilda')).files.map((f: { file: string }) => f.file)).toEqual(['Mon carré.ild']);
    await expect.poll(async () => (await studio.presets()).presets.some(p => p.id === 'ilda:Mon carré.ild')).toBe(true);
    expect((await studio.frame()).armed).toBe(false);

    // Deleted: gone from the list and the cue page.
    await openMedia(page, studio);
    page.once('dialog', d => d.accept());
    await page.locator('#ildaLib .ildaitem[data-file="Mon carré.ild"] button.del').click();
    await expect(page.locator('#ildaMsg')).toContainText('supprimé');
    await expect(page.locator('#ildaLib')).toContainText('Aucun fichier ILDA');
    expect((await studio.presets()).presets.some(p => p.id === 'ilda:Mon carré.ild')).toBe(false);
  });

  test('the look exports as a format 5 file named for the SD card, read back by the studio', async ({ page }) => {
    await openMedia(page, studio);
    await page.locator('#ildaNum').fill('12');
    await expect(page.locator('#ildaExport')).toHaveText('Exporter 012.ild');
    await page.locator('#ildaFrames').fill('8');
    await page.locator('#ildaExpFps').fill('20');
    const download = page.waitForEvent('download');
    await page.locator('#ildaExport').click();
    const file = await download;
    expect(file.suggestedFilename()).toBe('012.ild');
    await expect(page.locator('#ildaExpMsg')).toContainText('012.ild exporté');
    const bytes = readFileSync((await file.path())!);
    expect(bytes.subarray(0, 4).toString('ascii')).toBe('ILDA');
    expect(bytes[7]).toBe(5);

    // Read back by the studio's own reader (T-001): imported, 8 images.
    const r = await fetch(studio.url + '/api/media/ilda?name=012.ild', { method: 'POST', body: bytes });
    expect(r.status).toBe(200);
    expect(await r.json()).toMatchObject({ file: '012.ild', frames: 8 });

    // Reserved numbers are refused with the reason.
    await page.locator('#ildaNum').fill('230');
    await page.locator('#ildaExport').click();
    await expect(page.locator('#ildaExpMsg')).toContainText('réservés');
    await expect(page.locator('#ildaExpMsg')).toHaveClass(/err/);
    expect((await studio.frame()).armed).toBe(false);
  });
});

test.describe('fichier abîmé au démarrage', () => {
  const studio = useStudio({ files: { 'media/ilda/abime.ild': 'ILDA\0\0\0\x05 not really' } });

  test('is set aside with a warning, and the studio keeps working', async ({ page }) => {
    await expect.poll(async () => (await studio.get('/api/media/ilda')).loading).toBe(false);
    expect((await studio.get('/api/media/ilda')).files).toEqual([]);
    const warnings = await studio.get('/api/data/warnings');
    expect(JSON.stringify(warnings)).toContain('media/ilda/abime.ild');
    await openMedia(page, studio);
    await expect(page.locator('#ildaLib')).toContainText('Aucun fichier ILDA');
    expect((await studio.frame()).armed).toBe(false);
  });
});
