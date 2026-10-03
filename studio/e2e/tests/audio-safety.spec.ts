// Audio reactivity safety (T-245): audio-driven flashes are capped
// (« Flashs max / s »), a cut or a silence brings every audio link back to
// neutral, the silence action (Garder / Look calme / Noir) works, and no
// audio event ever arms the laser. Preview only: --no-audio, the browser
// source is POST /api/audio (what the page's own analysis sends).
import { test, expect, useStudio, openUi, reveal, isLit, extent } from '../studio';

const studio = useStudio();

test.describe.configure({ mode: 'serial' });

interface Guard { live: boolean; silent: boolean; action: string; gain: number; flash_limited: boolean; max_flash_hz: number }
interface Strobe { active: boolean; fast: boolean; rate_hz: number }
interface GuardFrame { points: [number, number, number, number, number][]; armed: boolean; audio_guard: Guard; strobe: Strobe }
const frame = () => studio.get<GuardFrame>('/api/frame');
const config = () => studio.get('/api/audio/config');
const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

/** A kick on the master brightness: dark between hits, lit on each one. */
const KICK_FLASH = { source: 'kick', target: 'master.brightness', shape: { attack: { ms: 0 }, decay: { ms: 30 }, min: -1, max: 0 } };

/** Browser features every `everyMs` for `ms`, the beat (= kick) counter ticking each time. */
async function feed(ms: number, everyMs: number, level = 0.8) {
  const end = Date.now() + ms;
  let beat = 1;
  while (Date.now() < end) {
    await studio.post('/api/audio', { level, bass: level, beat: level > 0 ? beat++ : 0 });
    await sleep(everyMs);
  }
}

test.beforeEach(async () => {
  await studio.reset();
  await studio.post('/api/control', { id: 'audio.enabled', value: false });
  expect(await studio.post('/api/audio/routes', { mix: 0.5, routes: [] })).toBe(200);
  expect(await studio.post('/api/audio/config', { source: 'browser', safety: { max_flash_hz: 10, silence_action: 'keep' } })).toBe(200);
  await studio.post('/api/settings', { ...(await studio.state()).settings, brightness: 1 });
});

test('« Sécurité audio » sets the flash cap and the silence action', async ({ page }) => {
  expect((await config()).safety).toEqual({ max_flash_hz: 10, silence_action: 'keep' });
  await openUi(page, studio);
  await reveal(page, '#musSafety');
  await expect(page.locator('#musSilAct [data-act="keep"]')).toHaveClass(/active/);
  await expect(page.locator('#musFlashV')).toContainText('10');

  await page.locator('#musFlash').fill('3');
  await expect.poll(async () => (await config()).safety.max_flash_hz).toBe(3);
  await expect(page.locator('#musFlashV')).toContainText('3');

  await page.locator('#musSilAct [data-act="blackout"]').click();
  await expect.poll(async () => (await config()).safety.silence_action).toBe('blackout');
  await expect(page.locator('#musSilAct [data-act="blackout"]')).toHaveClass(/active/);

  // Look calme needs a saved scene: then it takes the first one.
  await page.locator('#musSilAct [data-act="calm_look"]').click();
  await expect(page.locator('#musGuard')).toContainText('Sauvez d\'abord une scène');
  expect((await config()).safety.silence_action).toBe('blackout');
  expect(await studio.post('/api/scenes/save', { name: 'Calme', duration_secs: 8 })).toBe(200);
  await page.reload();
  await reveal(page, '#musSafety');
  await page.locator('#musSilAct [data-act="calm_look"]').click();
  await expect.poll(async () => (await config()).safety.silence_action).toEqual({ calm_look: 'Calme' });
  await expect(page.locator('#musCalm')).toBeVisible();
  await expect(page.locator('#musCalm')).toHaveValue('Calme');
  // Never above 10 per second, whatever is asked.
  expect(await studio.post('/api/audio/config', { safety: { max_flash_hz: 40 } })).toBe(200);
  expect((await config()).safety.max_flash_hz).toBe(10);
  expect(await studio.post('/api/audio/config', { safety: { arm: true } })).toBe(400);
  expect((await frame()).armed).toBe(false);
});

test('kicks at 20 Hz on the brightness flash at most « Flashs max / s », and never arm', async () => {
  test.setTimeout(30_000);
  expect(await studio.post('/api/audio/routes', { mix: 0.5, routes: [KICK_FLASH] })).toBe(200);
  expect(await studio.post('/api/audio/config', { safety: { max_flash_hz: 3 } })).toBe(200);
  // The output's measured flash rate (the strobe limiter's meter, on the
  // frames themselves) while the browser source kicks 20 times a second.
  const rates: number[] = [];
  let limited = false, armed = false;
  const sample = async () => {
    const end = Date.now() + 3500;
    await sleep(1000); // let the rate meter fill
    while (Date.now() < end) {
      const f = await frame();
      rates.push(f.strobe.rate_hz);
      limited ||= f.audio_guard.flash_limited;
      armed ||= f.armed;
      await sleep(40);
    }
  };
  await Promise.all([feed(3500, 50), sample()]);
  expect(armed).toBe(false);
  expect(limited).toBe(true);
  expect(Math.max(...rates)).toBeGreaterThan(1.5); // it does flash...
  expect(Math.max(...rates)).toBeLessThanOrEqual(3.3); // ...at most 3 a second
});

test('an audio cut on a dark peak comes back to the operator\'s look', async () => {
  // Inverted link: the brightness is dark while the audio holds it down.
  const hold = { source: 'level', target: 'master.brightness', shape: { gate: 0, attack: { ms: 0 }, release: { ms: 150 }, min: 0, max: -1 } };
  expect(await studio.post('/api/audio/routes', { mix: 0.5, routes: [hold] })).toBe(200);
  await studio.post('/api/audio', { level: 1, bass: 1, beat: 0 });
  await expect.poll(async () => (await frame()).points.some(isLit), { timeout: 2000, intervals: [20] }).toBe(false);
  // The browser stops sending: stale after 500 ms, neutral 150 ms later.
  const cut = Date.now();
  await expect.poll(async () => (await frame()).points.some(isLit), { timeout: 3000, intervals: [20] }).toBe(true);
  expect(Date.now() - cut).toBeLessThan(1500);
  const f = await frame();
  expect(f.audio_guard.live).toBe(false);
  expect(f.armed).toBe(false);
});

test('silence: Noir fades the output out and back, Look calme swaps the look, the laser is never armed', async ({ page }) => {
  test.setTimeout(30_000);
  await openUi(page, studio);
  await reveal(page, '#musSafety');
  expect(await studio.post('/api/audio/config', { safety: { silence_action: 'blackout' } })).toBe(200);
  await feed(300, 50);
  expect((await frame()).points.some(isLit)).toBe(true);
  // Silent browser features (level 0) for over a second: black.
  const quiet = feed(2500, 100, 0);
  await expect.poll(async () => (await frame()).audio_guard.silent, { timeout: 2500, intervals: [50] }).toBe(true);
  await expect.poll(async () => (await frame()).points.some(isLit), { timeout: 1500, intervals: [50] }).toBe(false);
  await expect(page.locator('#musGuard')).toContainText('Silence');
  await quiet;
  let f = await frame();
  expect(f.armed).toBe(false);
  // Sound back: lit again.
  await feed(400, 50);
  await expect.poll(async () => (await frame()).points.some(isLit), { timeout: 1500, intervals: [50] }).toBe(true);

  // Look calme: a small circle saved as a scene stands in during the silence.
  const look = (await studio.state()).settings;
  await studio.post('/api/settings', { ...look, scale: 0.2 });
  expect(await studio.post('/api/scenes/save', { name: 'Petit', duration_secs: 8 })).toBe(200);
  await studio.post('/api/settings', look);
  expect(await studio.post('/api/audio/config', { safety: { silence_action: { calm_look: 'Petit' } } })).toBe(200);
  await feed(300, 50);
  await expect.poll(async () => extent((await frame()).points), { timeout: 1500 }).toBeCloseTo(0.5, 1);
  const quiet2 = feed(2500, 100, 0);
  await expect.poll(async () => extent((await frame()).points), { timeout: 2500, intervals: [50] }).toBeCloseTo(0.2, 1);
  await quiet2;
  await feed(300, 50);
  await expect.poll(async () => extent((await frame()).points), { timeout: 1500 }).toBeCloseTo(0.5, 1);
  f = await frame();
  expect(f.armed).toBe(false);
  // The look the operator set is untouched.
  expect((await studio.state()).settings.scale).toBe(look.scale);
});
