// Pilote automatique (T-148): the « Pilote » menu next to the BPM changes
// cue every N bars, in order or at random, from the page shown or from
// the pages ticked; « Ignorer par le pilote » keeps a cell out. It only
// launches cues: it never arms, an e-stop switches it off for good, and
// its pages are an edit (undone, locked in show mode). Preview only.
import { test, expect, useStudio, openUi } from '../studio';

const studio = useStudio();
test.describe.configure({ mode: 'serial' });

type Autopilot = { enabled: boolean; every_bars: number; random: boolean; pages: number[]; allow_audience: boolean; last_pick: string | null; changes: number; candidates: string[]; waiting: string | null };
const autopilot = () => studio.get<Autopilot>('/api/autopilot');
type Grid = { pages: { cells: (string | null)[] }[] };

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/control', { id: 'vlj.enabled', value: 0 });
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/control', { id: 'page.1' });
  await studio.post('/api/autopilot', { pages: [], allow_audience: false });
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/control', { id: 'vlj.enabled', value: 0 });
  await studio.post('/api/control', { id: 'tempo.bpm', value: 120 });
  await studio.post('/api/lock', { show_mode: false });
});

test('the menu switches it on, every bar, and it changes cues without arming', async ({ page }) => {
  await page.locator('#vljBtn').click();
  await expect(page.locator('#vljMenu')).toBeVisible();
  await expect(page.locator('#vljStatus')).toHaveText('Coupé.');
  await page.locator('#vljEvery').selectOption('0');
  await expect.poll(async () => (await autopilot()).every_bars).toBe(1);
  await page.locator('#vljOrder').selectOption('1');
  await expect.poll(async () => (await autopilot()).random).toBe(true);
  // A fast tempo: a bar is one second.
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 240 })).toBe(200);
  await page.locator('#vljToggle').click();
  await expect(page.locator('#vljToggle')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#vljBtn')).toHaveClass(/active/);
  await expect.poll(async () => (await autopilot()).changes, { timeout: 10_000 }).toBeGreaterThanOrEqual(2);
  const a = await autopilot();
  expect(a.candidates).toContain(a.last_pick);
  expect((await studio.frame()).armed).toBe(false);
  expect(a.candidates).toContain((await studio.controlValues()).active_cue);
  await expect(page.locator('#vljStatus')).toContainText('en cours');
  // A click outside closes the menu; it keeps running.
  await page.locator('h1').click();
  await expect(page.locator('#vljMenu')).toBeHidden();
  expect((await autopilot()).enabled).toBe(true);
  // The e-stop switches it off; the reset doesn't switch it back on.
  expect(await studio.post('/api/estop', {})).toBe(200);
  await expect.poll(async () => (await autopilot()).enabled).toBe(false);
  await studio.post('/api/estop/reset', {});
  await page.waitForTimeout(500);
  expect((await autopilot()).enabled).toBe(false);
  await expect(page.locator('#vljBtn')).not.toHaveClass(/active/);
  // The same menu opens from LIVE › Cues (the top bar hides its button
  // under 1360 px).
  await page.setViewportSize({ width: 1280, height: 800 });
  await expect(page.locator('#vljBtn')).toBeHidden();
  await page.locator('#cueVljBtn').click();
  await expect(page.locator('#vljMenu')).toBeVisible();
  await expect(page.locator('#cueVljBtn')).toHaveAttribute('aria-expanded', 'true');
  const box = (await page.locator('#vljMenu').boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(1280);
});

test('pages and public content are an edit: undone, and locked in show mode', async ({ page }) => {
  await page.locator('#vljBtn').click();
  await page.locator('#vljPage2').check();
  await expect.poll(async () => (await autopilot()).pages).toEqual([2]);
  const g = await studio.get<Grid>('/api/grid');
  const page2 = g.pages[1].cells.filter((c): c is string => !!c);
  expect((await autopilot()).candidates.every(c => page2.includes(c))).toBe(true);
  expect(await studio.post('/api/undo', {})).toBe(200);
  await expect.poll(async () => (await autopilot()).pages).toEqual([]);
  await expect(page.locator('#vljPage2')).not.toBeChecked();

  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  await page.locator('#vljAudience').check();
  await expect(page.locator('#vljAudience')).not.toBeChecked();
  expect((await autopilot()).allow_audience).toBe(false);
  // Playing it is not an edit: the switch still works in show mode.
  await page.locator('#vljToggle').click();
  await expect.poll(async () => (await autopilot()).enabled).toBe(true);
  expect((await studio.frame()).armed).toBe(false);
});

test('« Ignorer par le pilote » in the cue properties keeps a cell out', async ({ page }) => {
  const g = await studio.get<Grid>('/api/grid');
  const first = g.pages[0].cells.find((c): c is string => !!c)!;
  expect((await autopilot()).candidates).toContain(first);
  await page.locator(`#cues .cue[data-id="${first}"]`).click({ button: 'right' });
  await expect(page.locator('#cueMenu')).toBeVisible();
  await page.locator('#cueMenuVljSkip').check();
  await expect.poll(async () => (await autopilot()).candidates).not.toContain(first);
  const slots = (await studio.get('/api/cues')).slots as Record<string, { vlj_skip?: boolean }>;
  expect(slots[first].vlj_skip).toBe(true);
  await page.locator('#cueMenuVljSkip').uncheck();
  await expect.poll(async () => (await autopilot()).candidates).toContain(first);
});
