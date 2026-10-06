// Site profiles, the export package and the partial import (T-289):
// a fresh data dir starts on the site profile « Par défaut »; a show is
// exported as a .lspack (project, its ILDA file, the site) from one
// studio and imported into another one with its own temporary data dir:
// same project, same media (equal SHA-256), the site offered at opening,
// never applied until « Utiliser celui du projet », a looser one only
// after the confirmation. A « zip slip » package is refused. A partial
// import takes only the parts ticked. Preview only: never armed, never
// --device.
import { test, expect, useStudio, openUi, openWorkspace, type Studio } from '../studio';
import type { Page } from '@playwright/test';
import { createHash } from 'node:crypto';
import { readFileSync, existsSync, rmSync } from 'node:fs';
import path from 'node:path';

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

const TRIANGLE = ildaFile([[
  { x: -0.4, y: -0.4, r: 0, g: 0, b: 0, blank: true },
  { x: -0.4, y: -0.4, r: 255, g: 0, b: 0 }, { x: 0.4, y: -0.4, r: 255, g: 0, b: 0 }, { x: 0, y: 0.4, r: 255, g: 0, b: 0 }, { x: -0.4, y: -0.4, r: 255, g: 0, b: 0 },
]]);

const CRC = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc32 = (b: Buffer) => { let c = 0xffffffff; for (const x of b) c = CRC[(c ^ x) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };

/** A stored (uncompressed) zip of `entries`, names taken as given. */
function zip(entries: [string, Buffer][]): Buffer {
  const locals: Buffer[] = [], centrals: Buffer[] = [];
  let offset = 0;
  for (const [name, data] of entries) {
    const n = Buffer.from(name, 'utf8'), crc = crc32(data);
    const l = Buffer.alloc(30);
    l.writeUInt32LE(0x04034b50, 0); l.writeUInt16LE(20, 4); l.writeUInt32LE(crc, 14);
    l.writeUInt32LE(data.length, 18); l.writeUInt32LE(data.length, 22); l.writeUInt16LE(n.length, 26);
    const c = Buffer.alloc(46);
    c.writeUInt32LE(0x02014b50, 0); c.writeUInt16LE(20, 4); c.writeUInt16LE(20, 6); c.writeUInt32LE(crc, 16);
    c.writeUInt32LE(data.length, 20); c.writeUInt32LE(data.length, 24); c.writeUInt16LE(n.length, 28); c.writeUInt32LE(offset, 42);
    locals.push(l, n, data); centrals.push(c, n);
    offset += 30 + n.length + data.length;
  }
  const cd = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cd.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, end]);
}

const sha = (b: Buffer) => createHash('sha256').update(b).digest('hex');
const cal = (offset_x: number) => ({ offset_x, offset_y: 0, scale_x: 1, scale_y: 1, rotation_deg: 0 });

async function postBytes(studio: Studio, p: string, body: Buffer) {
  const r = await fetch(studio.url + p, { method: 'POST', body });
  return { status: r.status, text: await r.text() };
}

async function openSitePanel(page: Page) {
  await openWorkspace(page, 'settings');
  const panel = page.locator('#sitePanel');
  if (!(await panel.evaluate(d => (d as HTMLDetailsElement).open))) await panel.locator('summary').click();
  await expect(page.locator('#siteSelect')).toBeVisible();
}

test.describe.configure({ mode: 'serial' });

const home = useStudio();
const venue = useStudio();
let packFile = '';
test.afterAll(() => { if (packFile) rmSync(packFile, { force: true }); });

test('a fresh data dir starts on « Par défaut »; « Enregistrer sous… » and « Charger » from the panel, disarmed', async ({ page }) => {
  const v = await home.get('/api/sites');
  expect(v.active).toBe('Par défaut');
  expect(existsSync(path.join(home.dataDir, 'sites'))).toBe(true);
  await openUi(page, home);
  await openSitePanel(page);
  await expect(page.locator('#siteInfo')).toContainText('Profil actif : Par défaut');
  expect(await home.post('/api/calibration', cal(0.2))).toBe(200);
  await page.locator('#siteSaveAs').click();
  await page.locator('#siteName').fill('Studio maison');
  await page.locator('#siteSaveOk').click();
  await expect(page.locator('#siteInfo')).toContainText('Profil actif : Studio maison');
  // « Par défaut » brings the old calibration back, laser disarmed.
  await page.locator('#siteSelect').selectOption('Par défaut');
  await page.locator('#siteLoad').click();
  await expect(page.locator('#siteInfo')).toContainText('Profil actif : Par défaut');
  const st = await home.state();
  expect(st.calibration.offset_x).toBe(0);
  expect(st.armed).toBe(false);
  await page.locator('#siteSelect').selectOption('Studio maison');
  await page.locator('#siteLoad').click();
  await expect(page.locator('#siteInfo')).toContainText('Profil actif : Studio maison');
  expect((await home.state()).calibration.offset_x).toBeCloseTo(0.2, 5);
});

test('« Exporter un paquet… » downloads the project, its ILDA file and the site', async ({ page }) => {
  const imported = await postBytes(home, '/api/media/ilda?name=triangle.ild', TRIANGLE);
  expect(imported.status).toBe(200);
  expect(await home.post('/api/presets/play', { id: 'ilda:triangle.ild' })).toBe(200);
  expect(await home.post('/api/scenes/save', { name: 'Triangle', duration_secs: 5 })).toBe(200);
  expect(await home.post('/api/project/save-as', { path: 'Tournée' })).toBe(200);
  await openUi(page, home);
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="pack-export"]').click();
  await expect(page.locator('#packDialog')).toBeVisible();
  await expect(page.locator('#packDialog')).toContainText('responsable des droits');
  await page.locator('#packSite').check();
  const download = page.waitForEvent('download');
  await page.locator('#packExportOk').click();
  const d = await download;
  expect(d.suggestedFilename()).toMatch(/\.lspack$/);
  packFile = path.join(path.dirname(home.dataDir), path.basename(home.dataDir) + '-tournee.lspack');
  await d.saveAs(packFile);
  const bytes = readFileSync(packFile);
  expect(bytes.subarray(0, 2).toString()).toBe('PK');
  // No path of this Mac in the package.
  expect(bytes.includes(Buffer.from(home.dataDir))).toBe(false);
});

test('« Importer un paquet… » on an empty data dir: same project, same media, the site offered and kept until chosen', async ({ page }) => {
  expect(packFile).not.toBe('');
  // The venue has a blanking zone: the project's site is looser.
  const { settings } = await venue.get('/api/safety');
  settings.zones.push({ id: 0, name: 'Plafond', kind: 'blank', level: 0, points: [[-1, 0.8], [1, 0.8], [1, 1], [-1, 1]] });
  expect(await venue.post('/api/safety', settings)).toBe(200);
  await openUi(page, venue);
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="pack-import"]').click();
  await expect(page.locator('#packImportDialog')).toBeVisible();
  await page.locator('#packFile').setInputFiles(packFile);
  await page.locator('#packImportOk').click();
  await expect(page.locator('#packImportMsg')).toContainText('Projet « Tournée » importé avec 1 média');
  await expect(page.locator('#packImportMsg')).toContainText('Profil de site « Studio maison »');
  // Same media, byte for byte.
  const there = readFileSync(path.join(venue.dataDir, 'media', 'ilda', 'triangle.ild'));
  expect(sha(there)).toBe(sha(TRIANGLE));
  expect((await venue.state()).calibration.offset_x).toBe(0);

  // Opening offers the site; « Garder le profil actuel » is the default.
  await page.locator('#packImportOpen').click();
  // The fresh « Sans titre » may count as modified: not kept.
  await expect(page.locator('#projAsk:visible, #siteAsk:visible').first()).toBeVisible();
  if (await page.locator('#projAsk').isVisible()) await page.locator('#askDiscard').click();
  await expect(page.locator('#siteAsk')).toBeVisible();
  await expect(page.locator('#siteAskDiff')).toContainText('Calibration');
  await expect(page.locator('#siteAskKeep')).toBeFocused();
  await page.locator('#siteAskKeep').click();
  await expect(page.locator('#siteAsk')).toBeHidden();
  await expect(page.locator('#projName')).toHaveText('Tournée');
  let st = await venue.state();
  expect(st.calibration.offset_x).toBe(0);
  expect(st.scenes.map((s: { name: string }) => s.name)).toEqual(['Triangle']);

  // From the panel: looser, so the list and a confirmation first.
  await openSitePanel(page);
  await expect(page.locator('#siteProjectText')).toContainText('Studio maison');
  await page.locator('#siteProjectUse').click();
  await expect(page.locator('#siteLoosen')).toBeVisible();
  await expect(page.locator('#siteLoosenText')).toContainText('moins restrictif');
  expect((await venue.state()).calibration.offset_x).toBe(0);
  expect((await venue.get('/api/safety')).settings.zones.length).toBe(1);
  await page.locator('#siteLoosenOk').click();
  await expect(page.locator('#siteInfo')).toContainText('Profil actif : Studio maison');
  st = await venue.state();
  expect(st.calibration.offset_x).toBeCloseTo(0.2, 5);
  expect(st.armed).toBe(false);
});

test('a « zip slip » package is refused and writes nothing', async ({ page }) => {
  const evil = zip([['manifest.json', Buffer.from('{"format_version":1,"project":"project.lsproj"}')], ['../../evil.lsproj', Buffer.from('{}')]]);
  const r = await postBytes(venue, '/api/pack/import', evil);
  expect(r.status).toBe(400);
  expect(r.text).toContain('chemin interdit');
  expect(existsSync(path.join(path.dirname(venue.dataDir), 'evil.lsproj'))).toBe(false);
  // Through the page: the reason is shown under the field.
  await openUi(page, venue);
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="pack-import"]').click();
  await page.locator('#packFile').setInputFiles({ name: 'mauvais.lspack', mimeType: 'application/zip', buffer: evil });
  await page.locator('#packImportOk').click();
  await expect(page.locator('#packImportError')).toContainText('chemin interdit');
  expect((await venue.state()).armed).toBe(false);
});

test('« Importer depuis un projet… » takes only the parts ticked', async ({ page }) => {
  // Another project here, with its own scene and nothing else.
  expect(await venue.post('/api/project/new')).toBe(200);
  expect(await venue.post('/api/scenes/save', { name: 'Locale', duration_secs: 4 })).toBe(200);
  expect(await venue.post('/api/project/save-as', { path: 'Ici' })).toBe(200);
  await openUi(page, venue);
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="partial"]').click();
  await expect(page.locator('#partialDialog')).toBeVisible();
  await page.locator('#partialFrom').selectOption('Tournée');
  await page.locator('#partialParts input[value="midi"]').check();
  await page.locator('#partialCheck').click();
  await expect(page.locator('#partialPreview')).toContainText('Correspondances MIDI : 0 ajouté');
  // MIDI only: the scenes stay.
  await page.locator('#partialOk').click();
  await expect(page.locator('#partialPreview')).toContainText('Importé');
  expect((await venue.state()).scenes.map((s: { name: string }) => s.name)).toEqual(['Locale']);
  // Then the scenes.
  await page.locator('#partialParts input[value="midi"]').uncheck();
  await page.locator('#partialParts input[value="scenes"]').check();
  await page.locator('#partialOk').click();
  await expect(page.locator('#partialPreview')).toContainText('Scènes : 1 ajouté');
  expect((await venue.state()).scenes.map((s: { name: string }) => s.name)).toEqual(['Locale', 'Triangle']);
  await expect(page.locator('#projName')).toHaveText('Ici •');
  expect((await venue.state()).armed).toBe(false);
});
