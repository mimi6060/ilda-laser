// « La plus récente gagne » (T-484): a control moved continuously keeps at
// most one request in flight, values posted meanwhile replace each other,
// and the newest always reaches the studio last, even when an older
// request is slow. Hold buttons and stops are not coalesced.
import { test, expect, useStudio, openUi } from '../studio';
import type { Page, Route } from '@playwright/test';

const studio = useStudio();

// Holds /api/control requests back (the first one longest, as on a slow
// link) and records what left and how many were in flight at once.
async function slowControl(page: Page) {
  const seen = { values: [] as unknown[], inflight: 0, maxInflight: 0 };
  let n = 0;
  await page.route('**/api/control', async (route: Route) => {
    const body = route.request().postDataJSON();
    seen.values.push(body.value ?? body.norm);
    seen.inflight++; seen.maxInflight = Math.max(seen.maxInflight, seen.inflight);
    await new Promise(r => setTimeout(r, n++ === 0 ? 400 : 20 + Math.random() * 60));
    try { await route.continue(); } finally { seen.inflight--; }
  });
  return seen;
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  await openUi(page, studio);
});

test('a burst of values on one control: one in flight, the last one wins', async ({ page }) => {
  const seen = await slowControl(page);
  const results = await page.evaluate(async () => {
    const w = window as unknown as { sendLatest: (k: string, m: string, p: string, b: unknown) => Promise<Response | null> };
    const sends = [];
    for (let i = 1; i <= 20; i++) sends.push(w.sendLatest('t484', 'POST', '/api/control', { id: 'master.size', value: i / 10 }));
    return (await Promise.all(sends)).map(r => (r === null ? null : r.status));
  });
  // The first left at once, the last after it; all in between were replaced.
  expect(results[0]).toBe(200);
  expect(results[19]).toBe(200);
  expect(results.slice(1, 19).every(r => r === null)).toBe(true);
  expect(seen.values).toEqual([0.1, 2]);
  expect(seen.maxInflight).toBe(1);
  await expect.poll(async () => (await studio.live()).size).toBe(2);
});

test('dragging a layer dimmer over a slow link ends on the released value', async ({ page }) => {
  const seen = await slowControl(page);
  const range = page.locator('#layerDim1');
  await range.scrollIntoViewIfNeeded();
  // Many input events, as a drag gives, ending at 25 %.
  await range.evaluate(async (el: HTMLInputElement) => {
    for (const v of [90, 80, 70, 60, 50, 40, 30, 20, 10, 0, 15, 25]) {
      el.value = String(v);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise(r => setTimeout(r, 5));
    }
  });
  await expect.poll(async () => (await studio.controlValues()).values['layer.1.dimmer']).toBe(0.25);
  // Settled: nothing else arrives afterwards.
  await page.waitForTimeout(300);
  expect((await studio.controlValues()).values['layer.1.dimmer']).toBe(0.25);
  expect(seen.maxInflight).toBe(1);
  expect(seen.values.at(-1)).toBe(0.25);
  expect(seen.values.length).toBeLessThan(12);
});

test('two controls do not wait for each other; a hold button is never coalesced', async ({ page }) => {
  const seen = await slowControl(page);
  // `controlLatest` is a page-level const: reached by name.
  await page.evaluate(`controlLatest({ id: 'master.size', value: 1.5 }); controlLatest({ id: 'layer.2.dimmer', value: 0.5 });`);
  await expect.poll(() => seen.maxInflight).toBe(2);
  await expect.poll(async () => (await studio.live()).size).toBe(1.5);
  await expect.poll(async () => (await studio.controlValues()).values['layer.2.dimmer']).toBe(0.5);
  await page.unroute('**/api/control');

  // « Noir » held then released: both posts leave, in order.
  const posts: unknown[] = [];
  page.on('request', r => { if (r.url().endsWith('/api/control') && r.method() === 'POST') posts.push(r.postDataJSON()); });
  const black = page.locator('#fxBlack');
  await black.scrollIntoViewIfNeeded();
  await black.hover();
  await page.mouse.down();
  await page.mouse.up();
  await expect.poll(() => posts.filter(p => (p as { id: string }).id === 'transport.black_hold').length).toBe(2);
  expect((await studio.state()).armed).toBe(false);
});
