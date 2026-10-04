// Incoming timecode (T-168): MTC from the simulated MIDI port of
// --midi-test (POST /api/midi/inject, our own quarter frames: never a real
// port) drives the timeline once TC-IN is armed and the external inputs
// gate is open; a dropout stops the show after the freewheel; Escape drops
// TC-IN and the timecode restarts nothing. The laser is never armed
// (preview only), and LTC is covered by the unit tests (--no-audio here).
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import { TEST_PORT } from '../midi';
import type { Page } from '@playwright/test';

const studio = useStudio({ midiTest: true });

interface Summary { source: string; armed: boolean; status: string; tc: string | null; rate: string | null; blocked: string | null; chasing: boolean }
interface TimelineState { name: string | null; position: number; playing: boolean }
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
  await studio.post('/api/timecode', { source: 'off', offset_s: 0, keep_running: false, freewheel_frames: 25 });
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
  await expect.poll(async () => (await studio.get('/api/timecode')).config).toMatchObject({ offset_s: 3600, keep_running: true });
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
  await expect(page.locator('#tcLed')).toHaveAttribute('data-status', /locked|freewheel/);
  expect(f.armed).toBe(false);

  // Timecode stops: after the freewheel (25 frames = 1 s) the show stops.
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
