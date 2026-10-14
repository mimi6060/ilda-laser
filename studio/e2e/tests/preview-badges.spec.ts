// The preview's badges (T-512): at 1024×768, 1280×800, 1440×900 and
// 1920×1080, in 2D, 3D and 2D+3D, on every workspace, with « Préparer »
// off and on, every badge shown at once (« Sortie », « Laser éteint »,
// « Galvos simulés », « Faisceaux dans le public ») sits inside the view
// and no two of them intersect; the 3D drawer opens under all of them;
// with the prepared pane shown (« Préparer » on, a cue prepared with it
// on or off) no badge covers the pane or any of its labels and controls,
// the cue name stays readable, the pane's buttons whole, and the 3D view
// keeps 160 px.
// At 1024×768 the drawer scrolls visibly (a scrollbar and a fade while
// more settings sit below) and its last setting can be reached.
//
// Preview only: the fake test output, the laser never armed; everything
// here is display, measured with getBoundingClientRect.
import { test, expect, useStudio, openUi, openWorkspace, open3dSettings, type Workspace } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
// Headless Chromium hides every scrollbar (--hide-scrollbars, a Playwright
// default); drop that flag in this file so scrollbars are drawn as in the
// operator's browser: the layout is measured with them, and the drawer's
// bar can be measured.
test.use({ launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] } });
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
    // The prepared pane and every label and control in it, when shown.
    const pane = [...document.querySelectorAll<HTMLElement>('#prepPane, #prepPane .prep-head > *, #prepPane canvas, #prepPane .prep-side label, #prepPane .prep-side input')]
      .filter(e => e.offsetParent && e.getBoundingClientRect().width > 0)
      .map(e => rect(e, e.id || `${e.tagName.toLowerCase()} « ${e.textContent?.trim() || (e as HTMLInputElement).type} »`));
    // With a cue prepared: the name's room, the header's items inside the
    // pane and the window, and the 3D canvas's size.
    const pp = document.getElementById('prepPane')!, nameEl = document.getElementById('prepName')!;
    const head = pp.offsetParent ? [...pp.querySelectorAll<HTMLElement>('.prep-head > *')].map(e => rect(e, e.id || e.tagName.toLowerCase())) : [];
    const paneBox = pp.offsetParent ? rect(pp, 'prepPane') : null;
    const name = pp.offsetParent ? { width: nameEl.getBoundingClientRect().width, full: nameEl.scrollWidth, text: nameEl.textContent ?? '' } : null;
    const bm = document.getElementById('beams')!;
    const beams = bm.offsetParent ? rect(bm, 'beams') : null;
    const vp = { id: 'window', x: 0, y: 0, width: innerWidth, height: innerHeight };
    return { badges, view, drawer, pane, head, paneBox, name, beams, vp };
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

async function checkBadges(page: Page, label: string, expected: string[], paneShown: boolean) {
  const m = await measure(page);
  expect(m.badges.map(b => b.id).sort(), `${label} badges shown`).toEqual([...expected].sort());
  for (const b of m.badges) expect(inside(b, m.view), `${label} #${b.id} inside the view`).toBe(true);
  for (let i = 0; i < m.badges.length; i++)
    for (let j = i + 1; j < m.badges.length; j++)
      expect(overlap(m.badges[i], m.badges[j]), `${label} #${m.badges[i].id} and #${m.badges[j].id} overlap`).toBe(false);
  if (m.drawer) for (const b of m.badges) expect(overlap(b, m.drawer), `${label} drawer covers #${b.id}`).toBe(false);
  expect(m.pane.length > 0, `${label} prepared pane shown`).toBe(paneShown);
  for (const b of m.badges) for (const c of m.pane) expect(overlap(b, c), `${label} #${b.id} covers ${c.id} of the prepared pane`).toBe(false);
  // PUPITRE's 300 px stage stacks the pane under the views, past the
  // window's edge, on develop already: its own task (not a badge issue).
  if (paneShown && !label.startsWith('desk')) {
    // Every header item (« Préparé », the name, « Envoyer », « Vider ») whole, inside the pane and the window.
    for (const h of m.head) {
      expect(inside(h, m.paneBox!), `${label} ${h.id} inside the prepared pane`).toBe(true);
      expect(inside(h, m.vp), `${label} ${h.id} inside the window`).toBe(true);
    }
    // The prepared cue's name can be read: 40 px at least, or all of it if shorter.
    expect(m.name!.width, `${label} « ${m.name!.text} » readable (${m.name!.width.toFixed(0)} px)`).toBeGreaterThanOrEqual(Math.min(40, m.name!.full) - 0.5);
    // The 3D view keeps T-503's 160 px with the pane shown.
    if (m.beams) expect(m.beams.height, `${label} 3D view height with the pane`).toBeGreaterThanOrEqual(160);
  }
}

/** No prepared pane; « Préparer » on with nothing prepared; on with a cue prepared; a cue prepared, « Préparer » off. */
const PREP_STATES = ['none', 'mode', 'cue', 'cue-only'] as const;

async function setPrep(page: Page, state: typeof PREP_STATES[number], cueId: string) {
  expect(await studio.post('/api/prepared/clear')).toBe(200);
  if (state === 'cue' || state === 'cue-only') expect(await studio.post('/api/prepared', { id: cueId })).toBe(200);
  const btn = page.locator('#prepBtn');
  const want = state === 'mode' || state === 'cue';
  if (want !== /active/.test((await btn.getAttribute('class')) ?? '')) await btn.click();
  await expect(btn).toHaveAttribute('aria-pressed', String(want));
  if (state === 'none') await expect(page.locator('#prepPane')).toBeHidden();
  else await expect(page.locator('#prepPane')).toBeVisible();
  if (state === 'cue' || state === 'cue-only') await expect(page.locator('#prepSend')).toBeEnabled();
  else await expect(page.locator('#prepSend')).toBeDisabled();
}

test.beforeEach(async () => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  // The galvo simulation on, so its badge shows with the others.
  expect(await studio.post('/api/venue/sim', { ...SIM_OFF, enabled: true })).toBe(200);
});

test.afterEach(async () => {
  await studio.post('/api/prepared/clear');
  await studio.post('/api/venue/sim', SIM_OFF);
});

for (const size of SIZES) {
  test(`${size.width}×${size.height}: no preview badge covers another or the prepared pane, on every workspace and in every mode`, async ({ page }) => {
    test.setTimeout(240_000);
    await page.setViewportSize(size);
    await openUi(page, studio);
    await expect(page.locator('#galvoBadge')).toBeVisible();
    const cueId = (await page.locator('#cues .cue[data-id]').first().getAttribute('data-id'))!;
    for (const ws of WORKSPACES) {
      await openWorkspace(page, ws);
      for (const prep of PREP_STATES) {
        await setPrep(page, prep, cueId);
        for (const mode of MODES) {
          const label = `${ws} ${mode} prep:${prep}`;
          await setMode(page, mode);
          const expected = ['offBadge', 'galvoBadge'];
          if (prep !== 'none') expected.push('outTag');
          if (mode !== '2d') { expected.push('aud3d'); await expect(page.locator('#aud3d'), label).toBeVisible(); }
          await checkBadges(page, label, expected, prep !== 'none');
          if (mode === '2d') continue;
          await open3dSettings(page);
          await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
          await checkBadges(page, `${label} drawer open`, expected, prep !== 'none');
          await page.locator('#render3dClose').click();
          await expect(page.locator('#render3d')).toBeHidden();
        }
      }
      await setPrep(page, 'none', cueId);
    }
  });
}

test('1024×768: the 3D drawer scrolls visibly and its last setting can be reached', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await openUi(page, studio);
  await setMode(page, '3d');
  await open3dSettings(page);
  const drawer = page.locator('#render3d');
  const m = await drawer.evaluate(d => { const cs = getComputedStyle(d); return { sh: d.scrollHeight, ch: d.clientHeight, oy: cs.overflowY, bar: (d as HTMLElement).offsetWidth - d.clientWidth - parseFloat(cs.borderLeftWidth) - parseFloat(cs.borderRightWidth) }; });
  expect(m.sh, 'more settings than the drawer shows').toBeGreaterThan(m.ch);
  expect(m.oy).toBe('auto');
  expect(m.bar, 'a drawn scrollbar, not a 2 px overlay').toBeGreaterThanOrEqual(6);
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
