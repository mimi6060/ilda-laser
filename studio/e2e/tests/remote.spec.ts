// The tablet remote (T-428): turned on from RÉGLAGES (off at every
// start), paired with the code shown there, it plays a cue and stops, and
// every other route answers 403: arming, resetting the stop, safety, the
// heartbeat. The studio is started with --remote-bind 127.0.0.1 (studio.ts):
// the "tablet" is a second browser context on loopback, never another
// machine. Preview only: no laser is ever armed.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import { createServer } from 'node:net';
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

async function audit(page: Page, where: string) {
  const r = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = r.violations.filter(v => v.impact === 'serious' || v.impact === 'critical').map(v => `${where}: ${v.id} ${v.nodes.slice(0, 3).map(n => n.target.join(' ')).join(', ')}`);
  expect(bad).toEqual([]);
}

const studio = useStudio();

interface Remote { enabled: boolean; port: number; bound_port: number | null; code: string; urls: string[]; sessions: { name: string; addr: string }[] }
const remote = () => studio.get<Remote>('/api/remote');

/** A free loopback port for the remote. */
function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.on('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const a = srv.address();
      const p = typeof a === 'object' && a ? a.port : 0;
      srv.close(() => resolve(p));
    });
  });
}

async function closed(port: number) {
  try {
    await fetch(`http://127.0.0.1:${port}/`, { signal: AbortSignal.timeout(2000) });
    return false;
  } catch {
    return true;
  }
}

test('enabled from RÉGLAGES, paired, plays and stops; never arms; off after a restart', async ({ page, browser }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  expect((await remote()).enabled).toBe(false);

  await openUi(page, studio);
  await expect(page.locator('#remotePill')).toBeHidden();
  await openWorkspace(page, 'settings');
  await page.locator('#remotePanel > summary').click();
  await expect(page.locator('#remoteOn')).not.toBeChecked();
  const port = await freePort();
  await page.locator('#remotePort').fill(String(port));
  await page.locator('#remoteOn').check();
  await expect(page.locator('#remoteActive')).toBeVisible();
  await expect(page.locator('#remoteCode')).toHaveText(/^\d{6}$/);
  const code = (await page.locator('#remoteCode').textContent())!.trim();
  await expect(page.locator('#remoteUrls')).toHaveText(`http://127.0.0.1:${port}`);
  await expect(page.locator('#remotePill')).toHaveText('Télécommande : 0');
  await audit(page, 'RÉGLAGES › Télécommande');
  // The external gate: closed, the remote could only stop.
  await page.locator('#extGate').click();
  await expect(page.locator('#extGate')).toHaveAttribute('aria-pressed', 'true');

  // The tablet: a second context on loopback.
  const ctx = await browser.newContext({ viewport: { width: 820, height: 1180 }, hasTouch: true });
  const tab = await ctx.newPage();
  await tab.goto(`http://127.0.0.1:${port}/`);
  await expect(tab.locator('#pairing')).toBeVisible();
  await audit(tab, 'pairing');
  // Unpaired: no state.
  expect((await tab.request.get(`http://127.0.0.1:${port}/api/remote/state`)).status()).toBe(401);
  await tab.locator('#code').fill('000000');
  await tab.locator('#pairBtn').click();
  await expect(tab.locator('#error')).toHaveText('Code incorrect');
  await tab.locator('#code').fill(code);
  await tab.locator('#name').fill('Tablette e2e');
  await tab.locator('#pairBtn').click();
  await expect(tab.locator('#remote')).toBeVisible();
  await expect(tab.locator('#state')).toHaveText('Laser éteint');
  await expect(tab.locator('#grid button').first()).toBeVisible();
  await audit(tab, 'remote');
  // Touch targets: at least 44 px.
  for (const sel of ['#grid button', '#tabs button', '#tap', '#sync', '#stopAll', '#stopBtn']) {
    const box = (await tab.locator(sel).first().boundingBox())!;
    expect(box.height, sel).toBeGreaterThanOrEqual(44);
  }
  // On a phone (T-510): 2 columns, every cue name in full on two lines at
  // most, the page tabs wrapped (no hidden scroll), NOIR / ARRÊT on screen.
  await tab.setViewportSize({ width: 390, height: 844 });
  await expect.poll(() => tab.locator('#grid').evaluate(g => getComputedStyle(g).gridTemplateColumns.split(' ').length)).toBe(2);
  const names = await tab.locator('#grid button').evaluateAll(els => els.map(el => {
    const r = document.createRange(); r.selectNodeContents(el);
    const lh = parseFloat(getComputedStyle(el).lineHeight);
    return { name: el.textContent, cut: el.scrollWidth > el.clientWidth || el.scrollHeight > el.clientHeight, lines: Math.round(r.getBoundingClientRect().height / lh) };
  }));
  expect(names.length).toBeGreaterThan(0);
  for (const n of names) {
    expect(n.cut, `${n.name} cut`).toBe(false);
    expect(n.lines, `${n.name} lines`).toBeLessThanOrEqual(2);
  }
  expect(await tab.locator('#tabs').evaluate(t => t.scrollWidth <= t.clientWidth)).toBe(true);
  await expect(tab.locator('#stopBtn')).toBeInViewport({ ratio: 1 });
  expect(await tab.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await tab.setViewportSize({ width: 820, height: 1180 });
  await expect(page.locator('#remotePill')).toHaveText('Télécommande : 1');
  await expect(page.locator('#remoteList')).toContainText('Tablette e2e');

  // A cue from the tablet plays on the main server.
  const pad = tab.locator('#grid button').first();
  await pad.click();
  await expect.poll(async () => (await studio.frame()).active_cue).not.toBeNull();
  // A master from the tablet.
  const ok = await tab.request.post(`http://127.0.0.1:${port}/api/remote/control`, { data: { id: 'master.brightness', value: 0.5 } });
  expect(ok.status()).toBe(200);
  await expect.poll(async () => (await studio.frame()).live.brightness).toBeCloseTo(0.5, 3);

  // Everything else is refused, cookie or not.
  for (const path of ['/api/arm', '/api/estop/reset', '/api/safety', '/api/heartbeat', '/api/remote', '/api/control']) {
    const r = await tab.request.post(`http://127.0.0.1:${port}${path}`, { data: { on: true } });
    expect(r.status(), path).toBe(403);
  }
  for (const id of ['transport.arm', 'timeline.play']) {
    const r = await tab.request.post(`http://127.0.0.1:${port}/api/remote/control`, { data: { id, value: 1 } });
    expect(r.status(), id).toBe(403);
  }
  expect((await studio.state()).armed).toBe(false);

  // NOIR / ARRÊT latches the stop; resetting it is the main screen's job.
  await tab.locator('#stopBtn').click();
  await expect.poll(async () => (await studio.get<{ estop: { source: string } | null }>('/api/arm')).estop?.source).toBe('remote');
  await expect(tab.locator('#stopped')).toBeVisible();
  expect((await tab.request.post(`http://127.0.0.1:${port}/api/estop/reset`, { data: {} })).status()).toBe(403);
  expect((await studio.get<{ estop: unknown }>('/api/arm')).estop).not.toBeNull();

  // Disconnected from the main screen: the tablet is back to pairing.
  await page.locator('#remoteList [data-kick]').click();
  await expect(page.locator('#remotePill')).toHaveText('Télécommande : 0');
  await expect(tab.locator('#pairing')).toBeVisible();
  await ctx.close();

  // A restart: off again, the port closed.
  await studio.restart();
  expect((await remote()).enabled).toBe(false);
  expect((await remote()).port).toBe(port);
  expect(await closed(port)).toBe(true);
  expect((await studio.state()).armed).toBe(false);
  await studio.post('/api/estop/reset', {});
});
