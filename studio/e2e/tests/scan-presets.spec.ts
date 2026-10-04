// Scan presets (T-415): a cue's « Balayage » in « Propriétés du cue »
// changes how many points it is traced with; a user preset made in
// RÉGLAGES › Sorties et projecteurs is assigned to a cue and survives a
// restart. Preview only.
import { test, expect, useStudio, openUi, reveal } from '../studio';

const studio = useStudio();
let catalog: Awaited<ReturnType<typeof studio.presets>>;

interface ScanView { builtin: { id: string }[]; user: { id: string; name: string; scan_pct: number }[]; default: string | null }
interface MixFrame { cues: { shown: string[] }; layers: { mix: { per_layer: number[] } } }

/** Median over a few frames of the points layer 1 rendered (before the budget). */
async function layerPoints(): Promise<number> {
  const n: number[] = [];
  for (let i = 0; i < 5; i++) n.push((await studio.get<MixFrame>('/api/frame')).layers.mix.per_layer[0]);
  return n.sort((a, b) => a - b)[2];
}

test.beforeAll(async () => { catalog = await studio.presets(); });

test.beforeEach(async ({ page }) => {
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.multi', value: false });
  await studio.post('/api/scan-presets', { user: [], default: '' });
  const slots = (await studio.get('/api/cues')).slots as Record<string, unknown>;
  for (const id of Object.keys(slots)) await studio.post('/api/cues/slot', { id });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('Faisceaux in the cue menu traces the cue with fewer points than Trame', async ({ page }) => {
  const cue = catalog.presets.filter(p => p.category === catalog.categories[0])[0];
  const cell = page.locator(`#cues .cue[data-id="${cue.id}"]`);
  await cell.click();
  await expect.poll(async () => (await studio.get<MixFrame>('/api/frame')).cues.shown).toEqual([cue.id]);
  const auto = await layerPoints();
  expect(auto).toBeGreaterThan(0);

  await cell.click({ button: 'right' });
  await expect(page.locator('#cueMenuScan option')).toHaveText(['Par défaut de la sortie', 'Faisceaux', 'Graphismes', 'Trame']);
  await page.locator('#cueMenuScan').selectOption('raster');
  await expect.poll(async () => (await studio.get('/api/cues')).slots[cue.id]?.scan).toBe('raster');
  await expect.poll(layerPoints).not.toBe(auto);
  const raster = await layerPoints();

  await page.locator('#cueMenuScan').selectOption('beams');
  await expect.poll(async () => (await studio.get('/api/cues')).slots[cue.id]?.scan).toBe('beams');
  await expect.poll(layerPoints).toBeLessThan(raster * 0.8);

  // Back to the output's default: the cue keeps its other properties, no preset.
  await page.locator('#cueMenuScan').selectOption('');
  await expect.poll(async () => (await studio.get('/api/cues')).slots[cue.id]?.scan ?? null).toBe(null);
  await page.locator('#cueMenuClose').click();
  expect((await studio.get('/api/state')).armed).toBe(false);
});

test('a user preset is created, edited, assigned and still there after a restart', async ({ page }) => {
  await reveal(page, '#spBox');
  // Built-ins are read-only.
  await page.locator('#spList').selectOption('beams');
  await expect(page.locator('#spLit')).toBeDisabled();
  await expect(page.locator('#spDelete')).toBeDisabled();
  await page.locator('#spDup').click();
  await expect.poll(async () => (await studio.get<ScanView>('/api/scan-presets')).user.map(u => u.id)).toEqual(['u-1']);
  await expect(page.locator('#spList')).toHaveValue('u-1');
  await expect(page.locator('#spLit')).toBeEnabled();
  await page.locator('#spName').fill('Faisceaux lents');
  await page.locator('#spName').press('Enter');
  await page.locator('#spScan').fill('70');
  await page.locator('#spScan').dispatchEvent('change');
  await expect.poll(async () => (await studio.get<ScanView>('/api/scan-presets')).user[0]).toMatchObject({ name: 'Faisceaux lents', scan_pct: 70 });
  await page.locator('#spDefault').selectOption('graphics');
  await expect.poll(async () => (await studio.get<ScanView>('/api/scan-presets')).default).toBe('graphics');

  // Assigned to a cue from its menu.
  const cue = catalog.presets.filter(p => p.category === catalog.categories[0])[1];
  await reveal(page, `#cues .cue[data-id="${cue.id}"]`);
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click({ button: 'right' });
  await page.locator('#cueMenuScan').selectOption('u-1');
  await expect.poll(async () => (await studio.get('/api/cues')).slots[cue.id]?.scan).toBe('u-1');
  await page.locator('#cueMenuClose').click();

  await studio.restart();
  const v = await studio.get<ScanView>('/api/scan-presets');
  expect(v.user.map(u => [u.id, u.name])).toEqual([['u-1', 'Faisceaux lents']]);
  expect(v.default).toBe('graphics');
  expect((await studio.get('/api/cues')).slots[cue.id]?.scan).toBe('u-1');
  expect((await studio.get('/api/state')).armed).toBe(false);

  // Deleted while still assigned: the cue falls back to the default, with a warning.
  await openUi(page, studio);
  await reveal(page, '#spBox');
  await page.locator('#spList').selectOption('u-1');
  await page.locator('#spDelete').click();
  await expect.poll(async () => (await studio.get<ScanView>('/api/scan-presets')).user.length).toBe(0);
  await expect(page.locator('#spMsg')).toContainText('préréglage supprimé');
});
