// Engine performance monitor (T-448): RÉGLAGES › Performance shows the
// stage timings over 10 s, and a simulated stall (--test-hooks, POST
// /api/test/stall) shows as a late frame, in red, on the panel and on the
// points indicator. Preview only: nothing is armed.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio({ testHooks: true });

interface Perf {
  render: { mean_ms: number; worst_ms: number };
  safety: { mean_ms: number; worst_ms: number };
  output: { mean_ms: number; worst_ms: number };
  frame: { mean_ms: number; worst_ms: number };
  points: number;
  late_frames: number;
  late_in_window: number;
  clients: number;
  window_s: number;
  warn_worst_ms: number;
}
const perf = () => studio.get<Perf>('/api/perf');
const ms = (text: string | null) => parseFloat((text ?? '').replace(',', '.'));

test.beforeEach(async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#perfPanel > summary').click();
});

test('the panel shows the stage timings, the points and the pages connected', async ({ page }) => {
  const j = await perf();
  expect(j.window_s).toBeGreaterThan(0);
  expect(j.frame.mean_ms).toBeGreaterThan(0);
  expect(j.warn_worst_ms).toBe(8);
  for (const id of ['Render', 'Safety', 'Output', 'Frame']) {
    await expect(page.locator(`#perf${id}Mean`)).toHaveText(/^\d+,\d\d ms$/);
    await expect(page.locator(`#perf${id}Worst`)).toHaveText(/^\d+,\d\d ms$/);
  }
  await expect.poll(async () => ms(await page.locator('#perfFrameMean').textContent())).toBeGreaterThan(0);
  await expect.poll(async () => ms(await page.locator('#perfRenderWorst').textContent())).toBeGreaterThan(0);
  await expect(page.locator('#perfPoints')).toHaveText(/^\d+$/);
  // This page beats: at least one page connected.
  await expect.poll(async () => +((await page.locator('#perfClients').textContent()) ?? 0)).toBeGreaterThanOrEqual(1);
  await expect(page.locator('#perfState')).toHaveText(/^(Normal|Chargé|Images en retard)$/);
  expect((await studio.state()).armed).toBe(false);
});

test('a 200 ms stall shows as a late frame, in red', async ({ page }) => {
  const before = (await perf()).late_frames;
  expect(await studio.post('/api/test/stall?ms=200')).toBe(200);
  await expect.poll(async () => (await perf()).late_frames).toBeGreaterThan(before);
  const j = await perf();
  expect(j.late_in_window).toBeGreaterThanOrEqual(1);
  expect(j.frame.worst_ms).toBeGreaterThanOrEqual(200);
  expect(j.render.worst_ms).toBeGreaterThanOrEqual(200);
  await expect(page.locator('#perfState')).toHaveText('Images en retard');
  await expect(page.locator('#perfState')).toHaveAttribute('data-level', '2');
  await expect.poll(async () => ms(await page.locator('#perfFrameWorst').textContent())).toBeGreaterThanOrEqual(200);
  await expect(page.locator('#perfLate')).toContainText('depuis le démarrage');
  // The points indicator under the preview turns red, with its hint.
  await openWorkspace(page, 'live');
  await expect(page.locator('#stPoints')).toHaveClass(/perf-late/);
  await expect(page.locator('#stPoints')).toHaveAttribute('title', /Réglages › Performance/);
  // Informative only: nothing armed, no disarm recorded for it.
  expect((await studio.state()).armed).toBe(false);
});
