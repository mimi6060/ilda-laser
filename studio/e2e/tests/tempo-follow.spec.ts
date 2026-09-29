// Tempo auto (T-234): the one tempo clock follows the audio detection.
// Preview only, --no-audio: --test-hooks enables POST /api/test/tempo_estimate,
// which publishes a simulated detector estimate as if from the native
// analysis (fresh for 500 ms, so the tests keep posting while they need one).
import { test, expect, useStudio, openUi, type Tempo } from '../studio';

const studio = useStudio({ testHooks: true });
const tempo = async () => (await studio.state()).tempo as Tempo;

interface Estimate { bpm: number; confidence: number; state: string; offset_s?: number }

/** Posts `est` every 100 ms for `ms`. */
async function feed(est: Estimate, ms: number) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    expect(await studio.post('/api/test/tempo_estimate', est)).toBe(200);
    await new Promise(r => setTimeout(r, 100));
  }
}

const LOCKED_128: Estimate = { bpm: 128, confidence: 0.9, state: 'locked', offset_s: 0.1 };

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'tempo.bpm', value: 120 }); // manual: Tempo auto off
  // The estimates are read from the native input.
  expect(await studio.post('/api/audio/config', { source: 'native' })).toBe(200);
  await openUi(page, studio);
});

test('Auto follows a locked estimate, then keeps the tempo through a break', async ({ page }) => {
  await page.locator('#tempoAuto').click();
  await expect.poll(async () => (await tempo()).source).toBe('audio');
  await expect(page.locator('#tempoAuto')).toHaveClass(/active/);
  await expect(page.locator('#tempoGuide')).toBeVisible();
  expect((await tempo()).follow).toBe('waiting');

  const feeding = feed(LOCKED_128, 4_000);
  await expect.poll(async () => (await tempo()).follow).toBe('locked');
  await expect.poll(async () => (await tempo()).bpm, { timeout: 6_000 }).toBeCloseTo(128, 0);
  await feeding;
  await expect(page.locator('#tempoSrc')).toContainText('verrouillé');
  await expect.poll(async () => Math.abs(+(await page.locator('#bpm').inputValue()) - 128)).toBeLessThan(0.3);

  // Nothing fresh any more: *Maintien*, same BPM.
  await expect.poll(async () => (await tempo()).follow).toBe('coasting');
  await expect(page.locator('#tempoSrc')).toContainText('maintien');
  expect((await tempo()).bpm).toBeCloseTo(128, 0);
  // Never arms anything.
  expect((await studio.state()).armed).toBe(false);
});

test('a tap takes the clock back until Auto is pressed again', async ({ page }) => {
  await studio.post('/api/control', { id: 'tempo.auto', value: 1 });
  await feed(LOCKED_128, 3_000);
  await expect.poll(async () => (await tempo()).bpm).toBeCloseTo(128, 0);

  await page.locator('button[data-ctl="tempo.tap"]').click();
  await expect.poll(async () => (await tempo()).source).toBe('tap');
  await expect(page.locator('#tempoAuto')).not.toHaveClass(/active/);
  await expect(page.locator('#tempoSrc')).toBeHidden();
  // A different tempo is detected: ignored while the tap holds the clock.
  await feed({ ...LOCKED_128, bpm: 140 }, 3_000);
  let t = await tempo();
  expect(t.source).toBe('tap');
  expect(t.bpm).toBeCloseTo(128, 0);

  await page.locator('#tempoAuto').click();
  const feeding = feed({ ...LOCKED_128, bpm: 140 }, 4_000);
  await expect.poll(async () => (await tempo()).bpm, { timeout: 6_000 }).toBeCloseTo(140, 0);
  await feeding;
  t = await tempo();
  expect(t.source).toBe('audio');

  // And off again: manual, the BPM stays.
  await page.locator('#tempoAuto').click();
  await expect.poll(async () => (await tempo()).source).toBe('manual');
  expect((await tempo()).bpm).toBeCloseTo(140, 0);
});

test('an unsure estimate never moves the clock', async () => {
  await studio.post('/api/control', { id: 'tempo.auto', value: 1 });
  await feed({ bpm: 140, confidence: 0.9, state: 'checking' }, 2_500);
  await feed({ bpm: 140, confidence: 0.4, state: 'locked' }, 2_500);
  const t = await tempo();
  expect(t.bpm).toBe(120);
  expect(t.follow).toBe('waiting');
  expect(t.confidence).toBeCloseTo(0.4, 2);
});

test('Nouveau morceau keeps Tempo auto and the clock', async () => {
  await studio.post('/api/control', { id: 'tempo.auto', value: 1 });
  expect(await studio.post('/api/control', { id: 'tempo.new_track' })).toBe(200);
  expect(await studio.post('/api/audio/tempo/new_track', {})).toBe(200);
  const t = await tempo();
  expect(t.source).toBe('audio');
  expect(t.guide_bpm).toBeNull();
  expect(t.bpm).toBe(120);
});
