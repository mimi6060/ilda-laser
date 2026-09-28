// Figure import (T-297): CRÉATION › Figures › « Importer… ». An SVG and a
// picture generated here (no third-party file) become editable figures,
// previewed before « Créer la figure », under the point budget; bad files
// give a clear message and the studio keeps working. Preview only: the
// laser stays disarmed.
import { test, expect, useStudio, openUi, openWorkspace, isLit, extent } from '../studio';
import type { Page } from '@playwright/test';
import { deflateSync } from 'node:zlib';

const studio = useStudio();

interface Stroke { points: [number, number][]; color: [number, number, number]; lit: boolean }
interface Figure { name: string; frames: { strokes: Stroke[] }[] }
const load = async (name: string) => {
  const r = await fetch(studio.url + '/api/figures/load', { method: 'POST', body: JSON.stringify({ name }) });
  return r.ok ? (await r.json()) as Figure : null;
};

// ----- a tiny PNG encoder (RGB, no filter), for generated pictures -----
const CRC = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf: Buffer) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type: string, data: Buffer) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
function png(w: number, h: number, pixel: (x: number, y: number) => [number, number, number]) {
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w * 3 + 1)] = 0;
    for (let x = 0; x < w; x++) raw.set(pixel(x, y), y * (w * 3 + 1) + 1 + x * 3);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

// Our own little logo: a red square, a group turned and scaled with a
// blue circle, a green curve, a black (→ default colour) triangle.
const LOGO_SVG = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200">
  <rect x="20" y="20" width="60" height="60" fill="none" stroke="#ff0000"/>
  <g transform="translate(140 50) scale(2)"><circle r="15" fill="#0000ff"/></g>
  <path d="M20 150 C60 100 100 200 180 150" fill="none" stroke="rgb(0,255,0)"/>
  <polygon points="100,120 120,160 80,160"/>
</svg>`;

async function openImport(page: Page) {
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="figures"]').click();
  await expect(page.locator('#figEditor')).toBeVisible();
  await page.locator('#figImport').click();
  await expect(page.locator('#figImportDlg')).toBeVisible();
  // The rights reminder is always there.
  await expect(page.locator('#impRights')).toContainText('droits');
}
const give = (page: Page, name: string, mimeType: string, buffer: Buffer) => page.locator('#impFile').setInputFiles({ name, mimeType, buffer });

test.beforeEach(async () => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
});

test('an SVG logo becomes a faithful, editable figure that plays as a cue', async ({ page }) => {
  await openImport(page);
  await expect(page.locator('#impCreate')).toBeDisabled();
  await give(page, 'mon logo.svg', 'image/svg+xml', Buffer.from(LOGO_SVG));
  await expect(page.locator('#impStats')).toContainText('4 tracé(s)');
  await expect(page.locator('#impCreate')).toBeEnabled();
  await expect(page.locator('#impName')).toHaveValue('mon logo');
  await expect(page.locator('#impSvgOpts')).toBeVisible();
  await expect(page.locator('#impRasterOpts')).toBeHidden();
  // Nothing saved by the preview.
  expect(await load('mon logo')).toBeNull();
  // A smaller size re-previews.
  await page.locator('#impSize').fill('50');
  await expect(page.locator('#impSizeV')).toHaveText('50 %');
  await page.locator('#impName').fill('Logo importe');
  await page.locator('#impCreate').click();
  await expect(page.locator('#figImportDlg')).toBeHidden();
  await expect(page.locator('#figMsg')).toContainText('créée depuis l\'import');

  // Saved, and open in the editor: editable like a drawn figure.
  const fig = (await load('Logo importe'))!;
  const strokes = fig.frames[0].strokes;
  expect(strokes.length).toBe(4);
  const colours = strokes.map(s => s.color.join(',')).sort();
  expect(colours).toEqual(['0,0,255', '0,255,0', '255,0,0', '255,255,255']);
  const all = strokes.flatMap(s => s.points);
  const ext = Math.max(...all.map(p => Math.max(Math.abs(p[0]), Math.abs(p[1]))));
  expect(ext).toBeGreaterThan(0.45); expect(ext).toBeLessThan(0.51);
  // The square stays a square (closed, 4 corners, same width and height), top left.
  const sq = strokes.find(s => s.color[0] === 255 && s.color[1] === 0)!;
  expect(sq.points[0]).toEqual(sq.points[sq.points.length - 1]);
  expect(sq.points.length).toBe(5);
  const xs = sq.points.map(p => p[0]), ys = sq.points.map(p => p[1]);
  expect(Math.abs((Math.max(...xs) - Math.min(...xs)) - (Math.max(...ys) - Math.min(...ys)))).toBeLessThan(0.01);
  expect(Math.max(...xs)).toBeLessThan(0); expect(Math.min(...ys)).toBeGreaterThan(0);
  // The circle is round (the group's scale applied).
  const circle = strokes.find(s => s.color[2] === 255 && s.color[0] === 0)!;
  const cx = circle.points.reduce((a, p) => a + p[0], 0) / circle.points.length;
  const cy = circle.points.reduce((a, p) => a + p[1], 0) / circle.points.length;
  const radii = circle.points.map(p => Math.hypot(p[0] - cx, p[1] - cy));
  expect(Math.max(...radii) - Math.min(...radii)).toBeLessThan(0.03);
  await expect(page.locator('#figName')).toHaveValue('Logo importe');
  await expect(page.locator('#figStats')).toContainText('4 tracé(s)');
  await page.locator('#figMirX').click();
  await page.locator('#figSave').click();
  await expect(page.locator('#figMsg')).toContainText('enregistrée');
  const mirrored = (await load('Logo importe'))!;
  expect(mirrored.frames[0].strokes[0].points[0][0]).toBeCloseTo(-strokes[0].points[0][0], 3);

  // It plays as a cue; the laser stays off.
  await page.locator('#figPlay').click();
  await expect.poll(async () => (await studio.frame()).active_cue).toBe('figure:Logo importe');
  await expect.poll(async () => (await studio.frame()).points.filter(isLit).length).toBeGreaterThan(50);
  expect(extent((await studio.frame()).points)).toBeGreaterThan(0.4);
  const f = await studio.frame() as unknown as { armed: boolean; output_lit: number };
  expect(f.armed).toBe(false);
  expect(f.output_lit).toBe(0);
  await page.screenshot({ path: 'test-results/figure-import.png' });
});

test('a contrasted picture becomes a figure under the point budget', async ({ page }) => {
  // Black shapes on white: a disc, a ring and a bar.
  const img = png(240, 180, (x, y) => {
    const d1 = Math.hypot(x - 60, y - 90), d2 = Math.hypot(x - 150, y - 90);
    const dark = d1 < 40 || (d2 < 40 && d2 > 25) || (x > 200 && x < 225 && y > 30 && y < 150);
    return dark ? [0, 0, 0] : [255, 255, 255];
  });
  await openImport(page);
  await page.locator('#impBudget').fill('600');
  await give(page, 'dessin.png', 'image/png', img);
  await expect(page.locator('#impRasterOpts')).toBeVisible();
  await expect(page.locator('#impCreate')).toBeEnabled();
  // Disc, ring (outside and inside) and bar.
  await expect(page.locator('#impStats')).toContainText('4 tracé(s)');
  await expect(page.locator('#impStats')).toContainText('budget de 600');
  await expect(page.locator('#impStats')).not.toHaveClass(/bad/);
  await page.screenshot({ path: 'test-results/figure-import-dialog.png' });
  const r = await fetch(`${studio.url}/api/figures/import?name=d.png&budget=600`, { method: 'POST', body: img });
  const preview = await r.json();
  expect(preview.stats.laser_points).toBeLessThanOrEqual(600);
  expect(preview.stats.threshold).toBeGreaterThan(0);

  // A tight budget: details dropped, a warning, still under it.
  await page.locator('#impBudget').fill('150');
  await page.locator('#impBudget').dispatchEvent('change');
  await expect(page.locator('#impMsg')).toContainText('budget de 150');
  await expect(page.locator('#impStats')).toContainText('budget de 150');

  // The colours mode: one colour here (black), drawn in the chosen colour... then back.
  await page.locator('#impBudget').fill('750');
  await page.locator('#impBudget').dispatchEvent('change');
  await page.locator('#impMode').selectOption('lines');
  await expect(page.locator('#impMsg')).not.toHaveClass(/err/);
  await page.locator('#impMode').selectOption('contours');
  await expect(page.locator('#impStats')).toContainText('4 tracé(s)');
  await page.locator('#impCreate').click();
  await expect(page.locator('#figImportDlg')).toBeHidden();
  const fig = (await load('dessin'))!;
  expect(fig.frames[0].strokes.length).toBe(4);
  expect(fig.frames[0].strokes.every(s => s.color.join() === '255,255,255')).toBe(true);
  await expect(page.locator('#figLib [data-fig="dessin"]')).toBeVisible();

  // Colours mode on a two-colour picture: each outline in its colour.
  const two = png(120, 60, (x, y) => (y < 10 || y > 50 || x < 10 || x > 110) ? [255, 255, 255] : x < 60 ? [200, 0, 0] : [0, 0, 180]);
  const c = await fetch(`${studio.url}/api/figures/import?name=c.png&mode=colors&colors=2`, { method: 'POST', body: two });
  expect(c.status).toBe(200);
  const colours = new Set(((await c.json()).figure as Figure).frames[0].strokes.map(s => s.color.join()));
  expect([...colours].sort()).toEqual(['0,0,255', '255,0,0']);
});

test('bad files give a clear message, nothing is created, the studio keeps working', async ({ page }) => {
  await openImport(page);
  const cases: [string, string, Buffer, RegExp][] = [
    ['notes.txt', 'text/plain', Buffer.from('bonjour'), /format non reconnu/],
    ['vide.svg', 'image/svg+xml', Buffer.alloc(0), /fichier vide/],
    ['casse.svg', 'image/svg+xml', Buffer.from('<svg><rect width="1"'), /SVG illisible/],
    ['bombe.svg', 'image/svg+xml', Buffer.from('<?xml version="1.0"?><!DOCTYPE s [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;">]><svg>&b;</svg>'), /entités/],
    ['externe.svg', 'image/svg+xml', Buffer.from('<?xml version="1.0"?><!DOCTYPE s [<!ENTITY x SYSTEM "http://127.0.0.1:9/x">]><svg>&x;</svg>'), /entités/],
    ['texte.svg', 'image/svg+xml', Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"><text>Salut</text></svg>'), /aucune forme dessinable/],
    ['profond.svg', 'image/svg+xml', Buffer.from('<svg>' + '<g>'.repeat(20000) + '</svg>'), /imbriqués/],
    ['tronque.png', 'image/png', png(50, 50, () => [0, 0, 0]).subarray(0, 60), /image illisible/],
    ['blanc.png', 'image/png', png(40, 40, () => [255, 255, 255]), /aucun contour/],
  ];
  for (const [name, mime, buf, re] of cases) {
    await give(page, name, mime, buf);
    await expect(page.locator('#impMsg')).toHaveClass(/err/);
    await expect(page.locator('#impMsg')).toContainText(re);
    await expect(page.locator('#impCreate')).toBeDisabled();
  }
  // A good file after the bad ones works; an invalid name is refused.
  await give(page, 'ok.svg', 'image/svg+xml', Buffer.from(LOGO_SVG));
  await expect(page.locator('#impCreate')).toBeEnabled();
  await page.locator('#impName').fill('../mauvais');
  await page.locator('#impCreate').click();
  await expect(page.locator('#impMsg')).toContainText('Nom invalide');
  await page.locator('#impCancel').click();
  await expect(page.locator('#figImportDlg')).toBeHidden();
  const list = await (await fetch(studio.url + '/api/figures')).json() as { name: string }[];
  expect(list.map(f => f.name)).not.toContain('ok');
  // Too big for the server: refused without reading it all.
  const big = await fetch(studio.url + '/api/figures/import', { method: 'POST', body: Buffer.alloc(21 * 1024 * 1024, 0x20) });
  expect(big.status).toBe(413);
  expect(await big.text()).toContain('trop gros');
  expect((await fetch(studio.url + '/api/state')).status).toBe(200);
  expect(((await studio.frame()) as unknown as { armed: boolean }).armed).toBe(false);
});
