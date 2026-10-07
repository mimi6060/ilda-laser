// The on-screen APC40 (T-210), in RÉGLAGES › Contrôleur. With --midi-test
// the simulated APC40 mkII is plugged: the drawing mirrors it (model,
// profile, LEDs). A click sends the button's own message through the
// mapping engine, recorded as the UI, and can never arm the laser.
import { test, expect, useStudio, openUi, reveal } from '../studio';
import { TEST_PORT } from '../midi';
import type { Page } from '@playwright/test';

const studio = useStudio({ midiTest: true });
let catalog: Awaited<ReturnType<typeof studio.presets>>;
const pageCues = (i: number) => catalog.presets.filter(p => p.category === catalog.categories[i]);
const el = (page: Page, id: string) => page.locator(`#vapc [data-vapc="${id}"]`);

async function openController(page: Page) {
  await reveal(page, '#midiPanel summary');
  if (!(await page.locator('#midiPanel').evaluate(d => (d as HTMLDetailsElement).open))) await page.locator('#midiPanel summary').click();
  await expect(el(page, 'pad.1.1')).toBeVisible();
}

test.beforeAll(async () => {
  catalog = await studio.presets();
  await expect.poll(async () => {
    const d = (await studio.get('/api/midi')).devices.find((x: { name: string }) => x.name === TEST_PORT);
    return d && [d.model, d.connected];
  }).toEqual(['Apc40Mk2', true]);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/midi/virtual', { id: 'shift', down: false });
  await studio.post('/api/midi/learn/cancel');
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
  await openController(page);
});

test('the drawing mirrors the connected APC40 mkII and fits 1280 px without scrolling', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await expect(page.locator('#vapcInfo')).toContainText(`APC40 mkII branché (${TEST_PORT})`);
  await expect(page.locator('#vapcModel')).toBeDisabled();
  await expect(page.locator('#vapc [data-vapc^="pad."]')).toHaveCount(40);
  await expect(page.locator('#vapc [data-vapc^="scene."]')).toHaveCount(5);
  await expect(page.locator('#vapc [data-vapc^="fader."]')).toHaveCount(9);
  const box = await page.locator('#vapc svg').boundingBox();
  expect(box!.width).toBeLessThanOrEqual(1280);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: test.info().outputPath('virtual-apc.png'), fullPage: false });
});

test('clicking pad (0,0) plays cue 1 of the page and lights it like the real pad', async ({ page }) => {
  const first = pageCues(0)[0];
  await el(page, 'pad.1.1').click();
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(first.id);
  // The simulated device got the same colour as the drawing shows.
  await expect.poll(async () => (await studio.get('/api/midi/leds')).leds['note:32:0'] ?? 0).toBeGreaterThan(0);
  await expect(el(page, 'pad.1.1')).toHaveClass(/lit/);
  await expect(el(page, 'pad.1.1').locator('.body')).toHaveClass(/pulse/);
  // The next pad holds a cue that isn't playing: dim, steady.
  await expect(el(page, 'pad.1.2')).toHaveClass(/lit/);
  await expect(el(page, 'pad.1.2').locator('.body')).not.toHaveClass(/pulse/);
  expect((await studio.state()).armed).toBe(false);
});

test('hovering a fader shows « Fader 1 — Taille », its Shift layer and its message', async ({ page }) => {
  await el(page, 'fader.1').hover();
  const tip = page.locator('#vapcTip');
  await expect(tip).toBeVisible();
  await expect(tip).toContainText('Fader 1 — Taille maître');
  await expect(tip).toContainText('Shift :');
  await expect(tip).toContainText('CC 0x07 · can. 1');
  await el(page, 'scene.1').hover();
  await expect(tip).toContainText('Scene Launch 1 — Page ');
  await expect(tip).toContainText('Note 0x52');
});

test('Shift verrouillé + Scene Launch 2 goes to page 7; dragging a fader moves its control', async ({ page }) => {
  await page.locator('#vapcShift').check();
  await expect.poll(async () => (await studio.get('/api/midi/leds')).shift).toBe(true);
  await expect(el(page, 'shift')).toHaveClass(/shift-on/);
  await el(page, 'scene.2').click();
  await expect.poll(async () => (await studio.controlValues()).cue_page).toBe(7);
  await el(page, 'shift').click();
  await expect.poll(async () => (await studio.get('/api/midi/leds')).shift).toBe(false);
  await expect(page.locator('#vapcShift')).not.toBeChecked();

  // Fader 1 (Taille maître, 0..2) starts at 1.0, mid travel: drag the cap to the top.
  await el(page, 'fader.1').scrollIntoViewIfNeeded();
  const cap = await el(page, 'fader.1').locator('.cap').boundingBox();
  const track = await el(page, 'fader.1').locator('.track').boundingBox();
  await page.mouse.move(cap!.x + cap!.width / 2, cap!.y + cap!.height / 2);
  await page.mouse.down();
  await page.mouse.move(cap!.x + cap!.width / 2, track!.y - 10, { steps: 8 });
  await page.mouse.up();
  await expect.poll(async () => (await studio.live()).size).toBe(2);
  await expect.poll(async () => (await studio.get('/api/midi/leds')).values['fader.1']).toBe(1);
});

test('the drawing never arms: Shift + Stop All is a blackout even with MIDI arming allowed', async ({ page }) => {
  await studio.post('/api/midi/safety', { allow_arm: true });
  try {
    await page.locator('#vapcShift').check();
    // Held past the 1 s arming gesture of the hardware (scrolled into view
    // first: the mouse presses at viewport coordinates).
    await el(page, 'stop_all').scrollIntoViewIfNeeded();
    const b = await el(page, 'stop_all').boundingBox();
    await page.mouse.move(b!.x + b!.width / 2, b!.y + b!.height / 2);
    await page.mouse.down();
    await page.waitForTimeout(1500);
    await page.mouse.up();
    expect((await studio.state()).armed).toBe(false);
    // Stop All is the blackout, as from the hardware, but credited to the UI.
    await expect.poll(async () => { const s = await studio.state(); return [s.armed, s.estop, s.arm.estop?.source]; }).toEqual([false, true, 'ui']);
  } finally {
    await studio.post('/api/midi/safety', { allow_arm: false });
  }
});

test('right click → Apprendre ici, then Retirer l\'affectation; the layout exports and re-imports the same', async ({ page }) => {
  await el(page, 'bank.1').click({ button: 'right' });
  await expect(page.locator('#vapcMenu')).toBeVisible();
  await expect(page.locator('#vapcMenuName')).toHaveText('Pan');
  await page.locator('#vapcMenuTarget').selectOption('cue.multi');
  await page.locator('#vapcMenuLearn').click();
  await expect(page.locator('#vapcMenu')).toBeHidden();
  await expect.poll(async () => {
    const l = await studio.get('/api/midi/virtual');
    return [l.profile, l.elements.find((e: { id: string }) => e.id === 'bank.1').map?.target];
  }).toEqual(['apc40-mk2-perso', 'cue.multi']);
  // The device's own profile changed: it is the same layout.
  expect((await studio.get('/api/midi')).devices.find((d: { name: string }) => d.name === TEST_PORT).profile).toBe('apc40-mk2-perso');
  await el(page, 'bank.1').hover();
  await expect(page.locator('#vapcTip')).toContainText('Pan — ');

  // « Exporter la disposition »: the profile in use, as a JSON download.
  const [download] = await Promise.all([page.waitForEvent('download'), page.locator('#vapcExport').click()]);
  expect(download.suggestedFilename()).toBe('apc40-mk2-perso.json');
  const exported = JSON.parse(await (await import('node:fs/promises')).readFile((await download.path())!, 'utf8'));
  const call = async (path: string, body: unknown) => (await fetch(studio.url + path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })).json();
  const { slug } = await call('/api/midi/profile/import', { profile: exported });
  expect(slug).not.toBe('apc40-mk2-perso');
  expect((await call('/api/midi/profile/export', { slug })).profile).toEqual(exported);

  await el(page, 'bank.1').click({ button: 'right' });
  await page.locator('#vapcMenuUnmap').click();
  await expect.poll(async () => (await studio.get('/api/midi/virtual')).elements.find((e: { id: string }) => e.id === 'bank.1').map).toBeNull();
  // Back to the built-in layout for the other tests.
  await studio.post('/api/midi/profile', { port: TEST_PORT, profile: 'apc40-mk2' });
});

test('with no controller connected the drawing still plays cues, on the model picked on screen', async ({ page }) => {
  await studio.post('/api/midi/device', { port: TEST_PORT, enabled: false });
  try {
    await expect(page.locator('#vapcInfo')).toContainText('aucun APC branché', { timeout: 8000 });
    await expect(page.locator('#vapcModel')).toBeEnabled();
    await page.locator('#vapcModel').selectOption('Apc40');
    await expect.poll(async () => (await studio.get('/api/midi/virtual')).model).toBe('Apc40');
    // The APC40 has no Crossfader A/B row and its pad (0,0) is note 0x35, channel 1.
    await expect(page.locator('#vapc [data-vapc^="ab."]')).toHaveCount(0);
    await el(page, 'pad.1.1').hover();
    await expect(page.locator('#vapcTip')).toContainText('Note 0x35 · can. 1');
    const first = pageCues(0)[0];
    await el(page, 'pad.1.1').click();
    await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(first.id);
    // APC40: green while playing.
    await expect.poll(async () => (await studio.get('/api/midi/leds')).leds['note:53:0']).toBe(1);
  } finally {
    await studio.post('/api/midi/virtual/model', { model: 'Apc40Mk2' });
    await studio.post('/api/midi/device', { port: TEST_PORT, enabled: true });
  }
});
