// The preview's badges (T-512): at 1024×768, 1280×800, 1440×900 and
// 1920×1080, in 2D, 3D and 2D+3D, on every workspace, with « Préparer »
// off and on, every badge shown at once (« Sortie », « Laser éteint »,
// « Galvos simulés », « Faisceaux dans le public ») sits inside the view
// and no two of them intersect; the 3D drawer opens under all of them.
// At 1024×768 the drawer scrolls visibly (a scrollbar and a fade while
// more settings sit below) and its last setting can be reached.
//
// Preview only: the fake test output, the laser never armed; everything
// here is display, measured with getBoundingClientRect.
import { test, expect, useStudio, openUi, openWorkspace, open3dSettings, type Workspace } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
const SIZES = [{ width: 1024, height: 768 }, { width: 1280, height: 800 }, { width: 1440, height: 900 }, { width: 1920, height: 1080 }] as const;
const WORKSPACES: Workspace[] = ['live', 'timeline', 'creation', 'settings', 'desk'];
const MODES = ['2d', '3d', 'both'] as const;
const SIM_OFF = { enabled: false, response_ms: 0.3, divergence_mrad: 1 };

type Box = { id: string; x: number; y: number; width: number; height: number };

/** The badges on screen, with their boxes, and the view's and drawer's boxes. */
async function measure(page: Page) {
  return page.evaluate(() => {
    const rect = (el: Element, id: string) => { const r = el.getBoundingClientRect(); return { id, x: r.x, y: r.y, width: r.width, height: r.height }; };
    const badges = [...document.querySelectorAll<HTMLElement>('#offBadge, #galvoBadge, #outTag, #aud3d, #viewBadges > *')]
      .filter(b => b.offsetParent && getComputedStyle(b).display !== 'none' && b.getBoundingClientRect().width > 0)
      .map(b => rect(b, b.id));
    const view = rect(document.querySelector('#stage .view')!, 'view');
    const d = document.getElementById('render3d')!;
    const drawer = d.offsetParent ? rect(d, 'render3d') : null;
    return { badges, view, drawer };
  });
}

const overlap = (a: Box, b: Box) =>
  a.x < b.x + b.width - 0.5 && b.x < a.x + a.width - 0.5 && a.y < b.y + b.height - 0.5 && b.y < a.y + a.height - 0.5;
const inside = (a: Box, b: Box, tol = 0.5) =>
  a.x >= b.x - tol && a.y >= b.y - tol && a.x + a.width <= b.x + b.width + tol && a.y + a.height <= b.y + b.height + tol;

async function setMode(page: Page, mode: typeof MODES[number]) {
  await page.locator(`#viewSeg [data-view="${mode}"]`).click();
  await expect(page.locator(`#viewSeg [data-view="${mode}"]`)).toHaveClass(/active/);
  if (mode !== '2d') await expect(page.locator('#beams')).toBeVisible();
  await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
}

async function checkBadges(page: Page, label: string, expected: string[]) {
  const m = await measure(page);
  expect(m.badges.map(b => b.id).sort(), `${label} badges shown`).toEqual([...expected].sort());
  for (const b of m.badges) expect(inside(b, m.view), `${label} #${b.id} inside the view`).toBe(true);
  for (let i = 0; i < m.badges.length; i++)
    for (let j = i + 1; j < m.badges.length; j++)
      expect(overlap(m.badges[i], m.badges[j]), `${label} #${m.badges[i].id} and #${m.badges[j].id} overlap`).toBe(false);
  if (m.drawer) for (const b of m.badges) expect(overlap(b, m.drawer), `${label} drawer covers #${b.id}`).toBe(false);
}

test.beforeEach(async () => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  // The galvo simulation on, so its badge shows with the others.
  expect(await studio.post('/api/venue/sim', { ...SIM_OFF, enabled: true })).toBe(200);
});

test.afterEach(async () => {
  await studio.post('/api/venue/sim', SIM_OFF);
});

for (const size of SIZES) {
  test(`${size.width}×${size.height}: no preview badge covers another, on every workspace and in every mode`, async ({ page }) => {
    await page.setViewportSize(size);
    await openUi(page, studio);
    await expect(page.locator('#galvoBadge')).toBeVisible();
    for (const ws of WORKSPACES) {
      await openWorkspace(page, ws);
      for (const prep of [false, true]) {
        const prepBtn = page.locator('#prepBtn');
        if (prep && !(await prepBtn.isVisible())) continue;
        if (prep !== /active/.test((await prepBtn.getAttribute('class')) ?? '')) await prepBtn.click();
        for (const mode of MODES) {
          const label = `${ws} ${mode}${prep ? ' prep' : ''}`;
          await setMode(page, mode);
          const expected = ['offBadge', 'galvoBadge'];
          if (prep) expected.push('outTag');
          if (mode !== '2d') { expected.push('aud3d'); await expect(page.locator('#aud3d'), label).toBeVisible(); }
          await checkBadges(page, label, expected);
          if (mode === '2d') continue;
          await open3dSettings(page);
          await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
          await checkBadges(page, `${label} drawer open`, expected);
          await page.locator('#render3dClose').click();
          await expect(page.locator('#render3d')).toBeHidden();
        }
      }
      if (await page.locator('#prepBtn').isVisible() && /active/.test((await page.locator('#prepBtn').getAttribute('class')) ?? '')) await page.locator('#prepBtn').click();
    }
  });
}

test('1024×768: the 3D drawer scrolls visibly and its last setting can be reached', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await openUi(page, studio);
  await setMode(page, '3d');
  await open3dSettings(page);
  const drawer = page.locator('#render3d');
  const m = await drawer.evaluate(d => ({ sh: d.scrollHeight, ch: d.clientHeight, oy: getComputedStyle(d).overflowY, bar: (d as HTMLElement).offsetWidth - d.clientWidth }));
  expect(m.sh, 'more settings than the drawer shows').toBeGreaterThan(m.ch);
  expect(m.oy).toBe('auto');
  expect(m.bar, 'a scrollbar takes room: it is drawn').toBeGreaterThan(0);
  // The fade says there is more below; it goes once at the bottom.
  await expect(drawer).toHaveClass(/more-below/);
  const last = page.locator('#r3sfOutdoor');
  await last.scrollIntoViewIfNeeded();
  await expect(last).toBeInViewport();
  const r = await last.evaluate(el => { const a = el.getBoundingClientRect(), b = document.getElementById('render3d')!.getBoundingClientRect(); return a.top >= b.top - 0.5 && a.bottom <= b.bottom + 0.5; });
  expect(r, 'last setting inside the drawer after scrolling').toBe(true);
  await last.check();
  await expect(last).toBeChecked();
  await last.uncheck();
  await drawer.evaluate(d => { d.scrollTop = d.scrollHeight; });
  await expect(drawer).not.toHaveClass(/more-below/);
});
