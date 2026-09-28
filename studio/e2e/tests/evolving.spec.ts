// Evolving cues (T-111): a look sent as keyframes over N beats plays by
// itself on the tempo clock, starts on a beat, loops, shows its progress
// under the preview, and survives a scene save + restart.
import { test, expect, useStudio, openUi, isLit } from '../studio';

const studio = useStudio();

interface Evolving { pos: number; length: number; key: number; keys: number; loop: boolean; waiting: boolean; ended: boolean; pass: number; beats_per_bar: number; cue: string | null; layer: number }
const evolving = async () => ((await studio.frame()) as unknown as { evolving: Evolving[] }).evolving;

// 4 beats at 240 BPM = 1 s: a small fan (key 0), then from beat 2 a wide
// red one (key 1, stepped), and back to key 0 on each loop. a = 0: the fan
// holds its width (no pump).
const CUE = {
  content: {
    kind: 'evolving', length_beats: 4, loop: true,
    keys: [
      { at_beats: 0, generator: 'fan', scale: 0.2, color: [0, 255, 0], ease: 'step', params: { count: 6, a: 0, b: 0.4 } },
      { at_beats: 2, generator: 'fan', scale: 0.9, color: [255, 0, 0], ease: 'step', params: { count: 6, a: 0, b: 0.4 } },
    ],
  },
  brightness: 1,
};

test.beforeEach(async () => {
  await studio.reset();
  await studio.post('/api/control', { id: 'tempo.bpm', value: 240 });
});

test('an evolving look plays its keys on the beat and loops', async () => {
  expect(await studio.post('/api/settings', CUE)).toBe(200);
  await expect.poll(async () => (await evolving()).length).toBe(1);
  const first = (await evolving())[0];
  expect(first).toMatchObject({ length: 4, keys: 2, loop: true, cue: null, layer: 1 });

  // Launched on a beat: the clock and the cue's position are a whole
  // number of beats apart (a frame is 1/15 beat at 240 BPM).
  await expect.poll(async () => {
    const f = await studio.frame() as unknown as { evolving: Evolving[]; tempo: { beat: number } };
    const e = f.evolving[0];
    if (!e || e.waiting) return 1;
    const off = (f.tempo.beat - e.pos) % 1;
    return Math.min(off, 1 - off);
  }).toBeLessThan(0.2);

  // Two instants: key 0 (small green fan), then key 1 (wide red fan).
  const look = async (key: number) => {
    for (;;) {
      const f = await studio.frame() as unknown as { evolving: Evolving[]; points: [number, number, number, number, number][] };
      if (f.evolving[0]?.key === key && !f.evolving[0].waiting && f.points.some(isLit)) return f;
    }
  };
  const small = await look(0);
  const wide = await look(1);
  expect(wide.evolving[0].pos).toBeGreaterThanOrEqual(2);
  // A fan is as wide as its size (its height is fixed): compare widths.
  const width = (pts: [number, number, number, number, number][]) => Math.max(...pts.filter(isLit).map(p => Math.abs(p[0])));
  expect(width(wide.points)).toBeGreaterThan(width(small.points) + 0.3);
  expect(wide.points.filter(isLit).every(p => p[2] > 0 && p[3] === 0)).toBe(true);
  expect(small.points.filter(isLit).every(p => p[3] > 0 && p[2] === 0)).toBe(true);

  // Loop: back on key 0 and a new pass.
  const pass = wide.evolving[0].pass;
  await expect.poll(async () => {
    const e = (await evolving())[0];
    return e.pass > pass && e.key === 0;
  }).toBe(true);

  // Another look: no evolving cue any more.
  await studio.reset();
  await expect.poll(async () => (await evolving()).length).toBe(0);
});

test('the preview shows the evolving cue\'s progress', async ({ page }) => {
  await openUi(page, studio);
  await expect(page.locator('#evoBar')).toBeHidden();
  expect(await studio.post('/api/settings', CUE)).toBe(200);
  await expect(page.locator('#evoBar')).toBeVisible();
  await expect(page.locator('#evoPos')).toContainText('/ 4 temps');
  await expect(page.locator('#evoState')).toContainText(/clé [12]\/2 · boucle/);
  await expect(page.locator('#evoPanel')).toBeVisible();
  await expect(page.locator('#evoPanel')).toContainText('2 clés sur 4 temps, en boucle');
  // The bar fills as the cue plays.
  await expect.poll(async () => page.locator('#evoFill').evaluate(e => parseFloat((e as HTMLElement).style.width))).toBeGreaterThan(40);
  // Editing the look (brightness) keeps the cue and its keys.
  await page.locator('#bright').fill('40');
  await page.locator('#bright').dispatchEvent('input');
  await expect.poll(async () => (await studio.state()).settings.brightness).toBeCloseTo(0.4, 2);
  expect((await studio.state()).settings.content.kind).toBe('evolving');
  await studio.reset();
  await expect(page.locator('#evoBar')).toBeHidden();
});

test('an evolving cue saved in a scene comes back after a restart', async () => {
  expect(await studio.post('/api/settings', CUE)).toBe(200);
  expect(await studio.post('/api/scenes/save', { name: 'Montée test', duration_secs: 8 })).toBe(200);
  await studio.restart();
  await studio.post('/api/control', { id: 'tempo.bpm', value: 240 });
  const scene = (await studio.state()).scenes.find((s: { name: string }) => s.name === 'Montée test');
  expect(scene.settings.content).toMatchObject({ kind: 'evolving', length_beats: 4, loop: true });
  expect(scene.settings.content.keys).toHaveLength(2);
  expect(await studio.post('/api/scenes/play', { name: 'Montée test' })).toBe(200);
  await expect.poll(async () => (await evolving())[0]?.keys).toBe(2);
  await studio.post('/api/scenes/delete', { name: 'Montée test' });
});
