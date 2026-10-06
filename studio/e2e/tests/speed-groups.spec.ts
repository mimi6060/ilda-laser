// Speed groups and the rate master (T-419): two cues locked on the tempo,
// one put in group A from « Propriétés du cue », group A set to ×½ in
// LIVE › Direct › Vitesses; its beat moves half as fast as the master's in
// /api/frame, the top bar recalls it, and the *Cadence* slider drives
// `master.rate`. Preview only: the studio stays disarmed.
import { test, expect, useStudio, openUi, reveal } from '../studio';

const studio = useStudio();
type Grid = { pages: { cells: (string | null)[] }[] };
interface Group { group: number; name: string; mult: number | null; free: boolean; bpm: number; label: string; beat?: number }
interface SpeedFrame {
  armed: boolean;
  tempo: { bpm: number; effect_beat: number; rate: number; change_on_bar: boolean; groups: Group[] };
  cues: { active: { cue: string; layer: number; speed_group: number }[] };
}
const frame = () => studio.get<SpeedFrame>('/api/frame');
let cells: string[] = [];

test.beforeAll(async () => {
  const g = await studio.get<Grid>('/api/grid');
  cells = g.pages[0].cells.filter((c): c is string => !!c);
});

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/control', { id: 'page.1' });
  const x1 = { type: 'multiple', mult: 1 };
  expect(await studio.post('/api/speed-groups', { groups: [x1, x1, x1, x1], rate: 1, change_on_bar: true })).toBe(200);
  await openUi(page, studio);
});

test('a cue in group A at ×½ moves half as fast as the master; Cadence drives master.rate', async ({ page }) => {
  const [a, b] = cells;
  // Cue b on layer 2 and in group A, from its properties.
  await page.locator(`#cues .cue[data-id="${b}"]`).click({ button: 'right' });
  await expect(page.locator('#cueMenu')).toBeVisible();
  await page.locator('#cueMenuLayer').selectOption('2');
  await page.locator('#cueMenuSpeed').selectOption('1');
  await expect.poll(async () => { const sl = (await studio.get('/api/cues')).slots[b]; return [sl?.layer, sl?.speed_group]; }).toEqual([2, 1]);
  await page.locator('#cueMenuClose').click();

  // LIVE › Direct › Vitesses: at once (not on the bar), then A at ×½.
  await reveal(page, '#speedGroups');
  await page.locator('#speedOnBar').uncheck();
  await expect.poll(async () => (await frame()).tempo.change_on_bar).toBe(false);
  const half = page.locator('#speedGroups .spg[data-g="1"] [data-mult="1"]');
  await half.click();
  await expect.poll(async () => (await frame()).tempo.groups[0].mult).toBe(0.5);
  await expect(half).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#speedHint')).toHaveText('A ×½');

  // Both cues play; the frame says which group each one is in.
  for (const id of [a, b]) expect(await studio.post('/api/cue', { id, mode: 'toggle' })).toBe(200);
  await expect.poll(async () => (await frame()).cues.active.map(c => [c.cue, c.speed_group]).sort()).toEqual([[a, 0], [b, 1]].sort());

  // Over a few beats, group A advances half as far as the master clock.
  const f0 = await frame();
  await page.waitForTimeout(2_000);
  const f1 = await frame();
  const master = f1.tempo.effect_beat - f0.tempo.effect_beat;
  const groupA = (f1.tempo.groups[0].beat ?? 0) - (f0.tempo.groups[0].beat ?? 0);
  expect(master).toBeGreaterThan(2);
  expect(Math.abs(groupA / master - 0.5)).toBeLessThan(0.01);
  // The other groups stay on the master.
  expect(Math.abs(((f1.tempo.groups[1].beat ?? 0) - (f0.tempo.groups[1].beat ?? 0)) / master - 1)).toBeLessThan(0.01);

  // Cadence: 150 % → master.rate 1.5; the top bar recalls it.
  const rate = page.locator('#speedRate');
  await expect(rate).toHaveAttribute('data-control', 'master.rate');
  await rate.fill('150');
  await expect.poll(async () => (await studio.controlValues()).values['master.rate']).toBe(1.5);
  await expect(page.locator('#speedRateV')).toHaveText('150 %');
  await expect(page.locator('#speedHint')).toHaveText('A ×½ · Cadence 150 %');
  // Double-click on its name: back to 100 %.
  await page.locator('label[for="speedRate"]').dblclick();
  await expect.poll(async () => (await studio.controlValues()).values['master.rate']).toBe(1);

  // A free BPM for group B: it no longer follows the master tempo.
  await page.locator('#speedGroups .spg[data-g="2"] [data-free]').check();
  await expect.poll(async () => (await frame()).tempo.groups[1].free).toBe(true);
  const bpmB = (await frame()).tempo.groups[1].bpm;
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 90 })).toBe(200);
  await expect.poll(async () => (await frame()).tempo.groups[0].bpm).toBe(45);
  expect((await frame()).tempo.groups[1].bpm).toBe(bpmB);

  expect((await frame()).armed).toBe(false);
});

test('a layer in a speed group, and a new multiple waits for the bar', async ({ page }) => {
  await reveal(page, '#layers');
  const sel = page.locator('#layers .layer[data-layer="3"] select.spd');
  await sel.selectOption('2');
  await expect.poll(async () => (await studio.get('/api/layers')).layers[2].speed_group).toBe(2);
  // With « Appliquer au début de la mesure » on, ×2 is waiting or landed.
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 60 })).toBe(200);
  expect(await studio.post('/api/control', { id: 'speed.2.mult', value: 3 })).toBe(200);
  const g = await studio.get<{ groups: (Group & { pending: unknown })[] }>('/api/speed-groups');
  expect(g.groups[1].mult === 2 || g.groups[1].pending !== null).toBe(true);
  await expect.poll(async () => (await frame()).tempo.groups[1].mult, { timeout: 10_000 }).toBe(2);
  await expect(page.locator('#speedHint')).toHaveText('B ×2');
  expect((await frame()).armed).toBe(false);
});
