// LTC out (T-501): TC-OUT sends the timeline's position as LTC on a channel
// of an audio output. Here the output is the simulated one of
// --test-ltc-out (« Test LTC »), which decodes what it plays with the
// studio's own LTC reader: no audio device is ever opened. Off by default,
// only while the show plays, Stop stops it; the laser is never armed.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio({ ltcTest: true });

interface OutStatus { ltc_frames: number; ltc_open: string | null; ltc_heard: string | null; ltc_available: boolean; ltc_devices: { name: string; channels: number }[] }
const status = async () => (await studio.get<{ status: OutStatus; config: { enabled: boolean; ltc: { device: string; channel: number; level_db: number } | null } }>('/api/timecode/out'));
const frame = () => studio.get<{ armed: boolean; timeline: { timecode_out: { enabled: boolean; sending: boolean } } }>('/api/frame');
const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

test.beforeAll(async () => {
  const catalog = await studio.presets();
  const show = {
    name: 'Show LTC',
    time_base: 'seconds',
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len: 60, source: { kind: 'cue', id: catalog.presets[0].id } }] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/timeline/stop');
  await studio.post('/api/timecode/out', { enabled: false, mtc_port: null, artnet: null, ltc: null, fps: '25', offset_s: 0 });
  await studio.reset();
  await openUi(page, studio);
});

test('LTC plays on the chosen output channel only while the show plays', async ({ page }) => {
  const first = await status();
  expect(first.config.enabled).toBe(false);
  expect(first.config.ltc).toBeNull();
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#tcoLtcDevice')).toBeHidden();

  // Choose the simulated output, channel 2, then switch TC-OUT on.
  await page.locator('#tcoLtc').check();
  await expect.poll(async () => (await status()).config.ltc?.device).toBe('Test LTC');
  await expect(page.locator('#tcoLtcDevice')).toBeVisible();
  await expect(page.locator('#tcoLtcLevel')).toHaveValue('-18');
  await page.locator('#tcoLtcChannel').fill('2');
  await page.locator('#tcoLtcChannel').dispatchEvent('change');
  await expect.poll(async () => (await status()).config.ltc?.channel).toBe(2);
  // A channel the device does not have: refused, with the reason.
  await page.locator('#tcoLtcChannel').fill('3');
  await page.locator('#tcoLtcChannel').dispatchEvent('change');
  await expect(page.locator('#tcoMsg')).toContainText('voie 3 absente');
  expect((await status()).config.ltc?.channel).toBe(2);
  await page.locator('#tcoOffset').fill('3600');
  await page.locator('#tcoOffset').dispatchEvent('change');
  await page.locator('#tcoToggle').click();
  await expect.poll(async () => (await frame()).timeline.timecode_out.enabled).toBe(true);
  await expect.poll(async () => (await status()).status.ltc_open).toBe('Test LTC · voie 2');

  // Enabled but nothing plays: silence.
  const idle = (await status()).status.ltc_frames;
  await sleep(400);
  expect((await status()).status.ltc_frames).toBe(idle);

  // The show plays: frames go out, and the simulated card decodes 01:00:0x.
  expect(await studio.post('/api/timeline/load', { name: 'Show LTC' })).toBe(200);
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await expect.poll(async () => (await status()).status.ltc_frames - idle, { timeout: 5000 }).toBeGreaterThan(20);
  await expect.poll(async () => (await status()).status.ltc_heard, { timeout: 5000 }).toMatch(/^01:00:0\d:\d\d$/);
  expect((await frame()).armed).toBe(false);

  // Stop: no new frame.
  await page.locator('#tlStop').click();
  await expect.poll(async () => (await frame()).timeline.timecode_out.sending).toBe(false);
  await sleep(200);
  const stopped = (await status()).status.ltc_frames;
  await sleep(600);
  expect((await status()).status.ltc_frames).toBe(stopped);

  // LTC off: the stream is closed.
  await page.locator('#tcoLtc').uncheck();
  await expect.poll(async () => (await status()).config.ltc).toBeNull();
  await expect.poll(async () => (await status()).status.ltc_open).toBeNull();
  expect((await frame()).armed).toBe(false);
});
