// Audio input source (T-230), analysis settings (T-231), onset settings (T-232) and the tempo estimate (T-233), through the API only. The test studio runs
// with --no-audio: no microphone or interface is ever opened, the native
// capture reports « disabled », and the browser source (POST /api/audio)
// keeps driving the looks as before.
import { test, expect, useStudio, extent } from '../studio';

const studio = useStudio();
const audio = async () => (await studio.state()).audio;
const setSource = (source: string) => studio.post('/api/audio/config', { source });
const ONSETS = { delta: 0.1, lookahead_hops: 1, kick_refractory_ms: 100 };
const ANALYSIS = { auto_gain: true, manual_gain_db: 0, silence_db: -60, onsets: ONSETS };

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
  expect(a.spectral).toBeNull();
  expect(a.onsets).toBeNull();
  expect(a.tempo).toBeNull();
  // Default source is the browser: the studio never opens the Mac's mic by itself.
  expect(await studio.get('/api/audio/config')).toEqual({ source: 'browser', device: null, buffer_frames: 256, analysis: ANALYSIS });
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
  expect(await studio.get('/api/audio/config')).toEqual({ source: 'browser', device: 'Scarlett 2i2 USB', buffer_frames: 128, analysis: ANALYSIS });
  await studio.restart();
  expect(await studio.get('/api/audio/config')).toEqual({ source: 'browser', device: 'Scarlett 2i2 USB', buffer_frames: 128, analysis: ANALYSIS });
  expect((await audio()).state).toBe('disabled');
  expect(await studio.post('/api/audio/config', { device: null, buffer_frames: 256 })).toBe(200);
});

test('the band analysis settings are patched field by field and kept across a restart', async () => {
  expect(await studio.post('/api/audio/config', { analysis: { auto_gain: 'oui' } })).toBe(400);
  expect(await studio.post('/api/audio/config', { analysis: { gain: 3 } })).toBe(400);
  expect(await studio.post('/api/audio/config', { analysis: { auto_gain: false, manual_gain_db: 99 } })).toBe(200);
  const kept = { auto_gain: false, manual_gain_db: 40, silence_db: -60, onsets: ONSETS };
  expect((await studio.get('/api/audio/config')).analysis).toEqual(kept);
  expect(await studio.post('/api/audio/config', { analysis: { silence_db: -45 } })).toBe(200);
  await studio.restart();
  const c = await studio.get('/api/audio/config');
  expect(c.analysis).toEqual({ ...kept, silence_db: -45 });
  expect(c.source).toBe('browser');
  expect(await studio.post('/api/audio/config', { analysis: ANALYSIS })).toBe(200);
});

test('the onset settings are patched field by field, validated and kept across a restart', async () => {
  expect(await studio.post('/api/audio/config', { analysis: { onsets: { sensitivity: 1 } } })).toBe(400);
  expect(await studio.post('/api/audio/config', { analysis: { onsets: 3 } })).toBe(400);
  expect(await studio.post('/api/audio/config', { analysis: { onsets: { delta: 'x' } } })).toBe(400);
  expect(await studio.post('/api/audio/config', { analysis: { onsets: { delta: 0.25 } } })).toBe(200);
  expect(await studio.post('/api/audio/config', { analysis: { onsets: { lookahead_hops: 9, kick_refractory_ms: 5 } } })).toBe(200);
  const kept = { delta: 0.25, lookahead_hops: 2, kick_refractory_ms: 30 };
  expect((await studio.get('/api/audio/config')).analysis.onsets).toEqual(kept);
  await studio.restart();
  const c = await studio.get('/api/audio/config');
  expect(c.analysis).toEqual({ ...ANALYSIS, onsets: kept });
  expect((await audio()).onsets).toBeNull();
  expect(await studio.post('/api/audio/config', { analysis: ANALYSIS })).toBe(200);
  expect((await studio.get('/api/audio/config')).analysis).toEqual(ANALYSIS);
});

test('« Nouveau morceau » is accepted without a capture and changes nothing else', async () => {
  const before = await studio.state();
  expect(await studio.post('/api/audio/tempo/new_track', {})).toBe(200);
  const after = await studio.state();
  expect(after.audio.tempo).toBeNull();
  // The detector only proposes: the tempo clock is untouched.
  expect(after.tempo.bpm).toBe(before.tempo.bpm);
  expect(after.tempo.source).toBe(before.tempo.source);
});
