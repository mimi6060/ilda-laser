// Projection zones (T-012): RÉGLAGES › Zones de projection edits each
// zone's geometry, « Propriétés du cue » routes a cue to several zones,
// the 4-corner warp is dragged on the canvas, and show mode refuses every
// edit. Preview only: the studio stays disarmed throughout.
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';

const studio = useStudio();

interface Zone { id: number; name: string; enabled: boolean; output: string | null; geometry: Record<string, any> }
interface Proj { config: { zones: Zone[]; default_zones: number[] }; align: number | null; max_zones: number }
const proj = () => studio.get<Proj>('/api/projection');
const lit = async () => (await studio.frame()).points.filter(isLit) as Point[];

async function openZones(page: import('@playwright/test').Page) {
  const summary = '#pzPanel summary';
  await reveal(page, summary);
  await page.locator(summary).click();
  await expect(page.locator('#pzList button')).not.toHaveCount(0);
}

let cueId = '';

test.beforeAll(async () => {
  cueId = (await studio.presets()).presets[0].id;
});

test.beforeEach(async ({ page }) => {
  await studio.reset();
  expect(await studio.post('/api/projection', { zones: [{ id: 1, name: 'Principale' }], default_zones: [1] })).toBe(200);
  expect(await studio.post('/api/projection/align', { zone: null })).toBe(200);
  expect(await studio.post('/api/cues/slot', { id: cueId })).toBe(200);
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/lock', { show_mode: false });
  await studio.post('/api/testpattern', { pattern: null });
  await studio.post('/api/control', { id: 'cue.stop_all' });
});

test('zones are added and shaped from the panel, and a cue plays on two of them', async ({ page }) => {
  await openZones(page);
  await expect(page.locator('#pzList button')).toHaveText(['1 · Principale★']);
  await expect(page.locator('#pzDelete')).toBeDisabled();

  // Zone 1 → the left half.
  await page.locator('#pz_size_x').fill('0.5');
  await page.locator('#pz_pos_x').fill('-0.5');
  await expect.poll(async () => (await proj()).config.zones[0].geometry.pos_x).toBe(-0.5);
  expect((await proj()).config.zones[0].geometry.size_x).toBe(0.5);

  // Zone 2 → the right half, named.
  await page.locator('#pzAdd').click();
  await expect(page.locator('#pzList button')).toHaveCount(2);
  await expect(page.locator('#pzList button.active')).toHaveText('2 · Zone 2');
  await page.locator('#pzName').fill('Droite');
  await page.locator('#pzName').press('Enter');
  await page.locator('#pz_size_x').fill('0.5');
  await page.locator('#pz_pos_x').fill('0.5');
  await expect.poll(async () => (await proj()).config.zones.map(z => [z.name, z.geometry.pos_x])).toEqual([['Principale', -0.5], ['Droite', 0.5]]);

  // A cue without zones plays on the default zone only: the left half.
  expect(await studio.post('/api/cue', { id: cueId })).toBe(200);
  await expect.poll(async () => { const l = await lit(); return l.length > 0 && l.every(p => p[0] <= 1e-4); }).toBe(true);

  // « Propriétés du cue »: zones 1 and 2 → drawn in both halves.
  await reveal(page, `#cues .cue[data-id="${cueId}"]`);
  await page.locator(`#cues .cue[data-id="${cueId}"]`).click({ button: 'right' });
  await expect(page.locator('#cueMenuZones input')).toHaveCount(2);
  await page.locator('#cueMenuZones input[value="1"]').check();
  await page.locator('#cueMenuZones input[value="2"]').check();
  await page.locator('#cueMenuClose').click();
  await expect.poll(async () => (await studio.get('/api/cues')).slots[cueId]?.zones).toEqual([1, 2]);
  await expect.poll(async () => {
    const l = await lit();
    return l.some(p => p[0] < -0.02) && l.some(p => p[0] > 0.02);
  }).toBe(true);

  // Zone 2 turned off: back to the left half only.
  await reveal(page, '#pzList button[data-zone="2"]');
  await page.locator('#pzList button[data-zone="2"]').click();
  await page.locator('#pzEnabled').uncheck();
  await expect.poll(async () => { const l = await lit(); return l.length > 0 && l.every(p => p[0] <= 1e-4); }).toBe(true);

  const st = await studio.state();
  expect(st.armed).toBe(false);
});

test('the warp corners are dragged on the canvas and the frame lands in the quad', async ({ page }) => {
  await openZones(page);
  await page.locator('#pzWarp').check();
  await expect.poll(async () => (await proj()).config.zones[0].geometry.warp.enabled).toBe(true);
  // Drag the top-right handle towards the centre.
  const box = (await page.locator('#pzCanvas').boundingBox())!;
  const m = 14 * box.width / 240;
  const [fromX, fromY] = [box.x + box.width - m, box.y + m];
  await page.mouse.move(fromX, fromY);
  await page.mouse.down();
  await page.mouse.move(fromX - box.width * 0.2, fromY + box.height * 0.2, { steps: 5 });
  await page.mouse.up();
  await expect.poll(async () => {
    const [x, y] = (await proj()).config.zones[0].geometry.warp.corners[2];
    return x < 0.8 && y < 0.8 && x > 0.3 && y > 0.3;
  }).toBe(true);
  const [cx, cy] = (await proj()).config.zones[0].geometry.warp.corners[2];

  // Nothing lit beyond the moved corner (outside the quad).
  expect(await studio.post('/api/cue', { id: cueId })).toBe(200);
  await expect.poll(async () => { const l = await lit(); return l.length > 0 && !l.some(p => p[0] > cx + 1e-3 && p[1] > cy + 1e-3); }).toBe(true);

  // A crossed quad is refused by the server.
  const crossed = { zones: [{ id: 1, geometry: { warp: { enabled: true, corners: [[-1, -1], [1, 1], [1, -1], [-1, 1]] } } }] };
  expect(await studio.post('/api/projection', crossed)).toBe(400);
  expect((await proj()).config.zones[0].geometry.warp.corners[2]).toEqual([cx, cy]);

  // « Aligner avec la mire »: the grid pattern, through this zone.
  await page.locator('#pzAlign').click();
  await expect(page.locator('#pzAlignState')).toHaveText('Mire à travers la zone 1');
  await expect.poll(async () => (await studio.get('/api/testpattern')).state.active).toBe('grid');
  await expect.poll(async () => { const l = await lit(); return l.length > 0 && !l.some(p => p[0] > cx + 1e-3 && p[1] > cy + 1e-3); }).toBe(true);
  expect((await studio.state()).armed).toBe(false);
});

test('show mode refuses zone edits', async ({ page }) => {
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  expect(await studio.post('/api/projection', { zones: [{ id: 1, geometry: { keystone_x: 0.5 } }] })).toBe(423);
  expect(await studio.post('/api/projection/align', { zone: 1 })).toBe(423);
  expect(await studio.post('/api/cues/slot', { id: cueId, zones: [1] })).toBe(423);
  expect((await proj()).config.zones[0].geometry.keystone_x).toBe(0);
  // RÉGLAGES (and so the zone panel) is not offered while locked.
  await page.reload();
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toBeHidden();
  expect((await studio.state()).armed).toBe(false);
});
