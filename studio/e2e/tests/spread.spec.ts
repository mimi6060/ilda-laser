// Spread across zones (T-421): a modulator on master.size spread in phase
// over a group of three test zones (left, centre, right thirds) gives each
// zone its own size, shifted; its width keeps it active a quarter of each
// cycle. A cell of the grid takes a delay spread from « Propriétés du
// cue ». Preview only: the studio stays disarmed.
import { test, expect, useStudio, openUi, openWorkspace, isLit, type Point } from '../studio';

const studio = useStudio();

type Spread = { group: string; param: string; from: number; to: number; wings: number; blocks: number; shuffle?: number };
type Lfo = { target: string; width: number; attack: number; decay: number; spread?: Spread };
const lfos = async () => (await studio.get<{ lfos: Lfo[] }>('/api/lfos')).lfos;

const zone = (id: number, x: number) => ({ id, name: `Z${id}`, geometry: { size_x: 0.25, size_y: 0.25, pos_x: x } });
const TRIO = { zones: [zone(1, -0.6), zone(2, 0), zone(3, 0.6)], default_zones: [1, 2, 3], groups: [{ name: 'Trio', zones: [1, 2, 3] }] };

/** The width of the lit points in the left, centre and right thirds. */
async function widths() {
  const pts = ((await studio.frame()).points as Point[]).filter(isLit);
  const third = (lo: number, hi: number) => {
    const xs = pts.filter(p => p[0] >= lo && p[0] < hi).map(p => p[0]);
    return xs.length ? Math.max(...xs) - Math.min(...xs) : 0;
  };
  return [third(-1, -0.3), third(-0.3, 0.3), third(0.3, 1.01)];
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  expect(await studio.post('/api/lfos', [])).toBe(200); // the reset keeps the modulators
  expect(await studio.post('/api/projection', TRIO)).toBe(200);
  // A still square, small enough that the size modulation never reaches
  // the zone's edge; 120 BPM: a 4-beat cycle lasts 2 s.
  const look = { ...(await studio.state()).settings, content: { kind: 'shape', shape: 'square' }, scale: 0.5, rotation_speed: 0, brightness: 1 };
  expect(await studio.post('/api/settings', look)).toBe(200);
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 120 })).toBe(200);
  await openUi(page, studio);
});

test('a modulator spread 0 → 360° over three zones shifts each zone, its width keeps it to a quarter', async ({ page }) => {
  await openWorkspace(page, 'live');
  await page.locator('#liveTabs [data-panel-tab="lfo"]').click();
  await page.locator('#lfoAdd').click();
  const row = page.locator('#lfoList .lfo').first();
  await expect(row.locator('select[data-f="target"]')).toHaveValue('master.size');
  await row.locator('select[data-f="period"]').selectOption('4');
  // « Étalement »: the group, 0 → 360° by default; one bar per zone.
  const group = row.locator('.spread select[data-sf="group"]');
  await expect(group).toBeEnabled();
  await group.selectOption('Trio');
  await expect.poll(async () => (await lfos())[0]?.spread?.group).toBe('Trio');
  const sp = (await lfos())[0].spread!;
  expect([sp.param, sp.from, sp.to]).toEqual(['phase', 0, 360]);
  await expect(row.locator('.spread-chart span')).toHaveCount(3);
  await expect(row.locator('.spread-chart')).toHaveAttribute('aria-label', /zone 2 : 120°/);

  // Three sizes, never all alike, and their order changes over a cycle.
  const orders = new Set<string>();
  for (let i = 0; i < 12; i++) {
    const w = await widths();
    expect(w.every(x => x > 0)).toBe(true);
    expect(Math.max(...w) - Math.min(...w)).toBeGreaterThan(0.01);
    orders.add(w.map((x, k) => [x, k]).sort((a, b) => a[0] - b[0]).map(([, k]) => k).join(''));
    await page.waitForTimeout(180);
  }
  expect(orders.size).toBeGreaterThan(1);

  // Largeur 25 %: on one cycle, zone 1 sits at rest (its smallest size)
  // about three quarters of the time.
  await row.locator('input[data-f="width"]').fill('25');
  await expect.poll(async () => (await lfos())[0]?.width).toBeCloseTo(0.25, 3);
  await expect(row.locator('input[data-f="width"]')).toHaveValue('25');
  const sizes: number[] = [];
  const start = Date.now();
  while (Date.now() - start < 2200) {
    sizes.push((await widths())[0]);
    await page.waitForTimeout(60);
  }
  const rest = Math.min(...sizes);
  const active = sizes.filter(x => x > rest * 1.05).length / sizes.length;
  expect(active).toBeGreaterThan(0.08);
  expect(active).toBeLessThan(0.45);

  const f = await studio.frame() as unknown as { armed: boolean; output_lit: number };
  expect(f.armed).toBe(false);
  expect(f.output_lit).toBe(0);
});

test('a cell spread in delay from « Propriétés du cue », kept as a reference to the group', async ({ page }) => {
  const g = await studio.get<{ pages: { cells: (string | null)[] }[] }>('/api/grid');
  const cue = g.pages[0].cells.find((c): c is string => !!c)!;
  await page.locator(`#cues .cue[data-id="${cue}"]`).click({ button: 'right' });
  await expect(page.locator('#cueMenu')).toBeVisible();
  const box = page.locator('#cueMenuSpread');
  await box.locator('select[data-sf="param"]').selectOption('delay');
  await box.locator('select[data-sf="group"]').selectOption('Trio');
  await box.locator('input[data-sf="to"]').fill('1');
  await box.locator('input[data-sf="to"]').press('Tab');
  await box.locator('input[data-sf="wings"]').fill('2');
  await box.locator('input[data-sf="wings"]').press('Tab');
  const slot = async () => ((await studio.get('/api/cues')).slots as Record<string, { spread?: Spread }>)[cue]?.spread;
  await expect.poll(async () => { const s = await slot(); return s ? [s.group, s.param, s.from, s.to, s.wings] : null; }).toEqual(['Trio', 'delay', 0, 1, 2]);
  // Two wings on three zones: the outer ones alike, the centre late.
  await expect(box.locator('.spread-chart')).toHaveAttribute('aria-label', 'zone 1 : 0 s, zone 2 : 1 s, zone 3 : 0 s');
  // Mélange and « Autre tirage »: a seed is kept.
  await box.locator('input[data-sf="shuffle"]').check();
  await expect.poll(async () => typeof (await slot())?.shuffle).toBe('number');
  const seed = (await slot())!.shuffle;
  await box.locator('button[data-sf-redraw]').click();
  await expect.poll(async () => (await slot())?.shuffle).not.toBe(seed);

  // A fourth zone added to the group changes the spread, not the cell.
  const four = { ...TRIO, zones: [...TRIO.zones, zone(4, 0.9)], groups: [{ name: 'Trio', zones: [1, 2, 3, 4] }] };
  expect(await studio.post('/api/projection', four)).toBe(200);
  expect((await slot())!.group).toBe('Trio');

  // « Aucun » groupe: the spread goes.
  await box.locator('select[data-sf="group"]').selectOption('');
  await expect.poll(slot).toBeUndefined();
  expect((await studio.state()).armed).toBe(false);
});

test('without any zone group the spread editor is greyed with its help', async ({ page }) => {
  expect(await studio.post('/api/projection', { ...TRIO, groups: [] })).toBe(200);
  await openUi(page, studio);
  await openWorkspace(page, 'live');
  await page.locator('#liveTabs [data-panel-tab="lfo"]').click();
  await page.locator('#lfoAdd').click();
  const set = page.locator('#lfoList .lfo').first().locator('.spread-set');
  expect(await set.evaluate(el => (el as HTMLFieldSetElement).disabled)).toBe(true);
  await expect(set.locator('select[data-sf="group"]')).toBeDisabled();
  await expect(set).toContainText('Nécessite au moins un groupe de zones');
});
