// Timeline envelopes (T-163): a lane under a track (∿) where double-click
// adds a node, dragging moves it, right click deletes it; Absolu / Relatif;
// a Bus track whose envelopes act on its whole layer; an envelope on an
// unknown parameter is kept but ignored, with a warning. The preview shows
// a brightness envelope dimming the event. Preview only: nothing arms.
import { test, expect, useStudio, openUi, openWorkspace, isLit, type Page, type Point } from '../studio';

const studio = useStudio();

interface Key { at: number; value: number; ease: string }
interface Env { target: string; mode: string; keys: Key[] }
interface Ev { id: number; start: number; len: number; source: { kind: string; id: string }; envelopes?: Env[] }
interface Show { name: string; tracks: { name: string; kind: string; layer: number; events: Ev[] }[] }
interface EnvView { id: string; mode: string; min: number; max: number; y0: number; y1: number }
const serverShow = async () => (await studio.get<{ show: Show }>('/api/timeline')).show;
const lit = async () => (await studio.get<{ points: Point[] }>('/api/frame')).points.filter(isLit).length;
let cue = '';

test.beforeAll(async () => {
  cue = (await studio.presets()).presets[0].id;
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/timeline/stop');
  await studio.post('/api/estop/reset');
  await studio.reset();
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/timeline/stop');
  expect((await studio.state()).armed, 'envelopes never arm').toBe(false);
});

/** A Secondes show (120 BPM: 1 beat = 0.5 s) with one 8 s event on track 1. */
async function seed(name: string, envelopes: Env[] = []) {
  expect(await studio.post('/api/timeline/new', { name, time_base: 'seconds' })).toBe(200);
  const show = await serverShow();
  show.tracks[0].events = [{ id: 1, start: 0, len: 8, source: { kind: 'cue', id: cue }, envelopes }];
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
}

async function openEditor(page: Page, name: string) {
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe(name);
  await page.evaluate(() => (window as any).tlDebug.setView(0, 40)); // 40 px per second
}

/** Page point of show time `u` at `value` in the envelope lane of track `ti`. */
async function envAt(page: Page, ti: number, u: number, value: number) {
  const env = await page.evaluate(i => (window as any).tlDebug.env(i), ti) as EnvView;
  expect(env, 'the envelope lane is open').not.toBeNull();
  const view = await page.evaluate(() => (window as any).tlDebug.view());
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  const y = env.y1 - (value - env.min) / (env.max - env.min) * (env.y1 - env.y0);
  return { x: box.x + (u - view.t0) * view.pps, y: box.y + y };
}
/** Page point of show time `u` in the event row of track `ti`. */
async function rowAt(page: Page, ti: number, u: number) {
  const top = await page.evaluate(i => (window as any).tlDebug.laneY(i), ti) as number;
  const view = await page.evaluate(() => (window as any).tlDebug.view());
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  return { x: box.x + (u - view.t0) * view.pps, y: box.y + top + view.lane / 2 };
}
const keys = async (target: string, track = 0) => {
  const e = (await serverShow()).tracks[track].events[0];
  return ((e.envelopes || []).find(v => v.target === target) || { keys: [] as Key[] }).keys;
};

test('draw an envelope: double-click adds nodes, drag moves one, right click deletes one', async ({ page }) => {
  await seed('Enveloppe e2e');
  await openEditor(page, 'Enveloppe e2e');
  await page.locator('#tlHeads .tl-head[data-ti="0"] button[data-f="env"]').first().click();
  await expect(page.locator('#tlHeads .tl-envhead')).toHaveCount(1);
  await page.locator('#tlHeads .tl-envhead select[data-f="envt"]').selectOption('look.size');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.env(0)?.id)).toBe('look.size');

  // Two nodes (size is 0.05..1): 0.3 at 2 s, 0.9 at 6 s (the magnet keeps them on the beats).
  let p = await envAt(page, 0, 2, 0.3);
  await page.mouse.dblclick(p.x, p.y);
  await expect.poll(async () => (await keys('look.size')).length).toBe(1);
  p = await envAt(page, 0, 6, 0.9);
  await page.mouse.dblclick(p.x, p.y);
  await expect.poll(async () => (await keys('look.size')).length).toBe(2);
  let k = await keys('look.size');
  expect(k.map(q => q.at)).toEqual([2, 6]);
  expect(k[0].value).toBeCloseTo(0.3, 1);
  expect(k[1].value).toBeCloseTo(0.9, 1);
  expect(k[0].ease).toBe('linear');
  await page.screenshot({ path: 'test-results/timeline-envelopes.png' });

  // Drag the second node down to 0.6: its time stays put.
  const from = await envAt(page, 0, 6, k[1].value), to = await envAt(page, 0, 6, 0.6);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(to.x, (from.y + to.y) / 2, { steps: 3 });
  await page.mouse.move(to.x, to.y, { steps: 3 });
  await page.mouse.up();
  await expect.poll(async () => (await keys('look.size'))[1]?.value ?? 1).toBeLessThan(0.75);
  k = await keys('look.size');
  expect(k[1].at).toBe(6);
  expect(k[1].value).toBeCloseTo(0.6, 1);

  // Double-click on a node: its next curve (Douce).
  p = await envAt(page, 0, 2, k[0].value);
  await page.mouse.dblclick(p.x, p.y);
  await expect.poll(async () => (await keys('look.size'))[0].ease).toBe('smooth');
  await expect(page.locator('#tlEdMsg')).toContainText('Douce');

  // Right click deletes the first node.
  p = await envAt(page, 0, 2, k[0].value);
  await page.mouse.click(p.x, p.y, { button: 'right' });
  await expect.poll(async () => (await keys('look.size')).length).toBe(1);

  // Relatif: the envelope multiplies the parameter.
  await page.locator('#tlHeads .tl-envhead select[data-f="envm"]').selectOption('relative');
  await expect.poll(async () => (await serverShow()).tracks[0].events[0].envelopes![0].mode).toBe('relative');
  // Undo brings the absolute mode back.
  await page.locator('#tlUndo').click();
  await expect.poll(async () => (await serverShow()).tracks[0].events[0].envelopes![0].mode).toBe('absolute');
});

test('a brightness envelope at 0 blacks the event out in the preview, and the show stays disarmed', async () => {
  await seed('Noir e2e');
  await studio.post('/api/timeline/play');
  await expect.poll(lit).toBeGreaterThan(0);
  const show = await serverShow();
  show.tracks[0].events[0].envelopes = [{ target: 'master.brightness', mode: 'absolute', keys: [{ at: 0, value: 0, ease: 'linear' }] }];
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
  await expect.poll(lit).toBe(0);
  // A brightness envelope only dims: 2 in Absolu is the event's own level.
  show.tracks[0].events[0].envelopes = [{ target: 'master.brightness', mode: 'absolute', keys: [{ at: 0, value: 2, ease: 'linear' }] }];
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
  await expect.poll(lit).toBeGreaterThan(0);
  expect((await studio.state()).armed).toBe(false);
});

test('a Bus track: its event plays nothing and its envelope dims the whole layer', async ({ page }) => {
  await seed('Bus e2e');
  await openEditor(page, 'Bus e2e');
  // Track 2 → Bus on layer 1 (where track 1 plays): its lane opens by itself.
  const head = page.locator('#tlHeads .tl-head[data-ti="1"]').first();
  await head.locator('select[data-f="layer"]').selectOption('1');
  await expect.poll(async () => (await serverShow()).tracks[1].layer).toBe(1);
  await page.locator('#tlHeads .tl-head[data-ti="1"]').first().locator('select[data-f="kind"]').selectOption('bus');
  await expect.poll(async () => (await serverShow()).tracks[1].kind).toBe('bus');
  await expect(page.locator('#tlHeads .tl-envhead[data-ti="1"]')).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.env(1)?.id)).toBe('master.brightness');

  // Double-click on the bus row: a bus event (4 bars = 8 s), then a node at 0 in its lane.
  const r = await rowAt(page, 1, 0.2);
  await page.mouse.dblclick(r.x, r.y);
  await expect.poll(async () => (await serverShow()).tracks[1].events.length).toBe(1);
  const bus = (await serverShow()).tracks[1].events[0];
  expect([bus.start, bus.len, bus.source.id]).toEqual([0, 8, '']);
  const p = await envAt(page, 1, 1, -0.1); // just under the bottom: exactly 0
  await page.mouse.dblclick(p.x, p.y);
  await expect.poll(async () => (await keys('master.brightness', 1)).length).toBe(1);
  expect((await keys('master.brightness', 1))[0].value).toBe(0);

  // Playing: the bus blacks out track 1's event; mute the bus and it's back.
  await studio.post('/api/timeline/play');
  await expect.poll(lit).toBe(0);
  await page.locator('#tlHeads .tl-head[data-ti="1"]').first().locator('button[data-f="mute"]').click();
  await expect.poll(lit).toBeGreaterThan(0);
});

test('an envelope on an unknown parameter is kept, ignored, and reported', async ({ page }) => {
  await seed('Inconnu e2e', [{ target: 'nope.param', mode: 'absolute', keys: [{ at: 0, value: 0, ease: 'linear' }] }]);
  await openEditor(page, 'Inconnu e2e');
  // The lane opens on it, labelled as unknown.
  await expect(page.locator('#tlHeads .tl-envhead select[data-f="envt"]')).toHaveValue('nope.param');
  await expect(page.locator('#tlHeads .tl-envhead select[data-f="envt"] option:checked')).toContainText('inconnu');
  // Any edit from the page reports it.
  await page.locator('#tlHeads .tl-head[data-ti="0"] input[data-f="name"]').fill('Renommée');
  await page.locator('#tlHeads .tl-head[data-ti="0"] input[data-f="name"]').press('Enter');
  await expect(page.locator('#tlEdMsg')).toContainText('enveloppe ignorée');
  await expect(page.locator('#tlEdMsg')).toContainText('nope.param');
  // Ignored at playback: the event still lights the preview.
  await studio.post('/api/timeline/play');
  await expect.poll(lit).toBeGreaterThan(0);
  expect((await serverShow()).tracks[0].events[0].envelopes![0].target).toBe('nope.param');
});
