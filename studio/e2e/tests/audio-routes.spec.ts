// Audio routes (T-153): an analysis value or event, shaped (T-238), drives
// an allowed control. Preview only: --no-audio, the browser source
// (POST /api/audio, what the page sends from its microphone) feeds the
// engine; nothing can arm.
import { test, expect, useStudio, openUi, reveal, extent } from '../studio';

const studio = useStudio();

test.describe.configure({ mode: 'serial' });

type Routing = { mix: number; routes: { source: string; target: string; enabled: boolean; shape: Record<string, unknown> }[] };
const routing = () => studio.get<Routing & { sources: { id: string; event: boolean }[]; targets: { id: string }[]; max: number }>('/api/audio/routes');

/** Posts browser features (they go stale after 500 ms), then reads the frame's extent. */
async function extentWith(bass: number) {
  await studio.post('/api/audio', { level: bass, bass, beat: 0 });
  return extent((await studio.frame()).points);
}

test.beforeAll(async () => {
  await studio.reset();
  // The look's own audio reaction stays off: only the route moves the size.
  await studio.post('/api/control', { id: 'audio.enabled', value: false });
});

test('the API lists sources and allowed targets, and refuses unsafe ones', async () => {
  const r = await routing();
  expect(r).toEqual(expect.objectContaining({ mix: 0.5, routes: [], max: 16 }));
  const sources = r.sources.map(s => s.id);
  for (const id of ['bass', 'mid', 'high', 'level', 'buildup', 'kick', 'snare', 'hat', 'drop', 'beat']) expect(sources).toContain(id);
  expect(r.sources.find(s => s.id === 'kick')!.event).toBe(true);
  const targets = r.targets.map(t => t.id);
  expect(targets).toContain('master.size');
  for (const id of ['transport.arm', 'transport.blackout', 'tempo.bpm', 'cue.max_active']) expect(targets).not.toContain(id);

  for (const target of ['transport.arm', 'transport.blackout', 'tempo.bpm', 'cue.max_active', 'calibration.x_scale']) {
    expect(await studio.post('/api/audio/routes', { mix: 0.5, routes: [{ source: 'kick', target }] }), target).toBe(400);
  }
  expect(await studio.post('/api/audio/routes', { routes: [{ source: 'volume', target: 'master.size' }] })).toBe(400);
  expect(await studio.post('/api/audio/routes', { routes: Array(17).fill({ source: 'bass', target: 'master.size' }) })).toBe(400);
  expect((await routing()).routes).toEqual([]);
  expect((await studio.frame()).armed).toBe(false);
});

test('a route bass → master.size added in the UI follows the browser audio', async ({ page }) => {
  await openUi(page, studio);
  await reveal(page, '#arAdd'); // LIVE › Modulateurs
  await expect(page.locator('#arHint')).toBeVisible();
  // The target menu only offers allowed controls.
  await page.locator('#arAdd').click();
  await expect(page.locator('#arList .aroute')).toHaveCount(1);
  await expect(page.locator('#arList [data-f="target"] option[value="transport.arm"]')).toHaveCount(0);
  await expect(page.locator('#arList [data-f="target"]')).toHaveValue('master.size');
  await expect(page.locator('#arList [data-f="source"]')).toHaveValue('bass');
  await page.locator('#arList [data-f="amount"]').fill('25');
  await expect(page.locator('#arList .aroute label', { hasText: 'Quantité' })).toContainText('25 %');
  await expect.poll(async () => (await routing()).routes[0]?.shape.max).toBeCloseTo(0.25, 3);

  // Silence: the size is the operator's (1 × the 0.5 circle); loud bass: 1 + 0.25 × range 2.
  await expect.poll(() => extentWith(0), { timeout: 3_000 }).toBeCloseTo(0.5, 1);
  await expect.poll(() => extentWith(1), { timeout: 3_000 }).toBeCloseTo(0.75, 1);
  // The live meter shows the shaped output.
  await studio.post('/api/audio', { level: 1, bass: 1, beat: 0 });
  await expect.poll(async () => +(await page.locator('#arList [data-out="0"]').getAttribute('data-v') ?? 0), { timeout: 3_000 }).toBeGreaterThan(0.1);
  // The stored value is untouched: only the engine's copy moves.
  expect((await studio.live()).size).toBe(1);
  expect((await studio.frame()).armed).toBe(false);

  // Off: the frame goes back to the operator's size.
  await page.locator('#arList [data-f="enabled"]').uncheck();
  await expect.poll(() => extentWith(1), { timeout: 3_000 }).toBeCloseTo(0.5, 1);
  await page.locator('#arList [data-f="enabled"]').check();
  // Temps ↔ Audio at 0: routes silenced.
  await page.locator('#arMix').fill('0');
  await expect.poll(async () => (await routing()).mix).toBe(0);
  await expect.poll(() => extentWith(1), { timeout: 3_000 }).toBeCloseTo(0.5, 1);
  await page.locator('#arMix').fill('50');
  await expect.poll(() => extentWith(1), { timeout: 3_000 }).toBeCloseTo(0.75, 1);
});

test('routes survive a restart', async ({ page }) => {
  const before = await routing();
  expect(before.routes).toHaveLength(1);
  await studio.restart();
  const after = await routing();
  expect({ mix: after.mix, routes: after.routes }).toEqual({ mix: before.mix, routes: before.routes });
  await openUi(page, studio);
  await reveal(page, '#arAdd');
  await expect(page.locator('#arList .aroute')).toHaveCount(1);
  await expect(page.locator('#arList [data-f="amount"]')).toHaveValue('25');
  await expect.poll(() => extentWith(1), { timeout: 3_000 }).toBeCloseTo(0.75, 1);
});

test('routes are saved in a project and come back when it is opened', async ({ page }) => {
  const saved = await routing();
  expect(await studio.post('/api/project/save-as', { path: 'Liens audio' })).toBe(200);
  expect(await studio.post('/api/audio/routes', { mix: 0.5, routes: [{ source: 'kick', target: 'master.brightness' }] })).toBe(200);
  expect((await routing()).routes[0].source).toBe('kick');
  expect(await studio.post('/api/project/open', { path: 'Liens audio' })).toBe(200);
  const back = await routing();
  expect({ mix: back.mix, routes: back.routes }).toEqual({ mix: saved.mix, routes: saved.routes });
  // A new project has none.
  expect(await studio.post('/api/project/new')).toBe(200);
  expect((await routing()).routes).toEqual([]);
  await openUi(page, studio);
  await reveal(page, '#arAdd');
  await expect(page.locator('#arList .aroute')).toHaveCount(0);
  await expect(page.locator('#arHint')).toBeVisible();
});
