// Test patterns (T-404): each pattern chosen in RÉGLAGES › Calibration
// replaces the output frame, stays at or under the 20 % default brightness,
// and « Quitter la mire » gives back the cues and the timeline where they
// were. Preview only: the studio has no output and stays disarmed.
import { test, expect, useStudio, openUi, reveal, openWorkspace, focusPage, isLit, type Point } from '../studio';

const studio = useStudio();

interface TpState { active: string | null; brightness: number }
interface TpFrame { points: Point[]; armed: boolean; output_lit: number; test_pattern: TpState; cues: { active: { cue: string }[] }; timeline: { playing: boolean; paused: boolean; position: number } }
const frame = () => studio.get<TpFrame>('/api/frame');
const peak = (pts: Point[]) => Math.max(0, ...pts.map(p => Math.max(p[2], p[3], p[4])));
/** A fingerprint of the lit geometry. */
const shape = (pts: Point[]) => pts.filter(isLit).map(p => `${p[0]},${p[1]}`).join(';');

/** RÉGLAGES › Calibration, opened on the « Mires » controls. */
async function openPatterns(page: import('@playwright/test').Page) {
  const summary = '#settingsPanel details:has(#tpSelect) summary';
  await reveal(page, summary);
  await page.locator(summary).click();
  await expect(page.locator('#tpSelect')).toBeVisible();
}

/** The next steady, lit frame whose geometry is not `prev` (the engine
 * takes a tick or two to show a newly chosen pattern). */
async function settled(prev: string) {
  let out = '';
  await expect.poll(async () => {
    const [a, b] = [await frame(), await frame()];
    out = shape(b.points);
    return a.points.some(isLit) && shape(a.points) === out && out !== prev;
  }).toBe(true);
  return out;
}

let patterns: { id: string; label: string }[] = [];
let cueId = '';

test.beforeAll(async () => {
  patterns = (await studio.get<{ patterns: { id: string; label: string }[] }>('/api/testpattern')).patterns;
  const catalog = await studio.presets();
  cueId = catalog.presets[0].id;
  const show = {
    name: 'Mire e2e', time_base: 'seconds',
    tempo_map: [{ at_s: 0, bpm: 120, beats_per_bar: 4 }],
    tracks: [{ name: 'Piste 1', layer: 2, events: [{ id: 1, start: 0, len: 600, source: { kind: 'cue', id: catalog.presets[1].id } }] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/testpattern', { pattern: null, brightness: 0.2 });
  await studio.post('/api/timeline/stop');
  await studio.reset();
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/testpattern', { pattern: null, brightness: 0.2 });
  await studio.post('/api/timeline/stop');
});

test('every pattern changes the frame, stays under 20 % and never arms', async ({ page }) => {
  expect(patterns.map(p => p.label)).toEqual([
    'Croix et cadre', 'Grille', 'Cercle dans un carré', 'Barres de couleur',
    'Rampe de blanc', 'Vitesse de balayage', 'Extinction et retard couleur',
  ]);
  await openPatterns(page);
  await expect(page.locator('#tpBrightV')).toHaveText('20 %');
  await expect(page.locator('#tpExit')).toBeDisabled();
  const seen = new Set<string>();
  let prev = shape((await frame()).points);
  for (const p of patterns) {
    await page.locator('#tpSelect').selectOption(p.id);
    await expect.poll(async () => (await frame()).test_pattern.active).toBe(p.id);
    // A new, static geometry (the same lit points on two frames).
    const s = await settled(prev);
    const f = await frame();
    expect(shape(f.points)).toBe(s);
    expect(seen.has(s), `${p.id} differs from the other patterns`).toBe(false);
    seen.add(s);
    prev = s;
    expect(peak(f.points)).toBeLessThanOrEqual(0.2 + 1e-3);
    expect(f.points.every(q => Math.abs(q[0]) <= 1 && Math.abs(q[1]) <= 1)).toBe(true);
    expect(f.armed).toBe(false);
    await expect(page.locator('#tpBanner')).toBeVisible();
  }
});

test('live modifiers do not touch the pattern; the slider sets its brightness', async ({ page }) => {
  const look = shape((await frame()).points);
  expect(await studio.post('/api/testpattern', { pattern: 'cross_frame' })).toBe(200);
  const ref = await settled(look);
  expect(ref).toContain('-1,-1');
  await studio.post('/api/control', { id: 'master.size', value: 0.3 });
  await studio.post('/api/control', { id: 'master.brightness', value: 0 });
  await page.waitForTimeout(200);
  expect(shape((await frame()).points)).toBe(ref);
  await studio.post('/api/control', { id: 'master.reset' });

  await openPatterns(page);
  const low = peak((await frame()).points);
  await page.locator('#tpBright').fill('60');
  await expect.poll(async () => (await frame()).test_pattern.brightness).toBeCloseTo(0.6, 2);
  await expect.poll(async () => peak((await frame()).points)).toBeGreaterThan(low * 2);
});

test('leaving the pattern gives back the cue and the timeline position', async ({ page }) => {
  expect(await studio.post('/api/cue', { id: cueId, mode: 'toggle' })).toBe(200);
  expect(await studio.post('/api/timeline/load', { name: 'Mire e2e' })).toBe(200);
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await expect.poll(async () => (await frame()).timeline.playing).toBe(true);
  const look = shape((await frame()).points);

  await openPatterns(page);
  await page.locator('#tpSelect').selectOption('grid');
  await expect.poll(async () => (await frame()).timeline.paused).toBe(true);
  const pos = (await frame()).timeline.position;
  await page.waitForTimeout(400);
  let f = await frame();
  expect(f.timeline.position).toBeCloseTo(pos, 3);
  expect(f.cues.active.map(a => a.cue)).toContain(cueId);
  expect(shape(f.points)).not.toBe(look);

  // The banner is shown in every workspace, with its own exit.
  await openWorkspace(page, 'live');
  await expect(page.locator('#tpBanner')).toBeVisible();
  await expect(page.locator('#tpBanner')).toContainText('Mire affichée — cues et timeline en pause');
  await page.locator('#tpBannerExit').click();
  await expect.poll(async () => (await frame()).test_pattern.active).toBe(null);
  await expect(page.locator('#tpBanner')).toBeHidden();
  f = await frame();
  expect(f.timeline.playing).toBe(true);
  expect(f.timeline.position).toBeGreaterThanOrEqual(pos);
  expect(f.timeline.position).toBeLessThan(pos + 1);
  expect(f.cues.active.map(a => a.cue)).toContain(cueId);
  expect(f.armed).toBe(false);
});

test('Escape under a pattern: dark output, still disarmed', async ({ page }) => {
  expect(await studio.post('/api/testpattern', { pattern: 'grid' })).toBe(200);
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await studio.state()).estop).toBe(true);
  const f = await frame();
  expect(f.armed).toBe(false);
  expect(f.output_lit).toBe(0);
  expect(f.test_pattern.active).toBe('grid');
  expect(await studio.post('/api/estop/reset')).toBe(200);
});
