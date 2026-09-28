// MIDI without hardware (T-209): a studio started with --no-midi
// --midi-test gets a simulated APC40 mkII; bytes injected through
// /api/midi/inject go through the real mapping engine and T-208 safety.
// The profile is a small test layout (midi.ts), written before start.
import { test, expect, useStudio, openUi, reveal } from '../studio';
import { Apc, testProfileFiles, DEVICE_INQUIRY, NOTE_ARM, NOTE_SCENE_1, NOTE_STOP_ALL, NOTE_SHIFT, CC_CUE_LEVEL, TEST_PORT } from '../midi';

/** APC40 mkII palette indexes used by LED feedback (T-205). */
const MK2_WHITE = 3;
const MK2_GREEN = 21;

const studio = useStudio({ midiTest: true, files: testProfileFiles() });
const apc = new Apc(studio);
let catalog: Awaited<ReturnType<typeof studio.presets>>;
let startedAt = 0;

const state = () => studio.state();
const cueValues = () => studio.controlValues();
const activeCues = async () => (await studio.frame() as any).cues.active.map((c: { cue: string }) => c.cue) as string[];
const size = async () => (await studio.live()).size;
const pageCues = (i: number) => catalog.presets.filter(p => p.category === catalog.categories[i]);

test.beforeAll(async () => {
  startedAt = Date.now();
  catalog = await studio.presets();
});

test.beforeEach(async ({ page }) => {
  await apc.shift(false);
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  await studio.post('/api/control', { id: 'page.1' });
  await studio.post('/api/midi/safety', { allow_arm: false });
  await openUi(page, studio);
});

test('the simulated APC40 mkII is detected, taken over and shown in the UI', async ({ page }) => {
  await expect.poll(async () => {
    const m = await studio.get('/api/midi');
    const d = m.devices.find((x: { name: string }) => x.name === TEST_PORT);
    return d && [m.test, d.model, d.profile, d.connected];
  }).toEqual([true, 'Apc40Mk2', 'e2e-apc', true]);
  await expect(page.locator('#midiStatus')).toHaveText('MIDI : APC40 mkII connecté');
  const dev = await apc.device();
  expect(dev.sent[0]).toEqual(DEVICE_INQUIRY);
  expect(dev.mode).toBe(0x41); // Introduction: the studio drives the LEDs
});

test('a grid pad plays its cue, a second press stops it', async ({ page }) => {
  const cue = pageCues(0)[1];
  await apc.pressPad(0, 1);
  await apc.releasePad(0, 1);
  await expect.poll(async () => (await cueValues()).active_cue).toBe(cue.id);
  await expect(page.locator(`#cues .cue[data-id="${cue.id}"]`)).toHaveClass(/active/);
  expect((await studio.get('/api/midi')).last.msg).toMatchObject({ kind: 'note_off', note: 33 });
  // LED feedback (T-205), APC40 mkII palette: green = playing, white = cue present.
  await expect.poll(() => apc.ledAt(0, 1)).toBe(MK2_GREEN);
  expect(await apc.ledAt(0, 0)).toBe(MK2_WHITE);

  await apc.pressPad(0, 1);
  await apc.releasePad(0, 1);
  await expect.poll(activeCues).not.toContain(cue.id);
  await expect.poll(() => apc.ledAt(0, 1)).toBe(MK2_WHITE);

  // Started from the UI (tab closed or not, the studio drives the LEDs).
  expect(await studio.post('/api/control', { id: 'grid.1.1.1', value: 1 })).toBe(200);
  expect(await studio.post('/api/control', { id: 'grid.1.1.1', value: 0 })).toBe(200);
  await expect.poll(() => apc.ledAt(0, 0)).toBe(MK2_GREEN);
  await expect.poll(() => apc.ledAt(0, 1)).toBe(MK2_WHITE);
});

test('« Retour LED » unticked leaves the APC dark, ticked lights it again', async ({ page }) => {
  await expect.poll(() => apc.ledAt(0, 0)).toBe(MK2_WHITE);
  await reveal(page, '#midiPanel summary');
  await page.locator('#midiPanel summary').click();
  const box = page.locator('[data-mled="0"]');
  await expect(box).toBeChecked();
  await box.uncheck();
  await expect.poll(() => apc.ledAt(0, 0)).toBe(0);
  await expect.poll(async () => (await studio.get('/api/midi')).devices[0].leds).toBe(false);
  await page.locator('[data-mled="0"]').check();
  await expect.poll(() => apc.ledAt(0, 0)).toBe(MK2_WHITE);
});

test('track fader 1 takes over master size with pickup; the encoder steps', async () => {
  expect(await studio.post('/api/control', { id: 'master.size', value: 1.5 })).toBe(200); // fader position 75 %
  // The fake reported its faders at 0: 64 (50 %) hasn't reached 75 % yet.
  await apc.moveFader(0, 64);
  await new Promise(r => setTimeout(r, 300)); // proving nothing happens
  expect(await size()).toBe(1.5);
  // 96 (75.6 %) is within 3 %: caught, and the fader drives it from now on.
  await apc.moveFader(0, 96);
  await expect.poll(size).toBeCloseTo(2 * 96 / 127, 3);
  await apc.moveFader(0, 114);
  await expect.poll(size).toBeCloseTo(2 * 114 / 127, 3);

  // Changed elsewhere: the fader must catch the new value again.
  expect(await studio.post('/api/control', { id: 'master.size', value: 0.5 })).toBe(200);
  await apc.moveFader(0, 100);
  await new Promise(r => setTimeout(r, 300));
  expect(await size()).toBe(0.5);
  await apc.moveFader(0, 20); // crosses 25 % on the way down
  await expect.poll(size).toBeCloseTo(2 * 20 / 127, 3);

  await apc.turnKnob(CC_CUE_LEVEL, 3);
  await expect.poll(async () => (await studio.live()).pos_x).toBeCloseTo(0.3, 3);
  await apc.turnKnob(CC_CUE_LEVEL, -1);
  await expect.poll(async () => (await studio.live()).pos_x).toBeCloseTo(0.2, 3);
});

test('Stop All blacks out and latches the emergency stop', async ({ page }) => {
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await expect.poll(async () => (await state()).armed).toBe(true);
  await apc.button(NOTE_STOP_ALL, true);
  await apc.button(NOTE_STOP_ALL, false);
  await expect.poll(async () => { const s = await state(); return [s.armed, s.estop, s.arm.estop?.source]; }).toEqual([false, true, 'midi']);
  await expect(page.locator('#estopBanner')).toBeVisible();
  // Latched: nothing re-arms until it is reset by hand.
  expect(await studio.post('/api/arm', { on: true })).toBe(409);
  await page.locator('#estopReset').click();
  await expect(page.locator('#estopBanner')).toBeHidden();
  expect((await state()).armed).toBe(false);
});

test('Shift selects the second layer of a button', async ({ page }) => {
  await apc.button(NOTE_SCENE_1, true);
  await apc.button(NOTE_SCENE_1, false);
  await apc.shift(true);
  await apc.button(NOTE_SCENE_1, true);
  await apc.button(NOTE_SCENE_1, false);
  await expect.poll(async () => (await cueValues()).cue_page).toBe(2);
  await expect(page.locator('#cueTabs button').nth(1)).toHaveClass(/active/);
  await apc.shift(false);
  // The grid follows the page: pad (0,0) is now page 2's first cue.
  await apc.pressPad(0, 0);
  await apc.releasePad(0, 0);
  await expect.poll(async () => (await cueValues()).active_cue).toBe(pageCues(1)[0].id);
  // Without Shift, the same button is page 1 again.
  await apc.button(NOTE_SCENE_1, true);
  await apc.button(NOTE_SCENE_1, false);
  await expect.poll(async () => (await cueValues()).cue_page).toBe(1);
});

// Last: it needs the controller plugged in for more than 5 s (T-208 guard).
test('nothing arms from the controller by default', async () => {
  test.setTimeout(60_000);
  const wait = startedAt + 5_500 - Date.now();
  if (wait > 0) await new Promise(r => setTimeout(r, wait)); // the plug guard is the subject here
  expect((await studio.get('/api/midi')).safety.allow_arm).toBe(false);

  // Shift + the arm button held 1.5 s: refused, the option is off.
  await apc.shift(true);
  await apc.button(NOTE_ARM, true);
  await new Promise(r => setTimeout(r, 1_500));
  await apc.button(NOTE_ARM, false);
  // Every note and CC on the 16 channels, Shift still held (Stop All left
  // out: its blackout would hide an arming).
  for (let ch = 0; ch < 16; ch++) {
    for (const half of [0, 64]) {
      const bytes: number[] = [];
      for (let n = half; n < half + 64; n++) {
        if (n === NOTE_STOP_ALL || n === NOTE_SHIFT) continue;
        bytes.push(0x90 | ch, n, 127, 0xb0 | ch, n, 127, 0x80 | ch, n, 0, 0xb0 | ch, n, 0);
      }
      await apc.inject(bytes);
    }
  }
  await apc.button(NOTE_ARM, true);
  await new Promise(r => setTimeout(r, 1_500));
  let s = await state();
  expect([s.armed, s.estop]).toEqual([false, false]);
  await apc.button(NOTE_ARM, false);

  // Positive control (preview only): once allowed, the same gesture arms,
  // so the refusal above really came from the option.
  expect(await studio.post('/api/midi/safety', { allow_arm: true })).toBe(200);
  await apc.button(NOTE_ARM, true);
  await expect.poll(async () => (await state()).armed, { timeout: 3_000 }).toBe(true);
  await apc.button(NOTE_ARM, false);
  await apc.shift(false);
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
  expect(await studio.post('/api/midi/safety', { allow_arm: false })).toBe(200);
  s = await state();
  expect(s.armed).toBe(false);
});
