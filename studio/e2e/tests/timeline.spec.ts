// Timeline (T-160): load a saved show, play / pause / stop / loop it from
// the Timeline section, follow the playhead, and check that the master
// live modifiers apply on top and that Escape stops the show - all
// without ever arming (preview only).
import { test, expect, useStudio, openUi, openWorkspace, focusPage, extent, isLit, type Point } from '../studio';

const studio = useStudio();

interface TimelineState {
  name: string | null; time_base: 'seconds' | 'beats'; position: number; length: number; beat: number; bar: number;
  playing: boolean; paused: boolean; waiting: boolean; loop: boolean; loop_region: [number, number] | null; active: number[];
}
const timeline = async () => (await studio.get<{ timeline: TimelineState }>('/api/frame')).timeline;
const points = async () => (await studio.get<{ points: Point[] }>('/api/frame')).points;
const lit = async () => (await points()).filter(isLit).length;

test.beforeAll(async () => {
  const catalog = await studio.presets();
  const cue = catalog.presets[0].id;
  // A 60 s show on the song's clock: one cue from 0 s, another from 30 s.
  const show = {
    name: 'Essai e2e',
    time_base: 'seconds',
    tempo_map: [{ at_s: 0, bpm: 120, beats_per_bar: 4 }],
    tracks: [{ name: 'Piste 1', layer: 1, events: [
      { id: 1, start: 0, len: 30, source: { kind: 'cue', id: cue } },
      { id: 2, start: 30, len: 30, source: { kind: 'cue', id: catalog.presets[1].id } },
    ] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/timeline/stop');
  await studio.post('/api/timeline/loop', { on: false });
  await studio.post('/api/estop/reset');
  await studio.reset();
  await openUi(page, studio);
});

test('load a show, play it, see the playhead move, then stop it', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await page.locator('#tlShow').selectOption('Essai e2e');
  await expect.poll(async () => (await timeline()).name).toBe('Essai e2e');
  expect((await timeline()).length).toBe(60);

  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await timeline()).playing).toBe(true);
  await expect(page.locator('#tlPlay')).toHaveClass(/active/);
  await expect.poll(async () => (await timeline()).active).toEqual([1]);
  await expect.poll(async () => (await timeline()).position).toBeGreaterThan(0.3);
  await expect(page.locator('#tlPos')).toContainText(/0:0\d\.\d \/ 1:00\.0 · mesure \d+\.\d/);
  expect(await lit()).toBeGreaterThan(0);
  const head = await page.locator('#tlHead').evaluate(e => parseFloat((e as HTMLElement).style.left));
  expect(head).toBeGreaterThan(0);
  const st = await studio.state();
  expect(st.armed).toBe(false);

  await page.locator('#tlStop').click();
  await expect.poll(async () => (await timeline()).playing).toBe(false);
  expect((await timeline()).position).toBe(0);
  // The show had taken over from the manual look: nothing left to draw.
  await expect.poll(lit).toBe(0);
  await expect(page.locator('#tlPlay')).not.toHaveClass(/active/);
});

test('pause freezes the playhead, seeking jumps, and the loop button toggles', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await page.locator('#tlShow').selectOption('Essai e2e');
  await expect.poll(async () => (await timeline()).name).toBe('Essai e2e');
  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await timeline()).playing).toBe(true);

  await page.locator('#tlPause').click();
  await expect.poll(async () => (await timeline()).paused).toBe(true);
  const at = (await timeline()).position;
  await page.waitForTimeout(300); // the point: time passes, the playhead doesn't
  expect((await timeline()).position).toBe(at);
  expect(await lit()).toBeGreaterThan(0); // the frozen frame stays

  // Click at three quarters of the bar: 45 s, in the second event.
  const box = (await page.locator('#tlBar').boundingBox())!;
  await page.locator('#tlBar').click({ position: { x: box.width * 0.75, y: box.height / 2 } });
  await expect.poll(async () => (await timeline()).position).toBeCloseTo(45, 0);
  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await timeline()).active).toEqual([2]);

  await page.locator('#tlLoop').click();
  await expect.poll(async () => (await timeline()).loop).toBe(true);
  await expect(page.locator('#tlLoop')).toHaveClass(/active/);
  expect((await studio.controlValues()).values['timeline.loop']).toBe(true);
});

test('the timeline controls work through the control ids', async () => {
  await studio.post('/api/timeline/load', { name: 'Essai e2e' });
  expect(await studio.post('/api/control', { id: 'timeline.play' })).toBe(200);
  await expect.poll(async () => (await timeline()).playing).toBe(true);
  expect(await studio.post('/api/control', { id: 'timeline.stop' })).toBe(200);
  await expect.poll(async () => (await timeline()).playing).toBe(false);
});

test('master modifiers apply on top of the show', async () => {
  await studio.post('/api/timeline/load', { name: 'Essai e2e' });
  await studio.post('/api/timeline/play');
  await expect.poll(lit).toBeGreaterThan(0);
  const full = extent(await points());
  await studio.post('/api/control', { id: 'master.size', value: 0.5 });
  await expect.poll(async () => extent(await points())).toBeLessThan(full * 0.75);
  await studio.post('/api/control', { id: 'master.brightness', value: 0 });
  await expect.poll(lit).toBe(0);
  expect((await timeline()).playing).toBe(true);
});

test('Escape stops the output and the timeline; it never arms', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await page.locator('#tlShow').selectOption('Essai e2e');
  await expect.poll(async () => (await timeline()).name).toBe('Essai e2e');
  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await timeline()).playing).toBe(true);

  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await timeline()).playing).toBe(false);
  await expect.poll(lit).toBe(0);
  const st = await studio.state();
  expect(st.estop).toBe(true);
  expect(st.armed).toBe(false);

  // Play is refused until the e-stop is reset, with a message.
  await page.locator('#tlPlay').click();
  await expect(page.locator('#tlMsg')).toContainText('arrêt d\'urgence');
  expect((await timeline()).playing).toBe(false);

  await page.locator('#estopReset').click();
  await expect.poll(async () => (await studio.state()).estop).toBe(false);
  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await timeline()).playing).toBe(true);
  expect((await studio.state()).armed).toBe(false);
});

test('a grid cell set to a show plays it, and a second press stops it', async ({ page }) => {
  const catalog = await studio.presets();
  const cell = catalog.presets.filter(p => p.category === catalog.categories[0])[2];
  await page.locator(`#cues .cue[data-id="${cell.id}"]`).click({ button: 'right' });
  await page.locator('#cueMenuShow').selectOption('Essai e2e');
  await expect.poll(async () => (await studio.get('/api/cues')).slots[cell.id]?.show).toBe('Essai e2e');
  await page.locator('#cueMenuClose').click();

  await page.locator(`#cues .cue[data-id="${cell.id}"]`).click();
  await expect.poll(async () => (await timeline()).playing).toBe(true);
  expect((await timeline()).name).toBe('Essai e2e');
  expect((await studio.get('/api/frame')).cues.active).toEqual([]);
  await page.locator(`#cues .cue[data-id="${cell.id}"]`).click();
  await expect.poll(async () => (await timeline()).playing).toBe(false);
  await studio.post('/api/cues/slot', { id: cell.id });
});
