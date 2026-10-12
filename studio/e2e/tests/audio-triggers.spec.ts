// Audio presets (T-239) and audio event triggers (T-240), LIVE › Musique.
// Preview only: --no-audio, the browser source (POST /api/audio) feeds the
// engine with simulated events; nothing can arm.
import { test, expect, useStudio, openUi, reveal, extent } from '../studio';

const studio = useStudio();

test.describe.configure({ mode: 'serial' });

type Routes = { mix: number; preset: string | null; routes: { source: string; target: string }[] };
const routes = () => studio.get<Routes>('/api/audio/routes');
type Triggers = { enabled: boolean; triggers: unknown[]; status: { enabled: boolean; live: boolean; rules: { fired: number }[] } };
const triggers = () => studio.get<Triggers>('/api/audio/triggers');

test.beforeAll(async () => {
  await studio.reset();
  // The look's own audio reaction stays off: only the routes move it.
  await studio.post('/api/control', { id: 'audio.enabled', value: false });
});

test('the « Punch » preset, chosen in the menu, makes a kick push the size out', async ({ page }) => {
  await openUi(page, studio);
  await reveal(page, '#musPreset'); // LIVE › Musique
  const names = await page.locator('#musPreset option').allTextContents();
  expect(names).toEqual(['Personnalisé', 'Punch', 'Groove', 'Scintillement', 'Festival', 'Calme']);
  const liveBefore = await studio.live();
  await page.locator('#musPreset').selectOption('punch');
  await expect.poll(async () => (await routes()).preset).toBe('punch');
  const r = await routes();
  expect(r.mix).toBe(0.5);
  expect(r.routes.map(x => `${x.source}→${x.target}`)).toEqual(['kick→master.brightness', 'kick→master.size']);
  await expect(page.locator('#musPresetHint')).toContainText('Punch');
  // Nothing else of the look changed.
  expect(await studio.live()).toEqual(liveBefore);

  // Between kicks the size is the operator's (the 0.5 circle); a kick pushes it out.
  let kick = 0;
  const after = async (newKick: boolean) => {
    if (newKick) kick++;
    await studio.post('/api/audio', { level: 0.5, bass: 0.2, beat: kick, kick, kick_strength: 1 });
    // The kick's push lasts half a beat: the widest of a few frames read
    // right after it (a loaded machine may answer late).
    let widest = 0;
    for (let i = 0; i < 6; i++) {
      widest = Math.max(widest, extent((await studio.frame()).points));
      if (!newKick) break;
      await new Promise(r => setTimeout(r, 15));
    }
    return widest;
  };
  await expect.poll(() => after(false), { timeout: 3_000 }).toBeCloseTo(0.5, 1);
  await expect.poll(() => after(true), { timeout: 3_000 }).toBeGreaterThan(0.54);
  expect((await studio.frame()).armed).toBe(false);

  // Another preset replaces the routes; « Calme » is bass only.
  await page.locator('#musPreset').selectOption('calm');
  await expect.poll(async () => (await routes()).routes.map(x => x.source)).toEqual(['bass']);
  expect((await routes()).mix).toBe(0.5);
  // An edit by hand: « Personnalisé ».
  await studio.post('/api/audio/routes', { mix: 0.5, routes: [] });
  expect((await routes()).preset).toBeNull();
});

test('a drop → cue rule, switched on in the panel, launches the cue and never arms', async ({ page }) => {
  const { presets } = await studio.presets();
  const cue = presets[2].id;
  // Rules are checked: an unknown cue is refused.
  expect(await studio.post('/api/audio/triggers', { triggers: [{ event: 'drop', action: { launch_cue: 'nope' } }] })).toBe(400);
  expect(await studio.post('/api/audio/triggers', { triggers: [{ event: 'drop', action: { launch_cue: cue } }] })).toBe(200);
  expect((await triggers()).enabled).toBe(false); // off by default

  await openUi(page, studio);
  await reveal(page, '#trgOn');
  await expect(page.locator('#trgList .trg')).toHaveCount(1);
  await expect(page.locator('#trgList [data-trg="event"]')).toHaveValue('drop');
  await expect(page.locator('#trgList [data-trg="cue"]')).toHaveValue(cue);
  await expect(page.locator('#trgOn')).not.toBeChecked();

  // Off: a drop does nothing.
  await studio.post('/api/audio', { level: 0.5, bass: 0.5, beat: 0, drop: 0 });
  await studio.post('/api/audio', { level: 0.5, bass: 0.5, beat: 0, drop: 1 });
  await page.waitForTimeout(200);
  expect((await studio.frame()).active_cue).not.toBe(cue);

  await page.locator('#trgOn').check();
  await expect.poll(async () => (await triggers()).enabled).toBe(true);
  await studio.post('/api/audio', { level: 0.5, bass: 0.5, beat: 0, drop: 1 });
  await page.waitForTimeout(100); // the rules take note of the counters first
  await studio.post('/api/audio', { level: 0.5, bass: 0.5, beat: 0, drop: 2 });
  await expect.poll(async () => (await studio.frame()).active_cue, { timeout: 3_000 }).toBe(cue);
  await expect.poll(async () => (await triggers()).status.rules[0]?.fired).toBe(1);
  await expect(page.locator('#trgList [data-count="0"]')).toHaveText('1');
  expect((await studio.frame()).armed).toBe(false);

  // Show mode: the rules are locked, the switch still works.
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  expect(await studio.post('/api/audio/triggers', { triggers: [] })).toBe(423);
  expect(await studio.post('/api/control', { id: 'audio.triggers.enabled', value: 0 })).toBe(200);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);

  // An e-stop switches it off; resetting doesn't switch it back on.
  expect(await studio.post('/api/control', { id: 'audio.triggers.enabled', value: 1 })).toBe(200);
  expect(await studio.post('/api/estop')).toBe(200);
  await expect.poll(async () => (await triggers()).enabled).toBe(false);
  await expect(page.locator('#trgOn')).not.toBeChecked();
  expect(await studio.post('/api/control', { id: 'audio.triggers.enabled', value: 1 })).toBe(403);
  expect(await studio.post('/api/estop/reset')).toBe(200);
  expect((await triggers()).enabled).toBe(false);
  expect((await studio.frame()).armed).toBe(false);
});

test('a rule added in the panel is saved as it is edited', async ({ page }) => {
  await openUi(page, studio);
  await reveal(page, '#trgAdd');
  await page.locator('#trgAdd').click();
  await expect(page.locator('#trgList .trg')).toHaveCount(2);
  await expect.poll(async () => (await triggers()).triggers).toHaveLength(2);
  const rule = page.locator('#trgList .trg').nth(1);
  await rule.locator('[data-trg="event"]').selectOption('every_bars');
  await expect.poll(async () => (await triggers()).triggers[1]).toEqual(expect.objectContaining({ event: { every_bars: 4 }, action: 'next_cue' }));
  await rule.locator('[data-trg="action"]').selectOption('black_hold');
  await expect.poll(async () => (await triggers()).triggers[1]).toEqual(expect.objectContaining({ action: { black_hold: { beats: 0.5 } } }));
  await rule.locator('[data-del="1"]').click();
  await expect.poll(async () => (await triggers()).triggers).toHaveLength(1);
});

test('rules survive a restart, the switch does not', async () => {
  expect(await studio.post('/api/control', { id: 'audio.triggers.enabled', value: 1 })).toBe(200);
  await studio.restart();
  const t = await triggers();
  expect(t.triggers).toHaveLength(1);
  expect(t.enabled).toBe(false);
});
