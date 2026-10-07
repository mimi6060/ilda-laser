// Figure parts (T-441): strokes tagged with a part in CRÉATION › Figures,
// a brightness modulator on one part (LIVE › Modulateurs) that leaves the
// other strokes untouched in /api/frame, the Direct panel's « Cible », and
// named SVG groups imported as parts. Preview only: the laser stays
// disarmed.
import { test, expect, useStudio, openUi, openWorkspace, isLit, type Point } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Stroke { points: [number, number][]; color: [number, number, number]; lit: boolean; part?: number }
interface Figure { name: string; frames: { strokes: Stroke[] }[]; parts?: { id: number; name: string }[] }
const load = async (name: string) => {
  const r = await fetch(studio.url + '/api/figures/load', { method: 'POST', body: JSON.stringify({ name }) });
  return r.ok ? (await r.json()) as Figure : null;
};

async function xy(page: Page, x: number, y: number): Promise<[number, number]> {
  const b = (await page.locator('#figCanvas').boundingBox())!;
  return [b.x + (x + 1) / 2 * b.width, b.y + (1 - y) / 2 * b.height];
}
async function click(page: Page, x: number, y: number) { await page.mouse.click(...await xy(page, x, y)); }
const tool = (page: Page, t: string) => page.locator(`#figTools [data-tool="${t}"]`).click();

async function openEditor(page: Page) {
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="figures"]').click();
  await expect(page.locator('#figEditor')).toBeVisible();
}

const red = (p: Point) => p[2] > 0 && p[3] === 0 && p[4] === 0;
const green = (p: Point) => p[3] > 0 && p[2] === 0 && p[4] === 0;

test.beforeEach(async () => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/lfos', []);
});

test('a brightness modulator on « Yeux » moves only the strokes of that part', async ({ page }) => {
  await openEditor(page);
  // Two strokes: a red line (the outline), a green line (the eyes).
  await page.locator('#figColor').fill('#ff0000');
  await tool(page, 'line');
  await click(page, -0.6, -0.4);
  await page.mouse.dblclick(...await xy(page, 0.6, -0.4));
  await page.locator('#figColor').fill('#00ff00');
  await tool(page, 'line');
  await click(page, -0.4, 0.4);
  await page.mouse.dblclick(...await xy(page, 0.4, 0.4));
  await expect(page.locator('#figStats')).toContainText('2 tracé(s)');
  await expect(page.locator('#figParts')).toContainText('Aucune partie');

  // Select the green one, give it a new part, name it « Yeux ».
  await tool(page, 'select');
  await click(page, 0, 0.4);
  await expect(page.locator('#figPart')).toBeEnabled();
  await page.locator('#figPart').selectOption('new');
  await expect(page.locator('#figPartName1')).toHaveValue('Partie 1');
  await page.locator('#figPartName1').fill('Yeux');
  await page.locator('#figPartName1').press('Tab');
  await expect(page.locator('#figParts .figpart')).toContainText('1 tracé');
  // Duplicating a stroke keeps its part; undo takes the copy back.
  await page.locator('#figDup').click();
  await expect(page.locator('#figParts .figpart')).toContainText('2 tracés');
  await page.locator('#figUndo').click();
  await expect(page.locator('#figParts .figpart')).toContainText('1 tracé');

  await page.locator('#figName').fill('Visage');
  await page.locator('#figPlay').click();
  await expect(page.locator('#figMsg')).toContainText('enregistrée');
  const saved = (await load('Visage'))!;
  expect(saved.parts).toEqual([{ id: 1, name: 'Yeux' }]);
  expect(saved.frames[0].strokes.map(s => s.part ?? 0)).toEqual([0, 1]);
  await expect.poll(async () => (await studio.frame()).active_cue).toBe('figure:Visage');

  // LIVE › Modulateurs: a fast sine on the brightness of « Visage › Yeux ».
  await openWorkspace(page, 'live');
  await page.locator('#liveTabs [data-panel-tab="lfo"]').click();
  await page.locator('#lfoAdd').click();
  const row = page.locator('#lfoList .lfo').first();
  await row.locator('select[data-f="part"]').selectOption('figure:Visage|1');
  await expect(row.locator('select[data-f="param"]')).toHaveValue('brightness');
  await expect(row.locator('select[data-f="target"]')).toBeHidden();
  await row.locator('select[data-f="period"]').selectOption('hz');
  await row.locator('input[data-f="hz"]').fill('3');
  await row.locator('input[data-f="depth"]').fill('100');
  await expect.poll(async () => {
    const r = await (await fetch(studio.url + '/api/lfos')).json();
    return r.lfos[0]?.part ? [r.lfos[0].part.cue, r.lfos[0].part.part, r.lfos[0].part.param, r.lfos[0].depth] : null;
  }).toEqual(['figure:Visage', 1, 'brightness', 1]);

  // Several instants: the red stroke's colour never moves, the green one's does.
  const reds = new Set<string>(), greens = new Set<number>();
  for (let i = 0; i < 8; i++) {
    const pts = (await studio.frame()).points.filter(isLit);
    pts.filter(red).forEach(p => reds.add(p[2].toFixed(5)));
    const g = pts.filter(green);
    if (g.length) greens.add(+Math.max(...g.map(p => p[3])).toFixed(3));
    await page.waitForTimeout(70);
  }
  expect(reds.size).toBe(1);
  expect(greens.size).toBeGreaterThan(1);
  const f = await studio.frame() as unknown as { armed: boolean; output_lit: number };
  expect(f.armed).toBe(false);
  expect(f.output_lit).toBe(0);

  // Direct › Cue › Cible: the part, its brightness stored in the cue's cell.
  await page.locator('#liveTabs [data-panel-tab="direct"]').click();
  await page.locator('#directTabs [data-scope="cue"]').click();
  await expect(page.locator('#directPartRow')).toBeVisible();
  await page.locator('#directPart').selectOption({ label: 'Yeux' });
  await expect(page.locator('#directScopeHint')).toContainText('Yeux');
  await page.locator('#mBright').fill('40');
  await expect.poll(async () => {
    const d = await (await fetch(studio.url + '/api/cues')).json();
    return d.slots?.['figure:Visage']?.part_modifiers?.['1']?.brightness;
  }).toBeCloseTo(0.4, 3);
  // The whole figure again: the cue's own controls.
  await page.locator('#directPart').selectOption({ label: 'Figure entière' });
  await expect(page.locator('#directScopeHint')).not.toContainText('Yeux');
  await studio.post('/api/lfos', []);
});

test('named SVG groups become parts; an unticked group stays without part', async ({ page }) => {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
    <g id="Contour"><rect x="10" y="10" width="80" height="80" fill="none" stroke="#ff0000"/></g>
    <g id="Yeux"><circle cx="35" cy="40" r="6" fill="none" stroke="#00ff00"/><circle cx="65" cy="40" r="6" fill="none" stroke="#00ff00"/></g>
    <g id="Bouche"><line x1="30" y1="70" x2="70" y2="70" stroke="#0000ff"/></g>
  </svg>`;
  await openEditor(page);
  await page.locator('#figImport').click();
  await expect(page.locator('#figImportDlg')).toBeVisible();
  await page.locator('#impFile').setInputFiles({ name: 'visage.svg', mimeType: 'image/svg+xml', buffer: Buffer.from(svg) });
  await expect(page.locator('#impParts')).toContainText('Parties détectées : Contour, Yeux, Bouche');
  await expect(page.locator('#impParts input[type=checkbox]')).toHaveCount(3);
  // « Bouche » not kept as a part.
  await page.locator('#impParts input[data-part-name="Bouche"]').uncheck();
  await page.locator('#impName').fill('Visage importe');
  await page.locator('#impCreate').click();
  await expect(page.locator('#figImportDlg')).toBeHidden();
  const fig = (await load('Visage importe'))!;
  expect(fig.parts!.map(p => p.name)).toEqual(['Contour', 'Yeux']);
  const byPart = (n: number) => fig.frames[0].strokes.filter(s => (s.part ?? 0) === n).length;
  expect([byPart(0), byPart(1), byPart(2)]).toEqual([1, 1, 2]);
  // The editor shows them.
  await expect(page.locator('#figParts .figpart')).toHaveCount(2);
  await expect(page.locator('#figPartName2')).toHaveValue('Yeux');
});
