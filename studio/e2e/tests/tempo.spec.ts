// Tempo bar: BPM field, Tap (Enter), Sync 1 (Backspace), ×2 / ÷2.
// Keys are real key presses in the visible page (a hidden tab throttles
// timers and would distort tap intervals).
import { test, expect, useStudio, openUi, focusPage, type Tempo } from '../studio';

const studio = useStudio();
const tempo = async () => (await studio.state()).tempo as Tempo;

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'tempo.bpm', value: 120 });
  await openUi(page, studio);
  expect(await page.evaluate(() => document.visibilityState)).toBe('visible');
});

test('the tempo bar shows the clock\'s BPM and pulses its beat dots', async ({ page }) => {
  await expect(page.locator('#bpm')).toHaveValue('120.0');
  await expect(page.locator('#beats span')).toHaveCount(4);
  // At 120 BPM a dot lights up for 250 ms of every 500 ms.
  await expect(page.locator('#beats span.on')).toHaveCount(1);
});

test('Enter taps the tempo', async ({ page }) => {
  await focusPage(page);
  // Five taps 400 ms apart = 150 BPM. The spacing is the point of the test,
  // so these waits are intentional.
  for (let i = 0; i < 5; i++) {
    if (i) await page.waitForTimeout(400);
    await page.keyboard.press('Enter');
  }
  await expect.poll(async () => (await tempo()).source).toBe('tap');
  const t = await tempo();
  expect(t.bpm).toBeGreaterThan(140);
  expect(t.bpm).toBeLessThan(160);
  await expect.poll(async () => +(await page.locator('#bpm').inputValue())).toBeCloseTo(t.bpm, 0);
});

test('Backspace puts "now" on the one of the bar', async ({ page }) => {
  // Slow tempo (1.5 s per beat) so the check has plenty of margin.
  await page.locator('#bpm').fill('40');
  await page.locator('#bpm').press('Enter');
  await expect.poll(async () => (await tempo()).bpm).toBe(40);
  await focusPage(page);
  // Wait until we are mid-bar, far from the one.
  await expect.poll(async () => (await tempo()).beat_in_bar, { timeout: 10_000 }).toBe(2);
  await page.keyboard.press('Backspace');
  await expect.poll(async () => {
    const t = await tempo();
    return t.beat_in_bar === 0 && t.phase < 0.5;
  }).toBe(true);
  expect((await tempo()).bpm).toBe(40);
});

test('Enter and Backspace are ignored while typing in the BPM field', async ({ page }) => {
  await page.locator('#bpm').fill('128');
  await page.locator('#bpm').press('Enter'); // commits the field, no tap
  await page.locator('#bpm').press('Backspace'); // edits the field, no resync
  await expect.poll(async () => (await tempo()).bpm).toBe(128);
  expect((await tempo()).source).toBe('manual');
});

test('the Tap button taps and Sync 1 resyncs', async ({ page }) => {
  const tap = page.locator('button[data-ctl="tempo.tap"]');
  for (let i = 0; i < 4; i++) {
    if (i) await page.waitForTimeout(500);
    await tap.click();
  }
  await expect.poll(async () => (await tempo()).source).toBe('tap');
  const bpm = (await tempo()).bpm;
  expect(bpm).toBeGreaterThan(110);
  expect(bpm).toBeLessThan(130);

  await page.locator('button[data-ctl="tempo.resync"]').click();
  await expect.poll(async () => {
    const t = await tempo();
    return t.beat_in_bar === 0 && t.phase < 0.9; // 0.5 s per beat at ~120 BPM
  }).toBe(true);
});

test('×2 and ÷2 double and halve the tempo', async ({ page }) => {
  await page.locator('button[data-ctl="tempo.double"]').click();
  await expect.poll(async () => (await tempo()).bpm).toBe(240);
  await expect(page.locator('#bpm')).toHaveValue('240.0');
  await page.locator('button[data-ctl="tempo.half"]').click();
  await page.locator('button[data-ctl="tempo.half"]').click();
  await expect.poll(async () => (await tempo()).bpm).toBe(60);
});
