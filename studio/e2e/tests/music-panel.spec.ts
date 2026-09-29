// The « Musique » panel v2 (T-243), LIVE › Musique. Preview only and
// --no-audio: no microphone or interface is ever opened. The browser
// source is driven by POST /api/audio (what the page's analysis posts),
// the native one by --test-hooks' POST /api/test/native_audio (a simulated
// capture state, device list and analysis snapshot, fresh for 500 ms).
// The test never clicks « Écouter » with the Navigateur source (that
// would open the page's micro).
import { test, expect, useStudio, openUi, reveal, extent, type Studio } from '../studio';
import type { Page } from '@playwright/test';

const studio: Studio = useStudio({ testHooks: true });
const config = () => studio.get('/api/audio/config');

const SNAPSHOT = {
  features: {
    bands: { sub: 0.9, bass: 0.7, low_mid: 0.5, mid: 0.3, high: 0.1 }, bass: 1, level: 0.8,
    kick: 1, snare: 1, hat: 1, onset: 3, bpm: 128, bpm_confidence: 0.8, section: 'buildup', buildup: 0.4, drop: 1,
  },
  level_db: -18, peak_db: -6,
  // A peak in band 20 of 64.
  spectrum_db: Array.from({ length: 64 }, (_, i) => (i === 20 ? -3 : -60)),
  tempo_state: 'locked', section_since_s: 6,
};

/** Posts the native simulation every 100 ms for `ms`, the counters ticking with `tick`. */
async function feedNative(ms: number, extra: Record<string, unknown> = {}, tick = true) {
  const end = Date.now() + ms;
  let n = 1;
  while (Date.now() < end) {
    const f = { ...SNAPSHOT.features, ...(tick ? { kick: n, snare: n, hat: n } : {}) };
    expect(await studio.post('/api/test/native_audio', { state: 'running', device: 'Micro test', snapshot: { ...SNAPSHOT, features: f }, ...extra })).toBe(200);
    n++;
    await new Promise(r => setTimeout(r, 100));
  }
}

async function openMusic(page: Page) {
  await openUi(page, studio);
  await reveal(page, '#musPanel');
}

const bandHeight = (page: Page, band: string) =>
  page.locator(`#musBands [data-band="${band}"] .bar > div`).evaluate(el => parseFloat((el as HTMLElement).style.height));

let errors: string[] = [];
test.beforeEach(async ({ page }) => {
  errors = [];
  page.on('pageerror', e => errors.push(String(e)));
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  await studio.reset();
  expect(await studio.post('/api/audio/config', { source: 'browser', device: null, analysis: { auto_gain: true, manual_gain_db: 0, silence_db: -60, onsets: { delta: 0.1 } } })).toBe(200);
  expect(await studio.post('/api/test/native_audio', { state: 'disabled', devices: [] })).toBe(200);
});
test.afterEach(() => { expect(errors).toEqual([]); });

test('source choice: Navigateur by default, Native explains the macOS prompt and starts only on « Écouter »', async ({ page }) => {
  await studio.post('/api/test/native_audio', { devices: [{ name: 'Micro test', is_default: true }, { name: 'Carte son', is_default: false }] });
  await openMusic(page);
  await expect(page.locator('#musSrc [data-src="browser"]')).toHaveClass(/active/);
  await expect(page.locator('#micBtn')).toHaveText('Écouter');
  await expect(page.locator('#musNativeInfo')).toBeHidden();
  await expect(page.locator('#micSelect')).toBeVisible();

  await page.locator('#musSrc [data-src="native"]').click();
  await expect(page.locator('#musNativeInfo')).toBeVisible();
  await expect(page.locator('#musNativeInfo')).toContainText("macOS demande l'autorisation d'accès au micro");
  await expect(page.locator('#musNativeInfo')).toContainText('La source par défaut reste Navigateur');
  // Picking Native alone opens nothing.
  expect((await config()).source).toBe('browser');
  // The Mac's inputs, from the studio's list.
  await expect(page.locator('#musDevice')).toBeVisible();
  await expect(page.locator('#musDevice option')).toHaveText(['Entrée par défaut du Mac', 'Micro test (par défaut)', 'Carte son']);
  await page.locator('#musDevice').selectOption('Carte son');

  await page.locator('#micBtn').click();
  await expect.poll(async () => (await config()).source).toBe('native');
  expect((await config()).device).toBe('Carte son');
  await expect(page.locator('#micBtn')).toHaveText('Arrêter');
  // --no-audio: said plainly.
  await expect(page.locator('#musStatus')).toContainText('--no-audio');

  // Arrêter: back to the default source, Native still picked for next time.
  await page.locator('#micBtn').click();
  await expect.poll(async () => (await config()).source).toBe('browser');
  await expect(page.locator('#musSrc [data-src="native"]')).toHaveClass(/active/);
  await expect(page.locator('#micBtn')).toHaveText('Écouter');

  await page.locator('#musSrc [data-src="none"]').click();
  await expect.poll(async () => (await config()).source).toBe('none');
  await expect(page.locator('#micBtn')).toBeHidden();
  await page.locator('#musSrc [data-src="browser"]').click();
  await expect.poll(async () => (await config()).source).toBe('browser');

  // A change made elsewhere is followed.
  await studio.post('/api/audio/config', { source: 'native' });
  await expect(page.locator('#musSrc [data-src="native"]')).toHaveClass(/active/);
  await expect(page.locator('#micBtn')).toHaveText('Arrêter');
  expect((await studio.state()).armed).toBe(false);
});

test('meters and lights follow the browser source (simulated POST /api/audio)', async ({ page }) => {
  await openMusic(page);
  const body = (beat: number) => ({ level: 0.6, bass: 0.5, beat, level_db: -12, bands: { sub: 0.2, bass: 0.9, low_mid: 0.4, mid: 0.6, high: 0.3 }, section: 'break', bpm: 124, bpm_confidence: 0.5 });
  let beat = 1;
  const feeding = (async () => { for (const end = Date.now() + 2500; Date.now() < end; beat++) { await studio.post('/api/audio', body(beat)); await new Promise(r => setTimeout(r, 150)); } })();
  await expect(page.locator('#musDb')).toHaveText('−12.0 dBFS');
  await expect.poll(() => bandHeight(page, 'bass')).toBe(90);
  await expect.poll(() => bandHeight(page, 'mid')).toBe(60);
  await expect.poll(() => bandHeight(page, 'sub')).toBe(20);
  await expect(page.locator('#musSection')).toHaveText('Break');
  await expect(page.locator('#musBpm')).toHaveText('124.0');
  await expect(page.locator('#musConfV')).toHaveText('50 %');
  await expect(page.locator('#musTempoState')).toHaveText('estimation native seulement');
  // The browser's beat is its kick.
  await expect(page.locator('#lKick')).toHaveClass(/hit/);
  await feeding;
  // No native capture: no spectrum drawn, and said.
  await expect(page.locator('#musSpectrum')).toHaveAttribute('data-bands', '0');
});

test('native snapshot: dBFS meter, bands, 64-band spectrum, lights, detected BPM and section', async ({ page }) => {
  await studio.post('/api/audio/config', { source: 'native' });
  await openMusic(page);
  const feeding = feedNative(3000);
  await expect(page.locator('#musDb')).toHaveText('−18.0 dBFS');
  await expect.poll(() => bandHeight(page, 'sub')).toBe(90);
  await expect.poll(() => bandHeight(page, 'high')).toBe(10);
  await expect(page.locator('#musSpectrum')).toHaveAttribute('data-bands', '64');
  await expect(page.locator('#musSpectrum')).toHaveAttribute('data-peak', '20');
  for (const id of ['#lKick', '#lSnare', '#lHat']) await expect(page.locator(id)).toHaveClass(/hit/);
  await expect(page.locator('#musBpm')).toHaveText('128.0');
  await expect(page.locator('#musConfV')).toHaveText('80 %');
  await expect(page.locator('#musTempoState')).toHaveText('Verrouillé');
  await expect(page.locator('#musSection')).toHaveText('Montée · 6 s');
  await expect(page.locator('#musBuildupV')).toHaveText('40 %');
  await expect(page.locator('#musDrops')).toHaveText('1');
  await expect(page.locator('#musStatus')).toContainText('Capture de « Micro test »');
  await expect(page.locator('#musSilentHint')).toBeHidden();
  await feeding;
});

test('a running but exactly silent native capture hints at a refused permission', async ({ page }) => {
  await studio.post('/api/audio/config', { source: 'native' });
  await openMusic(page);
  const silent = { ...SNAPSHOT, level_db: -120, peak_db: -120 };
  const end = Date.now() + 3500;
  const feeding = (async () => { while (Date.now() < end) { await studio.post('/api/test/native_audio', { state: 'running', snapshot: silent }); await new Promise(r => setTimeout(r, 100)); } })();
  await expect(page.locator('#musDb')).toHaveText('−∞ dBFS');
  // Not before 2 s of it.
  await expect(page.locator('#musSilentHint')).toBeHidden();
  await expect(page.locator('#musSilentHint')).toBeVisible({ timeout: 3000 });
  await expect(page.locator('#musSilentHint')).toContainText('Confidentialité et sécurité › Microphone');
  await expect(page.locator('#musSilentHint')).toContainText('tccutil reset Microphone');
  await feeding;
  // Sound again: gone.
  await feedNative(600);
  await expect(page.locator('#musSilentHint')).toBeHidden();
  // An explicit refusal says so at once.
  await studio.post('/api/test/native_audio', { state: 'permission_denied', message: 'accès au micro refusé' });
  await expect(page.locator('#musSilentHint')).toBeVisible();
  await expect(page.locator('#musSilentHint')).toContainText('Accès au micro refusé par macOS');
});

test('analysis settings: auto-gain, manual gain, silence threshold, onset sensitivity; « Nouveau morceau »', async ({ page }) => {
  await openMusic(page);
  await page.locator('#musSrc [data-src="native"]').click();
  await page.locator('#musSettings summary').click();
  await expect(page.locator('#musAutoGain')).toBeChecked();
  await expect(page.locator('#musGain')).toBeDisabled();
  await expect(page.locator('#musSensV')).toHaveText('48 %'); // delta 0.1

  await page.locator('#musAutoGain').uncheck();
  await expect.poll(async () => (await config()).analysis.auto_gain).toBe(false);
  await expect(page.locator('#musGain')).toBeEnabled();
  await page.locator('#musGain').fill('12');
  await page.locator('#musGain').dispatchEvent('change');
  await expect.poll(async () => (await config()).analysis.manual_gain_db).toBe(12);
  await expect(page.locator('#musGainV')).toHaveText('+12 dB');
  await page.locator('#musSilence').fill('-50');
  await page.locator('#musSilence').dispatchEvent('change');
  await expect.poll(async () => (await config()).analysis.silence_db).toBe(-50);
  await page.locator('#musSens').fill('100');
  await page.locator('#musSens').dispatchEvent('change');
  await expect.poll(async () => (await config()).analysis.onsets.delta).toBeCloseTo(0.03, 4);
  // Settings never open the input.
  expect((await config()).source).toBe('browser');

  const req = page.waitForRequest(r => r.url().endsWith('/api/control') && r.method() === 'POST' && r.postData()!.includes('tempo.new_track'));
  await page.locator('#musNewTrack').click();
  expect((await req).postDataJSON()).toEqual({ id: 'tempo.new_track' });
});

test('panel hidden: the laser keeps reacting, no polling; shown again: the display resumes', async ({ page }) => {
  await studio.post('/api/control', { id: 'audio.enabled', value: true });
  await studio.post('/api/control', { id: 'audio.size', value: 1 });
  await studio.post('/api/audio/config', { source: 'native' });
  await openMusic(page);
  const polls: string[] = [];
  page.on('request', r => { if (/\/api\/audio\/(state|spectrum)/.test(r.url())) polls.push(r.url()); });

  // Visible, native, spectrum on: under 30 requests a second.
  polls.length = 0;
  await feedNative(2000);
  expect(polls.length).toBeGreaterThan(20);
  expect(polls.length).toBeLessThanOrEqual(62);

  // Another LIVE tab, then the whole page hidden: no more polls.
  await page.locator('#liveTabs [data-panel-tab="direct"]').click();
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await page.waitForTimeout(400);
  polls.length = 0;
  // The engine reacts to the native snapshot (bass 1 → large), then to silence.
  const feeding = feedNative(1500);
  await expect.poll(async () => extent((await studio.frame()).points)).toBeGreaterThan(0.7);
  await feeding;
  expect(polls).toEqual([]);
  await expect.poll(async () => extent((await studio.frame()).points), { timeout: 3000 }).toBeLessThan(0.3);

  // Back: the display follows new values.
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await page.locator('#liveTabs [data-panel-tab="music"]').click();
  const again = feedNative(1500, {}, true);
  await expect(page.locator('#musDb')).toHaveText('−18.0 dBFS');
  await expect(page.locator('#musSpectrum')).toHaveAttribute('data-bands', '64');
  await again;
  expect((await studio.state()).armed).toBe(false);
});

test('layout at 1280 px: the panel fits, nothing overflows', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openMusic(page);
  await page.locator('#musSrc [data-src="native"]').click();
  await page.locator('#musSettings summary').click();
  const over = await page.evaluate(() => {
    const p = document.getElementById('musPanel')!;
    const r = p.getBoundingClientRect();
    const out = [...p.querySelectorAll('*')].filter(el => {
      const b = el.getBoundingClientRect();
      return b.width > 0 && (b.right > r.right + 1 || b.left < r.left - 1);
    }).map(el => el.id || el.tagName);
    return { out, pageScroll: document.documentElement.scrollWidth > window.innerWidth };
  });
  expect(over).toEqual({ out: [], pageScroll: false });
  for (const id of ['#musDbMeter', '#musBands', '#musSpectrum', '#lKick', '#musBpm', '#musSection']) await expect(page.locator(id)).toBeVisible();
});
