// Compensation de latence (T-246): the tempo bar's « Lat. » menu sets the
// *Décalage de sortie* (the effects read the beat ahead), shows the
// estimated latency and the analysis delay, and runs *Caler à l'oreille*.
// The offset never delays a stop: Escape still darkens the laser at once.
// Preview only (fake output).
import { test, expect, useStudio, openUi, focusPage, type Tempo } from '../studio';

const studio = useStudio();
type Latency = { config: { output_offset_ms: number; analysis_delay_ms: number }; min_ms: number; max_ms: number; output: { ms: number | null; detail: string } };
const latency = () => studio.get<Latency>('/api/latency');
const tempo = async () => (await studio.state()).tempo as Tempo & { output_offset_ms: number };

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  expect(await studio.post('/api/latency', { output_offset_ms: 0 })).toBe(200);
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/latency', { output_offset_ms: 0 });
});

test('the Lat. menu sets the output offset, clamped, and shows the estimates', async ({ page }) => {
  await page.locator('#latBtn').click();
  await expect(page.locator('#latMenu')).toBeVisible();
  await expect(page.locator('#latEst')).toContainText(/Latence estimée : \d+ ms/);
  await expect(page.locator('#latAnalysis')).toContainText("Retard d'analyse déjà compensé");
  // The slider, as a user drags it: +40 ms.
  await page.locator('#latOffset').evaluate((el: HTMLInputElement) => { el.value = '40'; el.dispatchEvent(new Event('input', { bubbles: true })); });
  await expect.poll(async () => (await latency()).config.output_offset_ms).toBe(40);
  await expect(page.locator('#latOffsetV')).toHaveText('+40 ms');
  // The clock's tempo does not move; the state reports the offset.
  const t = await tempo();
  expect(t.bpm).toBe(120);
  expect(t.output_offset_ms).toBe(40);
  await expect(page.locator('#latBtn')).toHaveClass(/active/);
  await expect(page.locator('#latBtn')).toHaveAttribute('title', /\+40 ms/);
  // The registry control (MIDI, OSC) is the same setting, clamped.
  expect(await studio.post('/api/control', { id: 'tempo.output_offset', value: -500 })).toBe(200);
  await expect.poll(async () => (await latency()).config.output_offset_ms).toBe(-100);
  await expect(page.locator('#latBtn')).toHaveAttribute('title', /-100 ms/);
  await expect(page.locator('#latOffsetV')).toHaveText('-100 ms');
  // A non-number is refused and changes nothing.
  expect(await studio.post('/api/latency', { output_offset_ms: 'beaucoup' })).toBe(400);
  expect((await latency()).config.output_offset_ms).toBe(-100);
  // A click outside closes the menu.
  await page.locator('h1').click();
  await expect(page.locator('#latMenu')).toBeHidden();
});

test('the offset is kept across a restart', async () => {
  expect(await studio.post('/api/latency', { output_offset_ms: 25 })).toBe(200);
  await studio.restart();
  expect((await latency()).config.output_offset_ms).toBe(25);
});

test('Caler à l\'oreille: starts and stops the metronome and the flash', async ({ page }) => {
  await page.locator('#latBtn').click();
  const ear = page.locator('#latEar');
  await ear.click();
  await expect(ear).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#latEarHint')).toBeVisible();
  // At 120 BPM the flash lights for 15 % of every beat.
  await page.waitForFunction(() => !!document.querySelector('#latFlash.on'), null, { polling: 'raf', timeout: 3_000 });
  await ear.click();
  await expect(ear).toHaveAttribute('aria-pressed', 'false');
  await expect(page.locator('#latFlash.on')).toHaveCount(0);
  // Closing the menu stops it too.
  await ear.click();
  await page.locator('#latBtn').click();
  await expect(page.locator('#latMenu')).toBeHidden();
  await expect(ear).toHaveAttribute('aria-pressed', 'false');
});

test('with the largest offset, Escape still darkens the laser at once', async ({ page }) => {
  expect(await studio.post('/api/latency', { output_offset_ms: 100 })).toBe(200);
  await page.locator('#latBtn').click();
  expect((await studio.state()).armed).toBe(false);
  await page.locator('#armBtn').click();
  await expect.poll(async () => (await studio.state()).armed).toBe(true);
  await expect.poll(async () => (await studio.frame() as unknown as { output_lit: number }).output_lit).toBeGreaterThan(0);
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await studio.state()).armed).toBe(false);
  expect((await studio.frame() as unknown as { output_lit: number }).output_lit).toBe(0);
  await expect(page.locator('#estopBanner')).toBeVisible();
});
