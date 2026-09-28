// The built-in APC40 mkII layout (T-204), driven through the simulated
// controller of --midi-test: no profile is seeded, so the studio must pick
// « APC40 mkII — Laser Studio » by itself from the Device Inquiry reply.
import { test, expect, useStudio, openUi } from '../studio';
import { Apc, NOTE_STOP_ALL, TEST_PORT } from '../midi';

const NOTE_SCENE = (n: number) => 0x51 + n; // scene launch 1–5 = 0x52–0x56
const NOTE_DOWN = 0x5f;
const NOTE_TAP = 0x63;
const MASTER = 8;

const studio = useStudio({ midiTest: true });
const apc = new Apc(studio);
let catalog: Awaited<ReturnType<typeof studio.presets>>;

const state = () => studio.state();
const cueValues = () => studio.controlValues();
const pageCues = (i: number) => catalog.presets.filter(p => p.category === catalog.categories[i]);
const brightness = async () => (await studio.live()).brightness;
const sleep = (ms: number) => new Promise(r => setTimeout(r, Math.max(0, ms)));

async function press(note: number) {
  await apc.button(note, true);
  await apc.button(note, false);
}

test.beforeAll(async () => {
  catalog = await studio.presets();
  await expect.poll(async () => {
    const d = (await studio.get('/api/midi')).devices.find((x: { name: string }) => x.name === TEST_PORT);
    return d && [d.model, d.profile, d.connected];
  }).toEqual(['Apc40Mk2', 'apc40-mk2', true]);
});

test.beforeEach(async ({ page }) => {
  await apc.shift(false);
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('the APC40 mkII gets the built-in layout, without warnings', async ({ page }) => {
  const m = await studio.get('/api/midi');
  expect(m.errors).toEqual([]);
  expect(m.devices.find((d: { name: string }) => d.name === TEST_PORT).profile_error ?? null).toBeNull();
  await expect(page.locator('#midiStatus')).toHaveText('MIDI : APC40 mkII connecté');
  await expect(page.locator('[data-mprof="0"] option:checked')).toHaveText('APC40 mkII — Laser Studio (intégré)');
});

test('the top-left pad plays the 1st cue of the page, the bottom-right one the 40th', async ({ page }) => {
  const cues = pageCues(0);
  expect(cues.length).toBeGreaterThanOrEqual(40);
  await apc.pressPad(0, 0);
  await apc.releasePad(0, 0);
  await expect.poll(async () => (await cueValues()).active_cue).toBe(cues[0].id);
  await expect(page.locator(`#cues .cue[data-id="${cues[0].id}"]`)).toHaveClass(/active/);
  await apc.pressPad(4, 7);
  await apc.releasePad(4, 7);
  await expect.poll(async () => (await cueValues()).active_cue).toBe(cues[39].id);
});

test('scene launch buttons change the page, and a page tab clicked in the UI moves the grid', async ({ page }) => {
  await press(NOTE_SCENE(3));
  await expect.poll(async () => (await cueValues()).cue_page).toBe(3);
  await expect(page.locator('#cueTabs button').nth(2)).toHaveClass(/active/);
  // Shift + scene launch 1 = page 6.
  await apc.shift(true);
  await press(NOTE_SCENE(1));
  await apc.shift(false);
  await expect.poll(async () => (await cueValues()).cue_page).toBe(6);
  // Down = next page.
  await press(NOTE_DOWN);
  await expect.poll(async () => (await cueValues()).cue_page).toBe(7);

  // The other way round: the UI tab sets the page the pads play from.
  await page.locator('#cueTabs button').nth(3).click();
  await expect.poll(async () => (await cueValues()).cue_page).toBe(4);
  await apc.pressPad(0, 0);
  await apc.releasePad(0, 0);
  await expect.poll(async () => (await cueValues()).active_cue).toBe(pageCues(3)[0].id);
});

test('the master fader drives master brightness with pickup (no jump)', async () => {
  expect(await brightness()).toBe(1);
  // The device reported its faders at 0: half-way hasn't reached 100 % yet.
  await apc.moveFader(MASTER, 64);
  await sleep(300); // proving nothing happens
  expect(await brightness()).toBe(1);
  // Caught at the top, then followed.
  await apc.moveFader(MASTER, 127);
  await apc.moveFader(MASTER, 64);
  await expect.poll(brightness).toBeCloseTo(64 / 127, 3);
  await apc.moveFader(MASTER, 32);
  await expect.poll(brightness).toBeCloseTo(32 / 127, 3);
});

test('Stop All is a blackout that latches the e-stop, with or without Shift', async ({ page }) => {
  for (const shift of [false, true]) {
    await studio.post('/api/estop/reset', {});
    expect(await studio.post('/api/arm', { on: true })).toBe(200);
    await expect.poll(async () => (await state()).armed).toBe(true);
    if (shift) await apc.shift(true);
    await press(NOTE_STOP_ALL);
    if (shift) await apc.shift(false);
    await expect.poll(async () => { const s = await state(); return [s.armed, s.estop, s.arm.estop?.source]; }).toEqual([false, true, 'midi']);
    await expect(page.locator('#estopBanner')).toBeVisible();
    expect(await studio.post('/api/arm', { on: true })).toBe(409);
  }
  // Arming from the controller is off by default.
  expect((await studio.get('/api/midi')).safety.allow_arm).toBe(false);
});

test('Tap tempo: four taps at 120 BPM set the tempo', async () => {
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 90 })).toBe(200);
  // The spacing is the subject: taps scheduled on absolute times, 500 ms apart.
  const t0 = Date.now();
  for (let i = 0; i < 4; i++) {
    await sleep(t0 + i * 500 - Date.now());
    await apc.button(NOTE_TAP, true);
    await apc.button(NOTE_TAP, false);
  }
  await expect.poll(async () => (await state()).tempo.source).toBe('tap');
  const bpm = (await state()).tempo.bpm;
  expect(Math.abs(bpm - 120)).toBeLessThanOrEqual(1);
});

