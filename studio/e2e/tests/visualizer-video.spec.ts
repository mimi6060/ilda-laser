// Vidéo de la vue 3D (T-459): the page records its own 3D view with
// MediaRecorder, through a composition canvas that carries the
// « Simulation » watermark, and offers the file to the browser. No route,
// no request, nothing changes on the studio side (arming, output).
import { test, expect, useStudio, openUi } from '../studio';
import type { Page, Request } from '@playwright/test';
import { readFileSync } from 'node:fs';

const studio = useStudio();

/** Console errors, uncaught exceptions, and every request the page makes. */
function watch(page: Page) {
  const errors: string[] = [], requests: Request[] = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push(String(e)));
  page.on('request', r => { requests.push(r); });
  return { errors, requests };
}

/** Requests from index `from` on that are not the page's usual reads. */
function unusual(requests: Request[], from: number, base: string) {
  return requests.slice(from).filter(r => {
    const u = r.url();
    if (u.startsWith('blob:' + base + '/')) return false; // the heartbeat worker and the video itself
    if (!u.startsWith(base + '/')) return true; // anything out of the studio
    if (new URL(u).pathname === '/api/heartbeat') return false;
    return r.method() !== 'GET';
  }).map(r => r.method() + ' ' + r.url());
}

async function show3d(page: Page) {
  await page.locator('#viewSeg [data-view="3d"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#vid3d')).toBeVisible();
  await expect(page.locator('#vidBtn')).toBeEnabled();
}

async function setPrefs(page: Page, seconds: string, size: string) {
  await page.locator('#vidOptBtn').click();
  await expect(page.locator('#vidMenu')).toBeVisible();
  await page.locator('#vidSeconds').selectOption(seconds);
  await page.locator('#vidSize').selectOption(size);
  // The show's song is the studio's (played on its own output): greyed.
  await expect(page.locator('#vidAudio')).toBeDisabled();
  await page.locator('#vidOptBtn').click();
  await expect(page.locator('#vidMenu')).toBeHidden();
}

test.beforeEach(async () => {
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
});

test('records n seconds of the 3D view, watermarked, without a request or a change on the studio', async ({ page }) => {
  const seen = watch(page);
  await openUi(page, studio);
  await show3d(page);
  expect(await page.evaluate(() => (window as any).__rec.format())).toMatch(/^video\/(webm|mp4)/);
  const presets = await studio.presets();
  const fan = presets.presets.find(p => p.name.startsWith('Éventail'))!;
  await studio.post('/api/cue', { id: fan.id });
  await setPrefs(page, '5', '720p');
  expect(JSON.parse(await page.evaluate(() => localStorage.getItem('laserStudio.record3d') || '{}'))).toEqual({ seconds: 5, size: '720p', audio: false });

  const before = await studio.frame();
  expect(before.armed).toBe(false);
  // Let the page's own preference save (3D tab) go out first.
  await page.waitForTimeout(800);
  const from = seen.requests.length;
  const download = page.waitForEvent('download', { timeout: 15_000 });
  await page.locator('#vidBtn').click();
  await expect(page.locator('#vidBtn')).toHaveClass(/recording/);
  await expect(page.locator('#vidBtn')).toHaveText(/^■ Arrêter — 00:0[0-5]$/);
  await expect(page.locator('#vidBtn')).toHaveAttribute('aria-pressed', 'true');
  expect((await studio.frame()).armed).toBe(false);

  // The watermark is in the recorded picture: light pixels in the bottom
  // right corner of the composition canvas (the 3D view itself stays dark there).
  const corner = await page.evaluate(() => {
    const cv = (window as any).__rec.canvas() as HTMLCanvasElement;
    const W = cv.width, H = cv.height, w = Math.round(W * 0.35), h = Math.round(H * 0.12);
    const d = cv.getContext('2d')!.getImageData(W - w, H - h, w, h).data;
    let white = 0;
    for (let i = 0; i < d.length; i += 4) if (d[i] > 200 && d[i + 1] > 200 && d[i + 2] > 200) white++;
    return { W, H, white };
  });
  expect(corner.white).toBeGreaterThan(50);
  // The beams are in it too: lit pixels elsewhere.
  await expect.poll(() => page.evaluate(() => {
    const cv = (window as any).__rec.canvas() as HTMLCanvasElement;
    const d = cv.getContext('2d')!.getImageData(0, 0, cv.width, Math.round(cv.height * 0.8)).data;
    let lit = 0;
    for (let i = 0; i < d.length; i += 16) if (d[i] + d[i + 1] + d[i + 2] > 60) lit++;
    return lit;
  })).toBeGreaterThan(50);

  const file = await download;
  expect(file.suggestedFilename()).toMatch(/^laser-studio-simulation-\d{8}-\d{6}\.(webm|mp4)$/);
  const bytes = readFileSync((await file.path())!);
  expect(bytes.length).toBeGreaterThan(0);
  await expect(page.locator('#vidStatus')).toHaveText(/^Vidéo prête \(\d+,\d Mo\)$/);
  await expect(page.locator('#vidBtn')).toHaveText('● Enregistrer');
  const last = await page.evaluate(() => (window as any).__rec.last());
  expect(last.why).toBe('fin');
  expect(Math.abs(last.seconds - 5)).toBeLessThanOrEqual(0.5);
  expect(last.bytes).toBe(bytes.length);

  // Nothing but the usual reads while recording; the studio untouched.
  expect(unusual(seen.requests, from, studio.url)).toEqual([]);
  const after = await studio.frame();
  expect(after.armed).toBe(false);
  expect(after.output).toBe(before.output);
  expect(after.active_cue).toBe(before.active_cue);
  // The view is drawn at its own size again.
  await expect(page.locator('#beams')).not.toHaveClass(/rec-fixed/);
  expect(seen.errors).toEqual([]);
});

test('leaving the 3D view or the workspace ends the recording and offers the file', async ({ page }) => {
  const seen = watch(page);
  await openUi(page, studio);
  await show3d(page);
  await setPrefs(page, '0', 'vue');

  let download = page.waitForEvent('download', { timeout: 15_000 });
  await page.locator('#vidBtn').click();
  await expect(page.locator('#vidBtn')).toHaveText(/^■ Arrêter — 0[45]:\d\d$/); // 5 minutes at most
  await page.waitForTimeout(1500);
  await page.locator('#viewSeg [data-view="2d"]').click();
  expect((await download).suggestedFilename()).toMatch(/\.(webm|mp4)$/);
  await expect(page.locator('#vidStatus')).toContainText('(vue 3D quittée)');
  await expect(page.locator('#vid3d')).toBeHidden();
  expect(await page.evaluate(() => (window as any).__rec.recording())).toBe(false);

  await show3d(page);
  download = page.waitForEvent('download', { timeout: 15_000 });
  await page.locator('#vidBtn').click();
  await page.waitForTimeout(1500);
  await page.locator('#wsTabs [data-ws-tab="timeline"]').click();
  await download;
  await page.locator('#wsTabs [data-ws-tab="live"]').click();
  await expect(page.locator('#vidStatus')).toContainText('(espace de travail changé)');
  expect(seen.errors).toEqual([]);
});

test('Alt+R starts and stops in LIVE; 1920×1080 draws the 3D view at that size', async ({ page }) => {
  const seen = watch(page);
  await openUi(page, studio);
  await show3d(page);
  await setPrefs(page, '60', '1080p');
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  const download = page.waitForEvent('download', { timeout: 15_000 });
  await page.keyboard.press('Alt+KeyR');
  await expect(page.locator('#vidBtn')).toHaveClass(/recording/);
  await expect(page.locator('#beams')).toHaveClass(/rec-fixed/);
  const sizes = await page.evaluate(() => {
    const cv = (window as any).__rec.canvas() as HTMLCanvasElement;
    return [cv.width, cv.height];
  });
  expect(sizes).toEqual([1920, 1080]);
  await expect.poll(() => page.evaluate('[view3d.canvas.width, view3d.canvas.height]')).toEqual([1920, 1080]);
  // No option removes the watermark: the menu holds the duration, the size and the sound only.
  expect(await page.locator('#vidMenu input, #vidMenu select').evaluateAll(els => els.map(e => e.id))).toEqual(['vidSeconds', 'vidSize', 'vidAudio']);
  await page.waitForTimeout(1000);
  await page.keyboard.press('Alt+KeyR');
  await download;
  await expect(page.locator('#vidBtn')).not.toHaveClass(/recording/);
  await expect(page.locator('#beams')).not.toHaveClass(/rec-fixed/);
  await expect.poll(() => page.evaluate('view3d.canvas.width')).not.toBe(1920);
  // R alone stays a cue key: it never records.
  expect((await studio.frame()).armed).toBe(false);
  expect(seen.errors).toEqual([]);
});

// T-516: at 1024×768 in 2D+3D the prepared pane hides the 3D canvas. A
// recording then ends with its file and a visible toast naming the
// reason (the status line sits in the hidden #vid3d), and Alt+R is
// refused with a message while the canvas is hidden.
test('the prepared pane hiding the 3D view ends the recording with a visible reason; Alt+R is refused meanwhile', async ({ page }) => {
  const seen = watch(page);
  await page.setViewportSize({ width: 1024, height: 768 });
  await openUi(page, studio);
  await page.locator('#viewSeg [data-view="both"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#vidBtn')).toBeEnabled();
  await setPrefs(page, '0', 'vue');
  const from = seen.requests.length;
  const download = page.waitForEvent('download', { timeout: 15_000 });
  await page.locator('#vidBtn').click();
  await expect(page.locator('#vidBtn')).toHaveClass(/recording/);
  await page.waitForTimeout(1500);
  await page.locator('#prepBtn').click();
  expect((await download).suggestedFilename()).toMatch(/\.(webm|mp4)$/);
  expect(await page.evaluate(() => (window as any).__rec.recording())).toBe(false);
  expect(await page.evaluate(() => (window as any).__rec.last().why)).toBe('masquee');
  await expect(page.locator('#beams')).toBeHidden();
  await expect(page.locator('#vid3d')).toBeHidden();
  const stopped = page.locator('#toasts .toast', { hasText: 'Enregistrement 3D arrêté' });
  await expect(stopped).toBeVisible();
  await expect(stopped).toContainText('vue 3D masquée par le panneau Préparé');
  await expect(stopped).toContainText(/\d Mo/);
  // Alt+R while the canvas is hidden: nothing records, a message says why.
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('Alt+KeyR');
  await expect(page.locator('#toasts .toast', { hasText: 'Enregistrement 3D impossible' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__rec.recording())).toBe(false);
  await expect(page.locator('#vid3d')).toBeHidden();
  // « Préparer » off (nothing prepared): the 3D view and the button are back.
  await page.locator('#prepBtn').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#vid3d')).toBeVisible();
  await expect(page.locator('#vidBtn')).toBeEnabled();
  expect(seen.errors).toEqual([]);
  // The recording, its stop and the refusal make no request (« Préparer » is this page's mode).
  expect(unusual(seen.requests, from, studio.url)).toEqual([]);
});

test('without MediaRecorder the button is greyed with its reason, no console error', async ({ page }) => {
  await page.addInitScript(() => { delete (window as any).MediaRecorder; });
  const seen = watch(page);
  await openUi(page, studio);
  await page.locator('#viewSeg [data-view="3d"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#vid3d')).toBeVisible();
  await expect(page.locator('#vidBtn')).toBeDisabled();
  await expect(page.locator('#vidBtn')).toHaveAttribute('title', 'Enregistrement vidéo indisponible sur ce navigateur');
  await expect(page.locator('#vidOptBtn')).toBeDisabled();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('Alt+KeyR');
  expect(await page.evaluate(() => (window as any).__rec.recording())).toBe(false);
  expect(seen.errors).toEqual([]);
});
