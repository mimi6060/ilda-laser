// T-422: presets the cues refer to (position, size, colour, motion). A
// cue that references « Plafond » follows it, even while it plays; a cue
// with its own values doesn't move. A preset in use can't be deleted; a
// rename keeps every reference; the project brings both back.
// Preview only: nothing here arms the laser.
import { test, expect, useStudio, openUi, reveal, centroid } from '../studio';

const studio = useStudio();
type Pool = { id: string; name: string; family: string; pos_x?: number; pos_y?: number; size?: number };
type Pools = { presets: Pool[]; uses: Record<string, string[]> };
type Frame = { armed: boolean; points: [number, number, number, number, number][]; cues: { active: { cue: string }[] } };
type Slot = { refs?: Record<string, string> };
const pools = () => studio.get<Pools>('/api/pools');
const frame = () => studio.get<Frame>('/api/frame');
const slots = async () => (await studio.get('/api/cues')).slots as Record<string, Slot>;
let cells: string[] = [];
async function capture(body: { family: string; name: string }): Promise<Pool> {
  const r = await fetch(`${studio.url}/api/pools/capture`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  expect(r.status).toBe(200);
  return r.json();
}

/** Mean y of the lit points over a few frames. */
async function meanY() {
  let sum = 0;
  for (let i = 0; i < 4; i++) {
    sum += centroid((await frame()).points).y;
    await new Promise(r => setTimeout(r, 40));
  }
  return sum / 4;
}

async function setPlafond(y: number) {
  const list = (await pools()).presets.map(p => p.id === 'position:plafond' ? { ...p, pos_y: y } : p);
  expect(await studio.post('/api/pools', list)).toBe(200);
}

test.beforeAll(async () => {
  const g = await studio.get<{ pages: { cells: (string | null)[] }[] }>('/api/grid');
  cells = g.pages[0].cells.filter((c): c is string => !!c);
});

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/control', { id: 'smoothing.on', value: 0 });
  // No references, no presets: each test starts clean.
  for (const id of cells.slice(0, 3)) expect(await studio.post('/api/cues/slot', { id })).toBe(200);
  expect(await studio.post('/api/pools', [])).toBe(200);
  await openUi(page, studio);
});

test('« Plafond » from the live moves the two cues that refer to it, not a third', async ({ page }) => {
  const [a, b, c] = cells;
  // Made from the live: the position family only.
  expect(await studio.post('/api/control', { id: 'master.pos_y', value: 0.25 })).toBe(200);
  expect(await studio.post('/api/control', { id: 'master.size', value: 0.6 })).toBe(200);
  await reveal(page, '#poolPanel');
  await page.locator('#poolName').fill('Plafond');
  await page.locator('[data-pool-add="position"]').click();
  await expect(page.locator('#poolMsg')).toContainText('« Plafond » enregistré');
  const made = (await pools()).presets;
  expect(made.map(p => [p.id, p.family])).toEqual([['position:plafond', 'position']]);
  expect(made[0].pos_y).toBeCloseTo(0.25, 3);
  expect(made[0].size).toBeUndefined();
  await studio.post('/api/control', { id: 'master.reset' });

  // Cue a from « Propriétés du cue », cue b through the API.
  await page.locator(`#cues .cue[data-id="${a}"]`).click({ button: 'right' });
  await expect(page.locator('#cueMenu')).toBeVisible();
  await expect(page.locator('#cueMenuRefPosition option').first()).toHaveText('Valeurs du cue');
  await page.locator('#cueMenuRefPosition').selectOption('position:plafond');
  await expect.poll(async () => (await slots())[a]?.refs?.position).toBe('position:plafond');
  await page.locator('#cueMenuClose').click();
  await expect(page.locator(`#cues .cue[data-id="${a}"] .tag`)).toContainText('↗');
  await expect(page.locator(`#cues .cue[data-id="${a}"]`)).toHaveAttribute('title', /Préréglages : Plafond/);
  expect(await studio.post('/api/cues/slot', { id: b, refs: { position: 'position:plafond' } })).toBe(200);
  // A reference to an unknown preset is refused from the page's route.
  expect(await studio.post('/api/cues/slot', { id: c, refs: { position: 'position:absent' } })).toBe(400);
  expect(await studio.post('/api/cues/slot', { id: c, refs: { size: 'position:plafond' } })).toBe(400);
  await page.locator('#liveTabs [data-panel-tab="scenes"]').click();
  await page.locator('#liveTabs [data-panel-tab="pools"]').click();
  await expect(page.locator('.pool-chip[data-pool="position:plafond"]')).toContainText('2 cues');

  // Frozen, so only the preset moves the pictures.
  expect(await studio.post('/api/control', { id: 'transport.freeze', value: 1 })).toBe(200);
  for (const [id, follows] of [[a, true], [b, true], [c, false]] as const) {
    await studio.post('/api/control', { id: 'cue.stop_all' });
    await setPlafond(-0.25);
    expect(await studio.post('/api/cue', { id })).toBe(200);
    await expect.poll(async () => (await frame()).cues.active.map(x => x.cue)).toContain(id);
    const before = await meanY();
    // Moved while the cue plays.
    await setPlafond(0.25);
    if (follows) {
      await expect.poll(async () => (await meanY()) - before, { message: `cue ${id} follows « Plafond »` }).toBeGreaterThan(0.2);
    } else {
      await new Promise(r => setTimeout(r, 300));
      expect(Math.abs((await meanY()) - before), `cue ${id} keeps its own values`).toBeLessThan(0.1);
    }
  }
  await studio.post('/api/control', { id: 'transport.freeze', value: 0 });
  expect((await frame()).armed).toBe(false);
});

test('a preset in use can\'t be deleted, and says by whom; unused, it goes', async ({ page }) => {
  const [a] = cells;
  const p = await capture({ family: 'size', name: 'Petit' });
  expect(p.id).toBe('size:petit');
  expect(await studio.post('/api/cues/slot', { id: a, refs: { size: p.id } })).toBe(200);
  await reveal(page, '#poolPanel');
  await page.locator('.pool-chip[data-pool="size:petit"]').click();
  await page.locator('#poolDelete').click();
  await expect(page.locator('#poolMsg')).toContainText('Préréglage utilisé par : cue « ');
  const r = await fetch(`${studio.url}/api/pools/delete`, { method: 'POST', body: JSON.stringify({ id: p.id }) });
  expect(r.status).toBe(409);
  expect((await r.json()).users).toHaveLength(1);
  expect((await pools()).presets).toHaveLength(1);

  expect(await studio.post('/api/cues/slot', { id: a })).toBe(200);
  await page.locator('#poolDelete').click();
  await expect(page.locator('#poolMsg')).toContainText('« Petit » supprimé');
  await expect(page.locator('.pool-chip')).toHaveCount(0);
  expect((await pools()).presets).toEqual([]);
  // Undone like any edit.
  expect(await studio.post('/api/undo')).toBe(200);
  expect((await pools()).presets.map(x => x.id)).toEqual(['size:petit']);
});

test('a rename keeps the references; the project brings presets and references back', async ({ page }) => {
  const [a] = cells;
  const p = await capture({ family: 'position', name: 'Écran' });
  expect(await studio.post('/api/cues/slot', { id: a, refs: { position: p.id } })).toBe(200);
  await reveal(page, '#poolPanel');
  await page.locator(`.pool-chip[data-pool="${p.id}"]`).click();
  await expect(page.locator('#poolName')).toHaveValue('Écran');
  await page.locator('#poolName').fill('Écran gauche');
  await page.locator('#poolRename').click();
  await expect(page.locator('#poolMsg')).toContainText('Renommé');
  expect((await pools()).presets.map(x => [x.id, x.name])).toEqual([[p.id, 'Écran gauche']]);
  expect((await slots())[a].refs).toEqual({ position: p.id });

  expect(await studio.post('/api/project/save-as', { path: 'Prereglages e2e' })).toBe(200);
  expect(await studio.post('/api/cues/slot', { id: a })).toBe(200);
  expect(await studio.post('/api/pools', [])).toBe(200);
  expect(await studio.post('/api/project/open', { path: 'Prereglages e2e' })).toBe(200);
  expect((await pools()).presets.map(x => [x.id, x.name])).toEqual([[p.id, 'Écran gauche']]);
  expect((await slots())[a].refs).toEqual({ position: p.id });
  expect(await studio.post('/api/project/new')).toBe(200);
  expect((await frame()).armed).toBe(false);
});
