// Zone chasers (T-170): a chaser made in RÉGLAGES › Zones de projection
// over two groups (left and right test zones), started from LIVE ›
// Calques › Chenillards de zones: the look moves from one half to the
// other on the beat, never both at once; stopped, both halves again.
// Editing is locked in show mode, playing is not; a cue can be tied to a
// chaser in « Propriétés du cue ». Preview only: the studio stays disarmed.
import { test, expect, useStudio, openUi, reveal, isLit, focusPage, openWorkspace, type Point } from '../studio';

const studio = useStudio();

interface Chaser { n: number; name: string; enabled: boolean; control: string; step: number | null; group: number | null }
interface ChaseView { chasers: Chaser[]; rate: number }
interface Proj { config: { chasers: { name: string; steps: number[]; step_beats: number; mode: string; enabled: boolean }[] } }
const chase = () => studio.get<ChaseView>('/api/zone-chase');
const proj = () => studio.get<Proj>('/api/projection');
type Grid = { pages: { cells: (string | null)[] }[] };

/** Which halves are lit in one frame. */
async function sides() {
  const pts = ((await studio.frame()).points as Point[]).filter(isLit);
  return { left: pts.some(p => p[0] < -0.02), right: pts.some(p => p[0] > 0.02) };
}

const ZONES = {
  zones: [
    { id: 1, name: 'Gauche', geometry: { size_x: 0.5, pos_x: -0.5 } },
    { id: 2, name: 'Droite', geometry: { size_x: 0.5, pos_x: 0.5 } },
  ],
  default_zones: [1, 2],
  groups: [{ name: 'Gauche', zones: [1] }, { name: 'Droite', zones: [2] }],
  chasers: [],
};

test.beforeEach(async ({ page }) => {
  await studio.reset();
  expect(await studio.post('/api/projection', ZONES)).toBe(200);
  expect(await studio.post('/api/control', { id: 'chaser.rate', value: 2 })).toBe(200);
  await openUi(page, studio);
});

test('a chaser made in the settings, started in LIVE, moves the look from one half to the other', async ({ page }) => {
  await expect(page.locator('#zoneChase')).toBeHidden();
  await expect.poll(async () => { const s = await sides(); return s.left && s.right; }).toBe(true);

  // RÉGLAGES › Zones de projection › Chenillards: one over both groups.
  await reveal(page, '#pzPanel summary');
  await page.locator('#pzPanel summary').click();
  await reveal(page, '#pzChaserAdd');
  await page.locator('#pzChaserAdd').click();
  await expect(page.locator('#pzChasers .pz-chaser')).toHaveCount(1);
  await expect.poll(async () => (await proj()).config.chasers.map(c => [c.steps, c.step_beats, c.mode, c.enabled])).toEqual([[[0, 1], 1, 'forward', false]]);
  await page.locator('#pzChName0').fill('Balayage');
  await page.locator('#pzChName0').press('Enter');
  await expect.poll(async () => (await chase()).chasers[0]?.name).toBe('Balayage');
  await expect(page.locator('#pzChasers .zc-stepline select')).toHaveCount(2);
  await page.locator('#pzChMode0').selectOption('bounce');
  await expect.poll(async () => (await proj()).config.chasers[0].mode).toBe('bounce');
  // Stopped: the routing is as before.
  expect(await sides()).toEqual({ left: true, right: true });

  // LIVE: the chaser's toggle, started.
  await openWorkspace(page, 'live');
  await reveal(page, '#zoneChase');
  const btn = page.locator('#zcRow .zc-btn[data-chaser="1"]');
  await expect(btn).toHaveAttribute('aria-pressed', 'false');
  await expect(btn.locator('.zc-name')).toHaveText('Balayage');
  await btn.click();
  await expect.poll(async () => (await chase()).chasers[0].enabled).toBe(true);
  await expect(btn).toHaveAttribute('aria-pressed', 'true');
  await expect(btn.locator('.zc-on')).toHaveText(/Gauche|Droite/);

  // Never both halves in one frame; both come up as the beats pass.
  const seen = { left: false, right: false };
  await expect.poll(async () => {
    const s = await sides();
    expect(s.left && s.right).toBe(false);
    seen.left ||= s.left;
    seen.right ||= s.right;
    return seen.left && seen.right;
  }, { timeout: 8000, intervals: [100] }).toBe(true);

  // Stopped again: both halves.
  await btn.click();
  await expect.poll(async () => (await chase()).chasers[0].enabled).toBe(false);
  await expect.poll(async () => { const s = await sides(); return s.left && s.right; }).toBe(true);
  expect((await studio.state()).armed).toBe(false);
});

test('show mode locks editing the chasers, not playing them; a cue ties to a chaser', async ({ page }) => {
  const one = { ...ZONES, chasers: [{ name: 'G-D', steps: [0, 1], step_beats: 1, mode: 'forward' }] };
  expect(await studio.post('/api/projection', one)).toBe(200);

  // « Propriétés du cue »: the cue follows chaser 1.
  const g = await studio.get<Grid>('/api/grid');
  const cue = g.pages[0].cells.find((c): c is string => !!c)!;
  await openUi(page, studio);
  await reveal(page, `#cues .cue[data-id="${cue}"]`);
  await page.locator(`#cues .cue[data-id="${cue}"]`).click({ button: 'right' });
  await expect(page.locator('#cueMenu')).toBeVisible();
  await page.locator('#cueMenuChaser').selectOption('0');
  await expect.poll(async () => (await studio.get('/api/cues')).slots[cue]?.zone_chaser).toBe(0);
  await page.locator('#cueMenuClose').click();

  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  try {
    const faster = { ...one, chasers: [{ ...one.chasers[0], step_beats: 0.5 }] };
    expect(await studio.post('/api/projection', faster)).toBe(423);
    await openWorkspace(page, 'live');
    await reveal(page, '#zoneChase');
    await page.locator('#zcRow .zc-btn[data-chaser="1"]').click();
    await expect.poll(async () => (await chase()).chasers[0].enabled).toBe(true);
    await page.locator('#zcRate').selectOption('3');
    await expect.poll(async () => (await chase()).rate).toBe(3);
  } finally {
    await studio.post('/api/lock', { show_mode: false });
    await studio.post('/api/control', { id: 'chaser.1.enabled', value: 0 });
  }
  expect((await proj()).config.chasers[0].step_beats).toBe(1);
  expect((await studio.state()).armed).toBe(false);
});

test('the global search finds « Chenillards de zones »', async ({ page }) => {
  await openWorkspace(page, 'live');
  await focusPage(page);
  await page.keyboard.press('Control+k');
  await expect(page.locator('#searchBox')).toHaveAttribute('data-ready', /\d+/);
  await page.locator('#searchInput').fill('chenillards de zones');
  const row = page.locator('#searchResults .sr[data-kind="setting"]').filter({ has: page.locator('.l', { hasText: /^Chenillards de zones/ }) }).first();
  await row.click();
  await expect(page.locator('#pzPanel')).toHaveAttribute('open', '');
  await expect(page.locator('#pzChaserAdd')).toBeVisible();
});
