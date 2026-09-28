// Audio input source (T-230), through the API only. The test studio runs
// with --no-audio: no microphone or interface is ever opened, the native
// capture reports « disabled », and the browser source (POST /api/audio)
// keeps driving the looks as before.
import { test, expect, useStudio, extent } from '../studio';

const studio = useStudio();
const audio = async () => (await studio.state()).audio;
const setSource = (source: string) => studio.post('/api/audio/config', { source });

/** Posts browser features until the frame has picked them up (they go stale after 500 ms). */
async function extentWith(bass: number) {
  await studio.post('/api/audio', { level: bass, bass, beat: 0 });
  return extent((await studio.frame()).points);
}

test.beforeAll(async () => {
  await studio.post('/api/control', { id: 'audio.enabled', value: true });
  await studio.post('/api/control', { id: 'audio.size', value: 1 });
});

test('--no-audio: no capture, no device, nothing fails', async () => {
  const devices = await studio.get('/api/audio/devices');
  expect(devices).toEqual({ capture: false, devices: [] });
  const a = await audio();
  expect(a.capture).toBe(false);
  expect(a.state).toBe('disabled');
  expect(a.level_db).toBeNull();
  // Default source is the browser: the studio never opens the Mac's mic by itself.
  expect(await studio.get('/api/audio/config')).toEqual({ source: 'browser', device: null, buffer_frames: 256 });
});

test('Native without a capture: the browser features stand in', async () => {
  await studio.post('/api/audio/config', { source: 'native' });
  await studio.post('/api/audio', { level: 0.6, bass: 0.4, beat: 2 });
  const a = await audio();
  expect(a.active).toBe('browser');
  expect(a.level).toBeCloseTo(0.6, 3);
  await expect.poll(() => extentWith(1), { timeout: 3_000 }).toBeGreaterThan(0.6);
});

test('source « Aucune » ignores the browser: the look stops reacting', async () => {
  expect(await setSource('none')).toBe(200);
  await studio.post('/api/audio', { level: 0.9, bass: 1, beat: 5 });
  const a = await audio();
  expect(a.source).toBe('none');
  expect(a.active).toBe('none');
  expect(a.level).toBe(0);
  // size 1 at no bass: half the 0.5 extent.
  await expect.poll(() => extentWith(1), { timeout: 3_000 }).toBeCloseTo(0.25, 1);
});

test('source « Navigateur » works as before T-230', async () => {
  expect(await setSource('browser')).toBe(200);
  await expect.poll(() => extentWith(1), { timeout: 3_000 }).toBeCloseTo(0.75, 1);
  await expect.poll(() => extentWith(0), { timeout: 3_000 }).toBeCloseTo(0.25, 1);
  const a = await audio();
  expect(a.source).toBe('browser');
  expect(a.active).toBe('browser');
});

test('the config is validated, patched field by field and kept across a restart', async () => {
  expect(await studio.post('/api/audio/config', { source: 'spotify' })).toBe(400);
  expect(await studio.post('/api/audio/config', { buffer_frames: 64, device: 'Scarlett 2i2 USB' })).toBe(200);
  expect(await studio.get('/api/audio/config')).toEqual({ source: 'browser', device: 'Scarlett 2i2 USB', buffer_frames: 128 });
  await studio.restart();
  expect(await studio.get('/api/audio/config')).toEqual({ source: 'browser', device: 'Scarlett 2i2 USB', buffer_frames: 128 });
  expect((await audio()).state).toBe('disabled');
  expect(await studio.post('/api/audio/config', { device: null, buffer_frames: 256 })).toBe(200);
});
