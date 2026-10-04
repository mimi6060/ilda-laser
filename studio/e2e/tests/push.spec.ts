// Pushed state and binary frames (T-411): the page follows the studio
// through GET /api/events and GET /api/frame.bin instead of polling
// /api/frame, and falls back to polling when the stream is unavailable.
// Preview only: no output is ever armed with light.
import type { Page, Request } from '@playwright/test';
import { test, expect, useStudio, openUi, reveal } from '../studio';

const studio = useStudio();

test.beforeEach(async () => {
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/control', { id: 'master.size', value: 1 });
});

const linkTitle = (page: Page) => page.locator('#engineLed');
const isJsonFrame = (r: Request) => new URL(r.url()).pathname === '/api/frame';

test('a change made in one page shows in another within 100 ms, without polling', async ({ page, context }) => {
  const other = await context.newPage();
  await openUi(page, studio);
  await openUi(other, studio);
  for (const p of [page, other]) await expect(linkTitle(p)).toHaveAttribute('title', /Liaison : flux/);
  const polled: string[] = [];
  const frames: string[] = [];
  for (const p of [page, other]) {
    p.on('request', r => { if (isJsonFrame(r)) polled.push(r.url()); if (r.url().includes('/api/frame.bin')) frames.push(r.url()); });
  }
  await reveal(page, '#mSize');
  await reveal(other, '#mSize');
  // When « Taille maître » changes in the second page (its value label).
  await other.evaluate(() => {
    const w = window as unknown as { seenAt?: number };
    const label = document.getElementById('mSizeV')!;
    new MutationObserver(() => { if (label.textContent === '37 %' && !w.seenAt) w.seenAt = Date.now(); })
      .observe(label, { childList: true, characterData: true, subtree: true });
  });
  const movedAt = await page.evaluate(() => {
    const el = document.getElementById('mSize') as HTMLInputElement;
    el.value = '37';
    const t = Date.now();
    el.dispatchEvent(new Event('input', { bubbles: true }));
    return t;
  });
  await expect.poll(() => other.evaluate(() => (window as unknown as { seenAt?: number }).seenAt ?? 0)).toBeGreaterThan(0);
  const seenAt = await other.evaluate(() => (window as unknown as { seenAt: number }).seenAt);
  expect(seenAt - movedAt, 'the second page followed after').toBeLessThan(100);
  await expect(other.locator('#mSize')).toHaveValue('37');
  expect((await studio.live()).size).toBeCloseTo(0.37, 3);
  // The preview kept running on binary frames, and nobody polled.
  await other.waitForTimeout(300);
  expect(frames.length).toBeGreaterThan(0);
  expect(polled).toEqual([]);
  await expect(other.locator('#stPoints')).not.toHaveText('0');
  await other.close();
});

test('the laser button follows an arm made elsewhere, from the frames', async ({ page }) => {
  await openUi(page, studio);
  await expect(linkTitle(page)).toHaveAttribute('title', /Liaison : flux/);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  expect(await studio.post('/api/arm', { on: true, source: 'ui' })).toBe(200);
  await expect(page.locator('#armBtn')).toHaveText('LASER ON', { timeout: 500 });
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF', { timeout: 500 });
});

test('without the stream the page falls back to polling and still follows', async ({ page }) => {
  await page.route('**/api/events', route => route.abort());
  const polled: string[] = [];
  page.on('request', r => { if (isJsonFrame(r)) polled.push(r.url()); });
  await openUi(page, studio);
  await expect(linkTitle(page)).toHaveAttribute('title', /Liaison : interrogation \(secours\)/);
  expect(polled.length).toBeGreaterThan(0);
  await reveal(page, '#mSize');
  await studio.post('/api/control', { id: 'master.size', value: 0.6 });
  await expect(page.locator('#mSize')).toHaveValue('60');
  await expect(page.locator('#linkLost')).toBeHidden();
});

test('an open stream is not the operator: without a page, the studio stays « absent »', async () => {
  const ctrl = new AbortController();
  const res = await fetch(studio.url + '/api/events', { signal: ctrl.signal });
  expect(res.status).toBe(200);
  expect(res.headers.get('content-type')).toContain('text/event-stream');
  const reader = res.body!.getReader();
  const first = new TextDecoder().decode((await reader.read()).value);
  expect(first).toContain('retry:');
  // No page beats (the earlier tests' pages are closed): arming stays
  // refused, whatever the stream.
  await expect.poll(async () => (await studio.get<{ status: { ui_alive: boolean } }>('/api/presence')).status.ui_alive, { timeout: 5000 }).toBe(false);
  expect(await studio.post('/api/arm', { on: true, source: 'ui' })).toBe(409);
  ctrl.abort();
});
