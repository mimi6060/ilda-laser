// T-432: the notification centre. Every refused or failed request of the
// page becomes a toast and a line behind the bell; a condition stays
// pinned while it lasts; the safety banners keep their place. The studio
// runs with --test-interlock (arming is always refused) and preview only;
// the forced errors are answered by Playwright, the studio never sees them.
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import type { Page } from '@playwright/test';
import { test, expect, useStudio, openUi, focusPage } from '../studio';

const studio = useStudio({ testInterlock: true });

// The page's one door to the studio (index.html).
declare function api(method: string, path: string, body?: unknown): Promise<Response>;

/** Every POST route the page calls with a literal path. */
function postRoutes(): string[] {
  // The page's code lives in its ES modules (T-435).
  const src = path.join(__dirname, '..', '..', 'src');
  const ui = path.join(src, 'ui');
  const html = [readFileSync(path.join(src, 'index.html'), 'utf8'), ...readdirSync(ui).filter(f => f.endsWith('.js')).map(f => readFileSync(path.join(ui, f), 'utf8'))].join('\n');
  const found = new Set<string>();
  for (const re of [/api\('POST', '(\/api\/[^'?]+)'/g, /\bpost\('(\/api\/[^'?]+)'/g]) {
    for (const m of html.matchAll(re)) found.add(m[1]);
  }
  return [...found].sort();
}

const panelRows = (page: Page) => page.locator('#noticePanel .n-row');

async function openPanel(page: Page) {
  if (await page.locator('#noticePanel').isHidden()) await page.locator('#noticeBtn').click();
  await expect(page.locator('#noticePanel')).toBeVisible();
}
async function closePanel(page: Page) {
  if (await page.locator('#noticePanel').isVisible()) await page.locator('#noticeBtn').click();
  await expect(page.locator('#noticePanel')).toBeHidden();
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await openUi(page, studio);
});

test('a 500 on any POST route gives a toast with the server text and a line in the list', async ({ page }) => {
  const routes = postRoutes();
  expect(routes.length).toBeGreaterThan(40);
  let target = '';
  await page.route('**/api/**', route => {
    const url = new URL(route.request().url());
    if (route.request().method() === 'POST' && url.pathname === target) {
      return route.fulfill({ status: 500, contentType: 'text/plain; charset=utf-8', body: `panne simulée sur ${target}` });
    }
    return route.continue();
  });
  // Exactly this route's text (one route's path can start another's).
  const exactly = (r: string) => new RegExp(`panne simulée sur ${r.replace(/[/.*+?^${}()|[\]\\]/g, '\\$&')}$`);
  for (const r of routes) {
    target = r;
    await page.evaluate(p => api('POST', p, {}).catch(() => {}), r);
    const toast = page.locator('#toasts .toast').filter({ has: page.locator('.toast-text', { hasText: exactly(r) }) });
    await expect(toast, r).toBeVisible();
    await expect(toast).toHaveClass(/error/);
    await expect(toast).toHaveAttribute('role', 'alert');
    await toast.locator('.toast-x').click();
    await expect(toast).toHaveCount(0);
  }
  target = '';
  await openPanel(page);
  for (const r of routes) await expect(page.locator('#noticePanel .n-line', { hasText: exactly(r) }), r).toHaveCount(1);
  // Read: no unread count left.
  await closePanel(page);
  await expect(page.locator('#noticeBadge')).toBeHidden();
});

test('a refusal shown by its own control is recorded without a second toast', async ({ page }) => {
  // The project dialog shows « Ouvrir » errors under its field.
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="open"]').click();
  await page.locator('#projPath').fill('Introuvable');
  await page.locator('#projOk').click();
  await expect(page.locator('#projError')).toContainText('introuvable');
  await expect(page.locator('#toasts .toast')).toHaveCount(0);
  await page.locator('#projCancel').click();
  await openPanel(page);
  await expect(panelRows(page).filter({ hasText: 'Projet · projet introuvable' })).toHaveCount(1);
});

test('the same error 50 times in a row is one line with a count', async ({ page }) => {
  await page.evaluate(async () => {
    for (let i = 0; i < 50; i++) await api('GET', '/api/nulle-part');
  });
  const toast = page.locator('#toasts .toast', { hasText: 'not found' });
  await expect(toast).toHaveCount(1);
  await expect(toast).toContainText('×50');
  await openPanel(page);
  const rows = panelRows(page).filter({ hasText: 'not found' });
  await expect(rows).toHaveCount(1);
  await expect(rows).toContainText('×50');
});

test('no answer from the studio pins « Studio injoignable » until it answers again', async ({ page }) => {
  await page.route('**/api/**', route => route.abort());
  await expect(page.locator('#noticeBtn')).toHaveAttribute('data-level', 'error');
  await expect(page.locator('#toasts .toast', { hasText: 'Studio injoignable' })).toBeVisible();
  await openPanel(page);
  await expect(page.locator('#noticePinned .n-row', { hasText: 'Studio injoignable' })).toHaveCount(1);
  // « Tout effacer » keeps a condition that is still on.
  await page.locator('#noticeClear').click();
  await expect(page.locator('#noticePinned .n-row', { hasText: 'Studio injoignable' })).toHaveCount(1);
  await page.unroute('**/api/**');
  await expect(page.locator('#noticePinned .n-row')).toHaveCount(0);
  await expect(panelRows(page).filter({ hasText: 'Studio injoignable' })).toContainText('résolu');
  await expect(page.locator('#toasts .toast', { hasText: 'Studio injoignable' })).toHaveCount(0);
  await expect(page.locator('#noticeBtn')).not.toHaveAttribute('data-level', 'error');
  // Now everything goes.
  await page.locator('#noticeClear').click();
  await expect(panelRows(page)).toHaveCount(0);
  await expect(page.locator('#noticeEmpty')).toBeVisible();
});

test('the arming refusal keeps its banner, never under a toast, and is recorded', async ({ page }) => {
  // Three toasts on screen first (and one waiting).
  await page.route('**/api/essai-*', route => route.fulfill({ status: 500, body: 'panne ' + new URL(route.request().url()).pathname }));
  await page.evaluate(async () => {
    for (const p of ['/api/essai-1', '/api/essai-2', '/api/essai-3', '/api/essai-4']) await api('GET', p);
  });
  await expect(page.locator('#toasts .toast')).toHaveCount(3);
  await page.locator('#armBtn').click();
  const box = page.locator('#armRefused');
  await expect(box).toBeVisible();
  await expect(box).toContainText('Verrou de test');
  // Nothing over it: not a toast, not the top bar's laser line (T-432).
  const covered = await box.evaluate(el => {
    const r = el.getBoundingClientRect();
    return [[r.left + 2, r.top + 1], [r.left + r.width / 2, r.top + r.height / 2], [r.right - 4, r.bottom - 4]]
      .map(([x, y]) => document.elementFromPoint(x, y))
      .filter(hit => !el.contains(hit)).map(hit => hit ? hit.tagName + '#' + hit.id : 'none');
  });
  expect(covered).toEqual([]);
  // Recorded, without a toast of its own.
  await expect(page.locator('#toasts .toast', { hasText: 'Verrou de test' })).toHaveCount(0);
  await openPanel(page);
  await expect(panelRows(page).filter({ hasText: 'Sécurité · armement impossible' })).toContainText('Verrou de test');
  expect((await studio.state()).armed).toBe(false);
});

test('Échap stays the emergency stop with toasts on screen, and the stop is pinned', async ({ page }) => {
  await page.evaluate(() => api('GET', '/api/nulle-part'));
  const toast = page.locator('#toasts .toast');
  await expect(toast).toHaveCount(1);
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect(page.locator('#estopBanner')).toBeVisible();
  await expect.poll(async () => (await studio.get('/api/arm')).estop !== null).toBe(true);
  await expect(toast).toHaveCount(1); // Échap is not « close the toast »
  // The banner is above the toasts' layer and the stop is a pinned condition.
  const z = await page.evaluate(() => [getComputedStyle(document.getElementById('top')!).zIndex, getComputedStyle(document.getElementById('toasts')!).zIndex].map(Number));
  expect(z[0]).toBeGreaterThan(z[1]);
  await openPanel(page);
  await expect(page.locator('#noticePinned .n-row', { hasText: "Arrêt d'urgence" })).toHaveCount(1);
  await studio.post('/api/estop/reset', {});
  await expect(page.locator('#noticePinned .n-row', { hasText: "Arrêt d'urgence" })).toHaveCount(0);
});

// T-482 (found by the soak): an open notification or history panel stayed
// open after Échap, over the e-stop banner, and hid its reset button.
test('Échap closes the bell and history panels, so the e-stop reset is never covered', async ({ page }) => {
  for (const [btn, panel] of [['#noticeBtn', '#noticePanel'], ['#historyBtn', '#historyPanel']]) {
    await page.locator(btn).click();
    await expect(page.locator(panel)).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('#estopBanner')).toBeVisible();
    await expect(page.locator(panel)).toBeHidden();
    await expect(page.locator(btn)).toHaveAttribute('aria-expanded', 'false');
    // A real click: it fails if anything lies over the button.
    await page.locator('#estopReset').click({ timeout: 3_000 });
    await expect(page.locator('#estopBanner')).toBeHidden();
  }
  expect((await studio.state()).armed).toBe(false);
});

test('the studio\'s own notices are read only, with ids to read on from', async () => {
  const j = await studio.get('/api/notices?since=0');
  expect(Array.isArray(j.notices)).toBe(true);
  expect(typeof j.last).toBe('number');
  expect((await studio.frame() as unknown as { notices_last: number }).notices_last).toBe(j.last);
  const r = await fetch(`${studio.url}/api/notices`, { method: 'POST', body: '{}' });
  expect(r.status).not.toBe(200);
});
