// Timecode output (T-417): TC-OUT sends the timeline's position as MTC on
// the simulated MIDI port of --midi-test (GET /api/midi/sent reads what
// it received: never a real port), only while the show plays. It is off
// by default, Stop stops the stream, and the timecode input cannot listen
// to the port the output sends on (the self-drive trap). Preview only:
// the laser is never armed, and nothing goes on the network (Art-Net is
// covered by the unit tests, on loopback).
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import { TEST_PORT } from '../midi';
import type { Page } from '@playwright/test';

const studio = useStudio({ midiTest: true });

interface OutSummary { enabled: boolean; sending: boolean; last: string | null; blocked: string | null; error: string | null }
const frame = () => studio.get<{ armed: boolean; timeline: { playing: boolean; timecode_out: OutSummary } }>('/api/frame');

/** Quarter frames (F1 xx) and full frames (F0 7F … 01 01 …) the simulated port received. */
async function received() {
  const s = await studio.get<{ devices: { port: string; sent: number[][] }[] }>('/api/midi/sent');
  const sent = s.devices.find(d => d.port === TEST_PORT)?.sent ?? [];
  return {
    quarters: sent.filter(m => m[0] === 0xf1).length,
    fulls: sent.filter(m => m[0] === 0xf0 && m[1] === 0x7f && m[3] === 0x01 && m[4] === 0x01).length,
  };
}

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

async function choosePort(page: Page) {
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#tcoPort option', { hasText: TEST_PORT })).toHaveCount(1, { timeout: 10_000 });
  await page.locator('#tcoPort').selectOption(TEST_PORT);
  await expect.poll(async () => (await studio.get('/api/timecode/out')).config.mtc_port).toBe(TEST_PORT);
}

test.beforeAll(async () => {
  const catalog = await studio.presets();
  const show = {
    name: 'Show TC-OUT',
    time_base: 'seconds',
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len: 60, source: { kind: 'cue', id: catalog.presets[0].id } }] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/timeline/stop');
  await studio.post('/api/timecode/out', { enabled: false, mtc_port: null, artnet: null, fps: '25', offset_s: 0 });
  await studio.post('/api/timecode', { source: 'off', midi_port: null });
  await studio.reset();
  await openUi(page, studio);
});

test('off by default: TC-OUT sends MTC only while the show plays, Stop stops it', async ({ page }) => {
  // Off at start: nothing configured, nothing sent.
  const first = await studio.get('/api/timecode/out');
  expect(first.config.enabled).toBe(false);
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#tcoToggle')).toHaveAttribute('aria-pressed', 'false');
  await expect(page.locator('#tcoState')).toHaveText('Émission : désactivée');

  await choosePort(page);
  await page.locator('#tcoOffset').fill('3600');
  await page.locator('#tcoOffset').dispatchEvent('change');
  await page.locator('#tcoToggle').click();
  await expect(page.locator('#tcoToggle')).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(async () => (await frame()).timeline.timecode_out.enabled).toBe(true);

  // Enabled but nothing plays: no stream.
  const idle = await received();
  await sleep(400);
  expect((await received()).quarters).toBe(idle.quarters);

  // The show plays: a full frame, then quarter frames (100 a second at 25 im/s).
  expect(await studio.post('/api/timeline/load', { name: 'Show TC-OUT' })).toBe(200);
  const before = await received();
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await expect.poll(async () => (await received()).quarters - before.quarters, { timeout: 5000 }).toBeGreaterThan(60);
  expect((await received()).fulls).toBeGreaterThan(before.fulls);
  await expect(page.locator('#tcoState')).toHaveText(/^Émission : 01:00:0\d:\d\d$/);
  const f = await frame();
  expect(f.timeline.timecode_out.sending).toBe(true);
  expect(f.timeline.timecode_out.last).toMatch(/^01:00:/);
  expect(f.armed).toBe(false);

  // Stop: no new message.
  await page.locator('#tlStop').click();
  await expect.poll(async () => (await frame()).timeline.timecode_out.sending).toBe(false);
  await sleep(200);
  const stopped = await received();
  await sleep(600);
  const after = await received();
  expect(after.quarters).toBe(stopped.quarters);
  expect(after.fulls).toBe(stopped.fulls);
  await expect(page.locator('#tcoState')).toHaveText(/à l'arrêt/);

  // Off again: still nothing when the show plays.
  await page.locator('#tcoToggle').click();
  await expect.poll(async () => (await frame()).timeline.timecode_out.enabled).toBe(false);
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await sleep(500);
  expect((await received()).quarters).toBe(after.quarters);
  expect((await frame()).armed).toBe(false);
});

test('the timecode input cannot listen to the port TC-OUT sends on', async ({ page }) => {
  await choosePort(page);
  await page.locator('#tcoToggle').click();
  await expect.poll(async () => (await frame()).timeline.timecode_out.enabled).toBe(true);
  // TC-IN on MTC (any port, so ours too): refused, with the reason.
  await page.locator('#tcSource').selectOption('mtc');
  await expect(page.locator('#tcMsg')).toHaveText('Entrée et sortie de timecode sur le même port : le studio se piloterait lui-même.');
  await expect.poll(async () => (await studio.get('/api/timecode')).config.source).toBe('off');
  // The other way round: the input on that port first, then TC-OUT.
  await page.locator('#tcoToggle').click();
  await expect.poll(async () => (await frame()).timeline.timecode_out.enabled).toBe(false);
  expect(await studio.post('/api/timecode', { source: 'mtc', midi_port: TEST_PORT })).toBe(200);
  await page.locator('#tcoToggle').click();
  await expect(page.locator('#tcoMsg')).toHaveText('Entrée et sortie de timecode sur le même port : le studio se piloterait lui-même.');
  expect((await studio.get('/api/timecode/out')).config.enabled).toBe(false);
  expect((await frame()).armed).toBe(false);
});
