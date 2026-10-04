// Incoming timecode (T-168): MTC from the simulated MIDI port of
// --midi-test (POST /api/midi/inject, our own quarter frames: never a real
// port) drives the timeline once TC-IN is armed and the external inputs
// gate is open; a dropout stops the show after the post-roll; Escape drops
// TC-IN and the timecode restarts nothing. The laser is never armed
// (preview only), and LTC is covered by the unit tests (--no-audio here).
// Art-Net timecode (T-409) comes from POST /api/test/timecode
// (--test-hooks): our own ArtTimeCode packets handed to the receiver, no
// socket and nothing on the network.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import { TEST_PORT } from '../midi';
import type { Page } from '@playwright/test';

const studio = useStudio({ midiTest: true, testHooks: true });

interface Summary { source: string; armed: boolean; status: string; state: string; status_fr: string; tc: string | null; rate: string | null; blocked: string | null; chasing: boolean }
interface TimelineState { name: string | null; position: number; playing: boolean; timecode?: Summary }
const frame = () => studio.get<{ timecode: Summary; timeline: TimelineState; armed: boolean }>('/api/frame');

/** The eight quarter frames of `frame` (frames since 00:00:00:00) at 25 im/s. */
function quarterFrames(frame: number): number[] {
  const f = frame % 25, s = Math.floor(frame / 25) % 60, m = Math.floor(frame / 1500) % 60, h = Math.floor(frame / 90000);
  const nib = [f & 15, f >> 4, s & 15, s >> 4, m & 15, m >> 4, h & 15, ((h >> 4) & 1) | (1 << 1)];
  return nib.flatMap((n, k) => [0xf1, (k << 4) | n]);
}

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

/** Runs MTC from `from` frames for `ms`, one complete time every 2 frames (80 ms). */
async function runMtc(from: number, ms: number) {
  const t0 = Date.now();
  for (let k = 0; Date.now() - t0 < ms; k++) {
    const due = t0 + k * 80;
    if (due > Date.now()) await sleep(due - Date.now());
    expect(await studio.post('/api/midi/inject', { port: TEST_PORT, bytes: quarterFrames(from + 2 * k) })).toBe(200);
  }
}

async function chooseMtc(page: Page) {
  await openWorkspace(page, 'timeline');
  await page.locator('#tcSource').selectOption('mtc');
  await expect.poll(async () => (await studio.get<{ config: { source: string } }>('/api/timecode')).config.source).toBe('mtc');
  await expect(page.locator('#tcPortRow')).toBeVisible();
}

test.beforeAll(async () => {
  const catalog = await studio.presets();
  const show = {
    name: 'Show TC',
    time_base: 'seconds',
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len: 60, source: { kind: 'cue', id: catalog.presets[0].id } }] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/timecode/arm', { on: false });
  await studio.post('/api/timecode', { source: 'off', offset_s: 0, after_roll_s: 1, pre_roll_frames: 5 });
  await studio.post('/api/external/gate', { on: false });
  await studio.post('/api/timeline/stop');
  await openUi(page, studio);
});

test('the panel sets the source and TC-IN, and says why nothing moves yet', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#tcReadout')).toHaveText('--:--:--:--');
  // No source: TC-IN is refused.
  await page.locator('#tcArm').click();
  await expect(page.locator('#tcArm')).not.toHaveClass(/on/);
  await chooseMtc(page);
  await page.locator('#tcOffset').fill('3600');
  await page.locator('#tcOffset').dispatchEvent('change');
  await page.locator('#tcKeep').check();
  await expect.poll(async () => (await studio.get('/api/timecode')).config).toMatchObject({ offset_s: 3600, after_roll_s: null });
  await page.locator('#tcArm').click();
  await expect(page.locator('#tcArm')).toHaveClass(/on/);
  await expect.poll(async () => (await frame()).timecode.armed).toBe(true);
  // Timecode arrives, but the gate is closed: shown, not applied.
  await runMtc(3600 * 25, 400);
  await expect(page.locator('#tcReadout')).toContainText('TC 01:00:00:');
  await expect(page.locator('#tcStatus')).toContainText('entrées externes fermées');
  expect((await frame()).timeline.playing).toBe(false);
  expect((await frame()).armed).toBe(false);
});

test('MTC drives the show, a dropout stops it, Escape drops TC-IN', async ({ page }) => {
  await chooseMtc(page);
  expect(await studio.post('/api/timeline/load', { name: 'Show TC' })).toBe(200);
  expect(await studio.post('/api/external/gate', { on: true })).toBe(200);
  await page.locator('#tcArm').click();
  await expect.poll(async () => (await frame()).timecode.armed).toBe(true);

  // 1.5 s of timecode from 00:00:10:00: the show plays there.
  await runMtc(250, 1500);
  const f = await frame();
  expect(f.timeline.playing).toBe(true);
  expect(f.timecode.chasing).toBe(true);
  expect(f.timeline.position).toBeGreaterThan(10.8);
  expect(f.timeline.position).toBeLessThan(12.5);
  expect(f.timecode.rate).toBe('25');
  await expect(page.locator('#tcLed')).toHaveAttribute('data-status', /locked|afterroll/);
  expect(f.armed).toBe(false);

  // Timecode stops: after the post-roll (1 s) the show stops.
  await expect.poll(async () => (await frame()).timeline.playing, { timeout: 4000 }).toBe(false);
  await expect(page.locator('#tcLed')).toHaveAttribute('data-status', 'lost');

  // Back again, then Escape: TC-IN drops and the timecode restarts nothing.
  await runMtc(500, 600);
  await expect.poll(async () => (await frame()).timeline.playing).toBe(true);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await frame()).timecode.armed).toBe(false);
  await expect(page.locator('#tcArm')).not.toHaveClass(/on/);
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/external/gate', { on: true });
  await runMtc(600, 800);
  const after = await frame();
  expect(after.timeline.playing).toBe(false);
  expect(after.timecode.blocked).toBe('TC-IN désarmé');
  expect(after.armed).toBe(false);
});

/** Art-Net timecode from `from` frames at 25 im/s for `ms`, one frame every 40 ms. */
async function runArtNet(from: number, ms: number) {
  const t0 = Date.now();
  for (let k = 0; Date.now() - t0 < ms; k++) {
    const due = t0 + k * 40;
    if (due > Date.now()) await sleep(due - Date.now());
    expect(await studio.post('/api/test/timecode', { frames: from + k, rate: '25' })).toBe(200);
  }
}

test('Art-Net timecode per show drives it, the post-roll ends in « Timecode perdu », Alt+T shows it big', async ({ page }) => {
  const catalog = await studio.presets();
  const show = {
    name: 'Show Art-Net',
    time_base: 'seconds',
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len: 60, source: { kind: 'cue', id: catalog.presets[0].id } }] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
  expect(await studio.post('/api/timeline/load', { name: 'Show Art-Net' })).toBe(200);
  await openWorkspace(page, 'timeline');
  // The show gets its own source; the global one stays « Aucune ».
  await expect(page.locator('#tcPerShow')).toBeEnabled({ timeout: 5000 });
  await page.locator('#tcPerShow').check();
  await expect.poll(async () => (await studio.get<{ per_show: boolean }>('/api/timecode')).per_show).toBe(true);
  await page.locator('#tcSource').selectOption('artnet');
  await expect.poll(async () => (await studio.get<{ show: { source: string } | null; config: { source: string } }>('/api/timecode'))).toMatchObject({ show: { source: 'artnet' }, config: { source: 'off' } });
  await expect(page.locator('#tcIfaceRow')).toBeVisible();
  expect(await studio.post('/api/external/gate', { on: true })).toBe(200);
  await page.locator('#tcArm').click();
  await expect.poll(async () => (await frame()).timecode.armed).toBe(true);

  // 1.2 s from 00:00:20:00: past the pre-roll, the show plays there.
  await runArtNet(500, 1200);
  const f = await frame();
  expect(f.timeline.playing).toBe(true);
  expect(f.timeline.timecode?.state).toBe('locked');
  expect(f.timeline.position).toBeGreaterThan(20.5);
  expect(f.timeline.position).toBeLessThan(22);
  expect(f.armed).toBe(false);

  // Alt+T: the big readout.
  await page.keyboard.press('Alt+KeyT');
  await expect(page.locator('#tcBig')).toBeVisible();
  await expect(page.locator('#tcBigShow')).toHaveText('Show Art-Net');
  await expect(page.locator('#tcBigTc')).toHaveText(/^00:00:2\d:\d\d$/);

  // Signal cut: post-roll (1 s), then stopped and « Timecode perdu », red.
  await expect.poll(async () => (await frame()).timeline.playing, { timeout: 4000 }).toBe(false);
  expect((await frame()).timecode.state).toBe('lost');
  await expect(page.locator('#tcBig')).toHaveAttribute('data-state', 'lost');
  await expect(page.locator('#tcBigState')).toContainText('Timecode perdu');
  await page.keyboard.press('Alt+KeyT');
  await expect(page.locator('#tcBig')).toBeHidden();
  await page.locator('#tcBigBtn').click();
  await expect(page.locator('#tcBig')).toBeVisible();
  await page.locator('#tcBig').click();
  await expect(page.locator('#tcBig')).toBeHidden();

  // Escape drops TC-IN: the receiver closes, nothing restarts.
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await frame()).timecode.armed).toBe(false);
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/external/gate', { on: true });
  await runArtNet(900, 400);
  expect((await frame()).timeline.playing).toBe(false);
  expect((await frame()).armed).toBe(false);
  await studio.post('/api/timeline/stop');
});
