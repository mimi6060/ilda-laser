// Temps 1 suggéré (T-235): the detected "1" is shown on its beat dot, and
// *Accepter* resyncs the bar on it. Preview only: --test-hooks enables
// POST /api/test/tempo_estimate, which publishes a simulated detection
// (beats on the grid offset_s + n·60/bpm, the "1" on grid beats ≡ downbeat_beat
// mod 4), fresh for 500 ms, so the tests keep posting while they need one.
import { test, expect, useStudio, openUi, type Tempo } from '../studio';

const studio = useStudio({ testHooks: true });
const tempo = async () => (await studio.state()).tempo as Tempo;

interface Estimate { bpm: number; confidence: number; state: string; offset_s: number; downbeat_beat?: number; downbeat_confidence?: number }

/** Posts `est()` every 100 ms until the returned stop function is called. */
function feeding(est: () => Estimate) {
  let on = true;
  const done = (async () => {
    while (on) {
      expect(await studio.post('/api/test/tempo_estimate', est())).toBe(200);
      await new Promise(r => setTimeout(r, 100));
    }
  })();
  return async () => { on = false; await done; };
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'tempo.bpm', value: 120 }); // manual: Tempo auto off
  expect(await studio.post('/api/audio/config', { source: 'native' })).toBe(200);
  await openUi(page, studio);
});

test('the suggested 1 is dotted on its beat, Accepter resyncs the bar on it', async ({ page }) => {
  await expect(page.locator('#downbeatHint')).toBeHidden();
  await expect(page.locator('#downbeatAccept')).toBeHidden();
  let est: Estimate = { bpm: 120, confidence: 0.9, state: 'locked', offset_s: 0, downbeat_beat: 1, downbeat_confidence: 0.8 };
  const stop = feeding(() => est);
  try {
    await expect.poll(async () => (await tempo()).downbeat).not.toBeNull();
    // Make sure the music's "1" is not already the clock's.
    if ((await tempo()).downbeat!.beat_in_bar_offset === 0) {
      est = { ...est, downbeat_beat: 2 };
      await expect.poll(async () => (await tempo()).downbeat!.beat_in_bar_offset).not.toBe(0);
    }
    const t = await tempo();
    expect(t.downbeat_suggested).toBe(true);
    const offset = t.downbeat!.beat_in_bar_offset;
    await expect(page.locator('#downbeatHint')).toBeVisible();
    await expect(page.locator('#downbeatHint')).toContainText('Temps 1 suggéré');
    await expect(page.locator('#downbeatHint')).toContainText('80 %');
    await expect(page.locator('#beats span.suggest')).toHaveCount(1);
    await expect(page.locator(`#beats span:nth-child(${offset + 1})`)).toHaveClass(/suggest/);

    // Manual source: the bar never moves by itself.
    await page.waitForTimeout(600);
    expect((await tempo()).downbeat!.beat_in_bar_offset).toBe(offset);

    await page.locator('#downbeatAccept').click();
    await expect.poll(async () => (await tempo()).downbeat!.beat_in_bar_offset).toBe(0);
    await expect(page.locator('#downbeatHint')).toBeHidden();
    await expect(page.locator('#downbeatAccept')).toBeHidden();
    await expect(page.locator('#beats span.suggest')).toHaveCount(0);
    expect((await tempo()).bpm).toBeCloseTo(120, 3);
  } finally {
    await stop();
  }
  // Never arms anything.
  expect((await studio.state()).armed).toBe(false);
});

test('an unsure detection shows nothing, and Accepter without one is refused', async ({ page }) => {
  const stop = feeding(() => ({ bpm: 120, confidence: 0.9, state: 'locked', offset_s: 0, downbeat_beat: 1, downbeat_confidence: 0.2 }));
  try {
    await expect.poll(async () => (await tempo()).downbeat?.confidence).toBeCloseTo(0.2, 2);
    expect((await tempo()).downbeat_suggested).toBe(false);
    await expect(page.locator('#downbeatHint')).toBeHidden();
    await expect(page.locator('#beats span.suggest')).toHaveCount(0);
  } finally {
    await stop();
  }
  // Stale: no detection at all, and nothing to accept.
  await expect.poll(async () => (await tempo()).downbeat, { timeout: 3_000 }).toBeNull();
  expect(await studio.post('/api/control', { id: 'tempo.downbeat_accept' })).not.toBe(200);
});
