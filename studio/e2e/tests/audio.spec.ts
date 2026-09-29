// Audio input source (T-230), analysis settings (T-231), onset settings (T-232), the tempo estimate (T-233), the engine's features v2 (T-237) and the section detector (T-236), through the API only. The test studio runs
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
  // Break / build-up / drop detection (T-236): native only.
  expect(a.sections).toBeNull();
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
  // The last values fall to neutral (T-237: a release, not a jump).
  await expect.poll(async () => (await audio()).level, { timeout: 3_000 }).toBeLessThan(0.01);
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

test('AudioFeatures v2: the new POST /api/audio format reaches /api/state.audio', async () => {
  expect(await setSource('browser')).toBe(200);
  const bands = { sub: 0.2, bass: 0.9, low_mid: 0.4, mid: 0.6, high: 0.3 };
  expect(await studio.post('/api/audio', {
    level: 0.5, bass: 0.9, beat: 12, level_db: -14, bands, onset: 40, kick: 12, snare: 6, hat: 30,
    kick_strength: 0.8, silent: false, bpm: 126, bpm_confidence: 0.75, section: 'buildup', buildup: 0.4, drop: 2,
  })).toBe(200);
  const a = await audio();
  expect(a.active).toBe('browser');
  for (const [k, v] of Object.entries(bands)) expect(a.bands[k]).toBeCloseTo(v, 5);
  expect(a.counters).toEqual({ beat: 12, onset: 40, kick: 12, snare: 6, hat: 30, drop: 2 });
  expect(a.section).toBe('buildup');
  expect(a.buildup).toBeCloseTo(0.4, 5);
  expect(a.detected_bpm).toBe(126);
  expect(a.detected_confidence).toBeCloseTo(0.75, 5);
  expect(a.signals.mid).toBeCloseTo(0.6, 5);
  expect(a.features.level_db).toBe(-14);
  expect(a.level).toBeCloseTo(0.5, 5);
  expect(a.bass).toBeCloseTo(0.9, 5);
  expect(a.beat).toBe(12);
  // Out-of-range values are bounded, wrong types refused.
  expect(await studio.post('/api/audio', { level: 3, bass: 0.1, beat: 12, bands: { high: 9, sub: -1 } })).toBe(200);
  const b = await audio();
  expect(b.level).toBe(1);
  for (const v of Object.values(b.bands) as number[]) expect(v >= 0 && v <= 1).toBe(true);
  expect(await studio.post('/api/audio', { level: 'fort' })).toBe(400);
  expect(await studio.post('/api/audio', { section: 'refrain' })).toBe(400);
});

test('AudioFeatures v2: the old format is filled in, and stale audio falls to neutral', async () => {
  expect(await setSource('browser')).toBe(200);
  expect(await studio.post('/api/audio', { level: 0.6, bass: 0.7, beat: 21 })).toBe(200);
  const a = await audio();
  expect(a.bands.bass).toBeCloseTo(0.7, 5);
  expect(a.bands.sub).toBeCloseTo(0.7, 5);
  expect(a.bands.mid).toBe(0);
  expect(a.counters.kick).toBe(21);
  expect(a.silent).toBe(false);
  expect(a.features.level_db).toBeCloseTo(-20, 1);
  // The page stops posting: within about a second everything is neutral,
  // the counters are kept (going stale is never a beat).
  await expect.poll(async () => (await audio()).bass, { timeout: 3_000, intervals: [100] }).toBeLessThan(0.01);
  const b = await audio();
  expect(b.active).toBe('none');
  expect(b.silent).toBe(true);
  expect(b.section).toBe('silence');
  expect(b.counters.beat).toBe(21);
  for (const v of Object.values(b.bands) as number[]) expect(v).toBeLessThan(0.01);
});

test('GET /api/audio/spectrum: 64 bands, null without a native capture', async () => {
  const s = await studio.get('/api/audio/spectrum');
  expect(s).toEqual({ bands: 64, lo_hz: 20, hi_hz: 20000, t: null, db: null, values: null });
});
