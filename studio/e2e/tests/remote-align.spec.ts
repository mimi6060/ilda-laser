// Zone alignment from a phone (T-460): the main screen opens a session
// for a paired remote, the phone pushes a corner by small steps (a draft
// shown through the Grid pattern), and only « Appliquer » on the main
// screen writes the zone; « Rejeter » drops the draft. The "phone" is a
// second browser context on loopback (--remote-bind 127.0.0.1), never
// another machine. Preview only: the laser is never armed.
import { test, expect, useStudio, openUi, reveal, isLit, type Point } from '../studio';
import { createServer } from 'node:net';
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Align { zone: number; remote: string; draft: number[][]; base: number[][]; changed: boolean }
interface Zone { id: number; geometry: { warp: { enabled: boolean; corners: number[][] } } }
const align = async () => (await studio.get<{ align: Align | null }>('/api/state')).align;
const zone1 = async () => (await studio.get<{ config: { zones: Zone[] } }>('/api/projection')).config.zones[0];
/** The right-most lit x of the preview (the Grid pattern's right edge). */
const rightEdge = async () => Math.max(...((await studio.frame()).points.filter(isLit) as Point[]).map(p => p[0]));

async function audit(page: Page, where: string, include?: string) {
  let b = new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']);
  if (include) b = b.include(include);
  const r = await b.analyze();
  const bad = r.violations.filter(v => v.impact === 'serious' || v.impact === 'critical').map(v => `${where}: ${v.id} ${v.nodes.slice(0, 3).map(n => n.target.join(' ')).join(', ')}`);
  expect(bad).toEqual([]);
}

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

test.afterEach(async () => {
  await studio.post('/api/zones/align/close');
  await studio.post('/api/remote', { enabled: false });
  await studio.post('/api/testpattern', { pattern: null });
});

test('the phone proposes, the main screen applies or rejects', async ({ page, browser }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  expect(await studio.post('/api/projection', { zones: [{ id: 1, name: 'Écran' }], default_zones: [1] })).toBe(200);
  const port = await freePort();
  expect(await studio.post('/api/remote', { enabled: true, port })).toBe(200);
  const { code } = await studio.get<{ code: string }>('/api/remote');

  // The phone pairs.
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  const phone = await ctx.newPage();
  const base = `http://127.0.0.1:${port}`;
  await phone.goto(`${base}/`);
  await phone.locator('#code').fill(code);
  await phone.locator('#name').fill('Téléphone e2e');
  await phone.locator('#pairBtn').click();
  await expect(phone.locator('#remote')).toBeVisible();
  // No session yet: the phone is told to ask, a nudge is refused.
  await expect(phone.locator('#alignAsk')).toHaveText('Demandez l\'ouverture de l\'alignement sur l\'écran principal');
  expect((await phone.request.post(`${base}/api/remote/align/nudge`, { data: { corner: 2, dx: -1, dy: 0 } })).status()).toBe(409);

  // The main screen opens the session.
  await openUi(page, studio);
  await reveal(page, '#pzPanel summary');
  await page.locator('#pzPanel summary').click();
  await expect(page.locator('#paRemote')).toHaveValue('Téléphone e2e');
  await page.locator('#paOpen').click();
  await expect(page.locator('#paActive')).toBeVisible();
  await expect(page.locator('#paTitle')).toContainText('zone « Écran » — télécommande « Téléphone e2e »');
  await audit(page, 'RÉGLAGES › Zones de projection', '#paBlock');
  // The Grid pattern, through the saved zone for now.
  await expect.poll(rightEdge).toBeGreaterThan(0.5);
  const before = await rightEdge();

  // The phone pushes the top-right corner three steps left.
  await expect(phone.locator('#alignOn')).toBeVisible();
  await audit(phone, 'phone alignment');
  for (const sel of ['[data-corner]', '[data-dx]', '#x5', '#alignReset', '#stopBtn']) {
    const box = (await phone.locator(sel).first().boundingBox())!;
    expect(box.height, sel).toBeGreaterThanOrEqual(44);
    expect(box.width, sel).toBeGreaterThanOrEqual(44);
  }
  await expect(phone.locator('#stopBtn')).toBeInViewport();
  await phone.getByRole('button', { name: 'Haut droit' }).click();
  for (let i = 0; i < 3; i++) {
    await phone.getByRole('button', { name: 'Gauche', exact: true }).click();
    await expect.poll(async () => (await align())?.draft[2][0]).toBeCloseTo(1 - 0.002 * (i + 1), 4);
  }
  await expect(phone.locator('#alignWait')).toHaveText('En attente de confirmation sur l\'écran principal');
  await expect(page.locator('#paTitle')).toContainText('modification proposée par la télécommande « Téléphone e2e »');
  // The draft is on the output; the saved zone is not touched.
  await expect.poll(rightEdge).toBeLessThan(before - 0.0005);
  expect((await zone1()).geometry.warp.enabled).toBe(false);
  // The pattern turned off (T-492): both screens say the draft is no
  // longer drawn; the session stays open. The Grid back: gone.
  await expect(page.locator('#paPattern')).toBeHidden();
  await expect(phone.locator('#alignPattern')).toBeHidden();
  expect(await studio.post('/api/testpattern', { pattern: null })).toBe(200);
  await expect(page.locator('#paPattern')).toContainText('Mire coupée');
  await expect(phone.locator('#alignPattern')).toContainText('Mire coupée sur l\'écran principal');
  expect((await align())?.remote).toBe('Téléphone e2e');
  expect(await studio.post('/api/testpattern', { pattern: 'grid' })).toBe(200);
  await expect(page.locator('#paPattern')).toBeHidden();
  await expect(phone.locator('#alignPattern')).toBeHidden();
  // The phone can't write a zone, arm or reset anything.
  for (const path of ['/api/zones/align/apply', '/api/zones/align/open', '/api/projection', '/api/safety', '/api/calibration', '/api/arm']) {
    expect((await phone.request.post(`${base}${path}`, { data: { zone: 1, remote: 'Téléphone e2e', on: true } })).status(), path).toBe(403);
  }
  // Out of bounds: refused with a message.
  const r = await phone.request.post(`${base}/api/remote/align/nudge`, { data: { corner: 2, dx: 0, dy: 1 } });
  expect(r.status()).toBe(409);
  expect(await r.text()).toContain('Limite atteinte');
  expect((await phone.request.post(`${base}/api/remote/align/nudge`, { data: { corner: 2, dx: 0.5, dy: 0 } })).status()).toBe(400);

  // « Appliquer » on the main screen writes it.
  await page.locator('#paApply').click();
  await expect.poll(async () => (await zone1()).geometry.warp.enabled).toBe(true);
  const z = await zone1();
  expect(z.geometry.warp.corners[2][0]).toBeCloseTo(0.994, 4);
  expect(await align()).toBeNull();
  await expect(page.locator('#paActive')).toBeHidden();
  await expect(phone.locator('#alignAsk')).toBeVisible();

  // Again, rejected this time: nothing changes.
  await page.locator('#paOpen').click();
  await expect(phone.locator('#alignOn')).toBeVisible();
  await phone.getByRole('button', { name: 'Pas ×5' }).click();
  await phone.getByRole('button', { name: 'Bas gauche' }).click();
  await phone.getByRole('button', { name: 'Droite', exact: true }).click();
  await expect.poll(async () => (await align())?.draft[0][0]).toBeCloseTo(-0.99, 4);
  await page.locator('#paReject').click();
  await expect.poll(align).toBeNull();
  expect((await zone1()).geometry.warp.corners).toEqual(z.geometry.warp.corners);
  await expect(page.locator('#paActive')).toBeHidden();

  // The stop from the phone ends a session at once.
  await page.locator('#paOpen').click();
  await expect.poll(async () => (await align())?.remote).toBe('Téléphone e2e');
  await phone.locator('#stopBtn').click();
  await expect.poll(align).toBeNull();
  expect((await studio.state()).armed).toBe(false);
  await ctx.close();
  await studio.post('/api/estop/reset', {});
});
