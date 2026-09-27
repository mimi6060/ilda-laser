// Cue layers (T-156): a cue's layer from « Propriétés du cue », two cues
// on two layers both reaching the frame, the layer strips (dimmer, Muet,
// Solo, Vider) and the point budget with four heavy layers.
import { test, expect, useStudio, openUi, isLit, type Point } from '../studio';

const studio = useStudio();
let catalog: Awaited<ReturnType<typeof studio.presets>>;

interface Mix { demand: number; points: number; budget: number; decimated: boolean; dropped: number[]; per_layer: number[] }
interface LayerFrame { points: Point[]; cues: { active: { cue: string; layer: number }[]; shown: string[] }; layers: { mix: Mix } }
const frame = () => studio.get<LayerFrame>('/api/frame');
const mix = async () => (await frame()).layers.mix;
const litCount = async () => (await frame()).points.filter(isLit).length;
const strip = (page: import('@playwright/test').Page, n: number) => page.locator(`#layers .layer[data-layer="${n}"]`);

test.beforeAll(async () => { catalog = await studio.presets(); });

test.beforeEach(async ({ page }) => {
  // Stop the cues first: the reset look then shows again.
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.multi', value: false });
  await studio.post('/api/layers', {});
  // Cues given a layer by an earlier test go back to layer 1.
  const slots = (await studio.get('/api/cues')).slots as Record<string, unknown>;
  for (const id of Object.keys(slots)) await studio.post('/api/cues/slot', { id });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('shows four layer strips with the point counter', async ({ page }) => {
  await expect(page.locator('#layers .layer')).toHaveCount(4);
  await expect(strip(page, 3).locator('.t')).toHaveText('Calque 3');
  await expect(page.locator('#layerPoints')).toContainText(/Points : \d+ \/ 750/);
  await expect(page.locator('#layerPoints')).not.toHaveClass(/over/);
});

test('two cues on two layers both reach the frame; dimmer 0, solo, mute and Vider', async ({ page }) => {
  const [a, b] = catalog.presets.filter(p => p.category === catalog.categories[0]);
  // « Propriétés du cue » → Calque 2 for b.
  await page.locator(`#cues .cue[data-id="${b.id}"]`).click({ button: 'right' });
  await page.locator('#cueMenuLayer').selectOption('2');
  await expect.poll(async () => (await studio.get('/api/cues')).slots[b.id]?.layer).toBe(2);
  await expect(page.locator(`#cues .cue[data-id="${b.id}"] .tag`)).toHaveText('C2');
  await page.locator('#cueMenuClose').click();

  // « Un cue » replaces within a layer only: a (layer 1) and b (layer 2) both play.
  await page.locator(`#cues .cue[data-id="${a.id}"]`).click();
  await page.locator(`#cues .cue[data-id="${b.id}"]`).click();
  await expect.poll(async () => (await frame()).cues.shown.sort()).toEqual([a.id, b.id].sort());
  await expect.poll(async () => {
    const m = await mix();
    return m.per_layer[0] > 0 && m.per_layer[1] > 0 && m.demand > Math.max(m.per_layer[0], m.per_layer[1]);
  }).toBe(true);
  expect((await frame()).cues.active.map(c => c.layer).sort()).toEqual([1, 2]);

  // Dimmer of layer 2 at 0: only layer 1's points are lit.
  await strip(page, 2).locator('input[type=range]').fill('0');
  await expect.poll(async () => (await studio.controlValues()).values['layer.2.dimmer']).toBe(0);
  await expect(strip(page, 2)).toHaveClass(/off/);
  await expect.poll(async () => { const m = await mix(); return m.demand <= m.per_layer[0]; }).toBe(true);
  expect(await litCount()).toBeGreaterThan(0);
  expect(await litCount()).toBeLessThanOrEqual((await mix()).per_layer[0]);
  await strip(page, 2).locator('input[type=range]').fill('100');
  await expect.poll(async () => (await studio.controlValues()).values['layer.2.dimmer']).toBe(1);

  // Solo layer 2: only layer 2 plays.
  await strip(page, 2).locator('.solo').click();
  await expect(strip(page, 2).locator('.solo')).toHaveClass(/active/);
  await expect(strip(page, 1)).toHaveClass(/off/);
  await expect.poll(async () => { const m = await mix(); return m.demand <= m.per_layer[1]; }).toBe(true);
  await strip(page, 2).locator('.solo').click();
  await expect.poll(async () => (await studio.controlValues()).values['layer.2.solo']).toBe(false);

  // Mute layer 1: only layer 2 plays.
  await strip(page, 1).locator('.mute').click();
  await expect(strip(page, 1).locator('.mute')).toHaveClass(/active/);
  await expect.poll(async () => { const m = await mix(); return m.demand <= m.per_layer[1]; }).toBe(true);
  await strip(page, 1).locator('.mute').click();
  await expect.poll(async () => (await studio.controlValues()).values['layer.1.mute']).toBe(false);

  // Vider layer 2: its cue stops, layer 1's keeps playing.
  await strip(page, 2).locator('.clear').click();
  await expect.poll(async () => (await frame()).cues.active.map(c => c.cue)).toEqual([a.id]);
  await expect(page.locator(`#cues .cue[data-id="${a.id}"]`)).toHaveClass(/active/);
  await expect(page.locator(`#cues .cue[data-id="${b.id}"]`)).not.toHaveClass(/active/);
});

test('a layer dimmer moved from outside (MIDI/API) shows on its strip', async ({ page }) => {
  expect(await studio.post('/api/control', { id: 'layer.3.dimmer', norm: 0.4 })).toBe(200);
  expect(await studio.post('/api/control', { id: 'layer.4.mute', value: true })).toBe(200);
  await expect(strip(page, 3).locator('input[type=range]')).toHaveValue('40');
  await expect(strip(page, 3).locator('.dv')).toHaveText('40 %');
  await expect(strip(page, 4).locator('.mute')).toHaveClass(/active/);
});

test('four heavy layers stay within the point budget, with a warning', async ({ page }) => {
  // Heavy but each under the budget alone: 400..700 points.
  const heavy: string[] = [];
  for (const p of catalog.presets) {
    if (heavy.length === 4) break;
    await studio.post('/api/cue', { id: p.id, mode: 'restart' });
    await new Promise(r => setTimeout(r, 60));
    let most = 0;
    for (let i = 0; i < 4; i++) { most = Math.max(most, (await mix()).demand); await new Promise(r => setTimeout(r, 40)); }
    if (most >= 400 && most <= 700) heavy.push(p.id);
  }
  expect(heavy).toHaveLength(4);
  await studio.post('/api/control', { id: 'cue.stop_all' });
  for (const [i, id] of heavy.entries()) {
    expect(await studio.post('/api/cues/slot', { id, layer: i + 1 })).toBe(200);
    expect(await studio.post('/api/cue', { id })).toBe(200);
  }
  await expect.poll(async () => (await frame()).cues.shown.length).toBe(4);
  // The mix report comes from the engine's next frame.
  await expect.poll(async () => (await mix()).per_layer.every(n => n > 0)).toBe(true);

  for (let i = 0; i < 5; i++) {
    const f = await frame();
    expect(f.layers.mix.demand).toBeGreaterThan(750);
    expect(f.points.length).toBeLessThanOrEqual(750);
    expect(f.layers.mix.dropped.length).toBeGreaterThan(0);
    expect(f.layers.mix.dropped).not.toContain(1);
    await new Promise(r => setTimeout(r, 50));
  }
  await expect(page.locator('#layerPoints')).toHaveClass(/over/);
  await expect(page.locator('#layerWarn')).toContainText('budget dépassé');
  await expect(strip(page, 4).locator('.pts')).toHaveText('coupé');
  expect(Number(await page.locator('#stPoints').textContent())).toBeLessThanOrEqual(750);
});
