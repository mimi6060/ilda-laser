// The preview's badges (T-512): at 1024×768, 1280×800, 1440×900 and
// 1920×1080, in 2D, 3D and 2D+3D, on every workspace, with « Préparer »
// off and on, every badge shown at once (« Sortie », « Laser éteint »,
// « Galvos simulés », « Faisceaux dans le public ») sits inside the view
// and no two of them intersect; the 3D drawer opens under all of them;
// with the prepared pane shown (« Préparer » on, a cue prepared with it
// on or off) no badge covers the pane or any of its labels and controls,
// the audience badge still names the beams (« Faisc. public ») and the
// row is one line at 1024 even with a three-digit count, the pane sits
// inside the view and the window (PUPITRE too: T-513), the
// whole cue name shows (no ellipsis, no clamp), the pane's buttons are
// whole, its slider column is whole or hidden, its picture is at least
// 56 px and uses the height left, and the 3D view keeps 160 px.
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
    // With the pane shown: the header's items, the name's text whole (no
    // overflow sideways, no clamped line), the picture, the slider column
    // (whole or hidden) and the canvases' sizes.
    const pp = document.getElementById('prepPane')!, nameEl = document.getElementById('prepName')!;
    const head = pp.offsetParent ? [...pp.querySelectorAll<HTMLElement>('.prep-head > *')].map(e => rect(e, e.id || e.tagName.toLowerCase())) : [];
    const paneBox = pp.offsetParent ? rect(pp, 'prepPane') : null;
    const name = pp.offsetParent ? { width: nameEl.getBoundingClientRect().width, text: nameEl.textContent ?? '', whole: nameEl.scrollWidth <= nameEl.clientWidth + 1 && nameEl.scrollHeight <= nameEl.clientHeight + 1 } : null;
    const pic = pp.offsetParent ? rect(document.getElementById('prepared')!, 'prepared') : null;
    const sd = pp.querySelector<HTMLElement>('.prep-side')!;
    const side = pp.offsetParent && sd.offsetParent ? { shown: sd.clientHeight, needs: sd.scrollHeight } : null;
    const bm = document.getElementById('beams')!, pv = document.getElementById('preview')!;
    const beams = bm.offsetParent ? rect(bm, 'beams') : null;
    const preview = pv.offsetParent ? rect(pv, 'preview') : null;
    const vp = { id: 'window', x: 0, y: 0, width: innerWidth, height: innerHeight };
    // The audience badge's visible text, and the row's lines with a
    // three-digit count (set and put back in this same pass: the page
    // refreshes the count between passes).
    const aud = document.getElementById('aud3d')!, n = document.getElementById('aud3dN')!;
    const audText = aud.offsetParent ? aud.innerText : null;
    const was = n.textContent; n.textContent = '120';
    const rowLines = new Set([...document.querySelectorAll<HTMLElement>('#viewBadges > *')].filter(b => b.offsetParent).map(b => Math.round(b.getBoundingClientRect().top))).size;
    n.textContent = was;
    return { badges, view, drawer, pane, head, paneBox, name, pic, side, beams, preview, vp, audText, rowLines };
  });
}

const overlap = (a: Box, b: Box) =>
  a.x < b.x + b.width - 0.5 && b.x < a.x + a.width - 0.5 && a.y < b.y + b.height - 0.5 && b.y < a.y + a.height - 0.5;
const inside = (a: Box, b: Box, tol = 0.5) =>
  a.x >= b.x - tol && a.y >= b.y - tol && a.x + a.width <= b.x + b.width + tol && a.y + a.height <= b.y + b.height + tol;

async function setMode(page: Page, mode: typeof MODES[number]) {
  await page.locator(`#viewSeg [data-view="${mode}"]`).click();
  await expect(page.locator(`#viewSeg [data-view="${mode}"]`)).toHaveClass(/active/);
  // In 2D+3D with the pane in a view up to 420 px wide, the 3D view
  // yields to the 2D output (T-516: stage-layout.spec measures it).
  if (mode === '3d') await expect(page.locator('#beams')).toBeVisible();
  if (mode === 'both') await expect(page.locator('#preview')).toBeVisible();
  await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
}

async function checkBadges(page: Page, label: string, expected: string[], paneShown: boolean, cuePrepared = false) {
  const m = await measure(page);
  expect(m.badges.map(b => b.id).sort(), `${label} badges shown`).toEqual([...expected].sort());
  for (const b of m.badges) expect(inside(b, m.view), `${label} #${b.id} inside the view`).toBe(true);
  for (let i = 0; i < m.badges.length; i++)
    for (let j = i + 1; j < m.badges.length; j++)
      expect(overlap(m.badges[i], m.badges[j]), `${label} #${m.badges[i].id} and #${m.badges[j].id} overlap`).toBe(false);
  if (m.drawer) for (const b of m.badges) expect(overlap(b, m.drawer), `${label} drawer covers #${b.id}`).toBe(false);
  expect(m.pane.length > 0, `${label} prepared pane shown`).toBe(paneShown);
  for (const b of m.badges) for (const c of m.pane) expect(overlap(b, c), `${label} #${b.id} covers ${c.id} of the prepared pane`).toBe(false);
  if (paneShown) {
    // The audience badge, short or not, says it counts beams (« Faisc. public : N », never a bare « Public : N »).
    if (m.audText !== null) expect(m.audText, `${label} audience badge names the beams`).toMatch(/Faisc/);
    // At 1024 the row stays one line with a three-digit count, so the pane
    // under it keeps its height (PUPITRE's pane is under the views, not
    // under the row, and its 300 px row wraps).
    if (page.viewportSize()!.width === 1024 && !label.startsWith('desk')) expect(m.rowLines, `${label} badge row lines with « 120 »`).toBe(1);
    // The pane inside the view and the window (PUPITRE stacks it under the views: T-513).
    expect(inside(m.paneBox!, m.view), `${label} prepared pane inside the view`).toBe(true);
    expect(inside(m.paneBox!, m.vp), `${label} prepared pane inside the window`).toBe(true);
    // Every header item (« Préparé », the name, « Envoyer », « Vider ») whole, inside the pane and the window.
    for (const h of m.head) {
      expect(inside(h, m.paneBox!), `${label} ${h.id} inside the prepared pane`).toBe(true);
      expect(inside(h, m.vp), `${label} ${h.id} inside the window`).toBe(true);
    }
    // The whole name shows: no ellipsis, no clamped line, so the operator can tell the cue apart without hovering.
    expect(m.name!.whole, `${label} « ${m.name!.text} » shown whole (${m.name!.width.toFixed(0)} px)`).toBe(true);
    // The slider column shows whole or not at all, never a sliver.
    if (m.side) expect(m.side.needs, `${label} slider column whole (${m.side.shown} px shown)`).toBeLessThanOrEqual(m.side.shown + 1);
    // The 3D view keeps T-503's 160 px with the pane shown. PUPITRE's
    // 300 px strip in 2D+3D is the one place the two views and the
    // picture share the height instead.
    if (m.beams && !(label.startsWith('desk both'))) expect(m.beams.height, `${label} 3D view height with the pane`).toBeGreaterThanOrEqual(160);
    if (cuePrepared) {
      // The prepared picture is at least 56 px, and takes the height left
      // under the badges and the header (the pane ends within 8 px of the
      // view's edge) unless it is large anyway or already as wide as the
      // output view beside it (the two share the width).
      expect(m.pic!.height, `${label} prepared picture`).toBeGreaterThanOrEqual(56);
      const slack = m.view.y + m.view.height - (m.paneBox!.y + m.paneBox!.height);
      const square = m.preview ?? m.beams;
      const widthBound = square ? m.pic!.width >= square.width - 1 : false;
      expect(m.pic!.height >= 160 || slack <= 8 || widthBound, `${label} pane uses the height left (${slack.toFixed(0)} px unused, picture ${m.pic!.height.toFixed(0)} px)`).toBe(true);
    }
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

// The layout is measured in the page (ResizeObserver): a loop that never
// settles is reported by Chromium on the console; none may show.
let loops: string[] = [];
test.beforeEach(async ({ page }) => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  // The galvo simulation on, so its badge shows with the others.
  expect(await studio.post('/api/venue/sim', { ...SIM_OFF, enabled: true })).toBe(200);
  loops = [];
  page.on('console', msg => { if (/ResizeObserver loop/.test(msg.text())) loops.push(msg.text()); });
  page.on('pageerror', err => { if (/ResizeObserver loop/.test(err.message)) loops.push(err.message); });
});

test.afterEach(async () => {
  await studio.post('/api/prepared/clear');
  await studio.post('/api/venue/sim', SIM_OFF);
  expect(loops, 'no ResizeObserver loop').toEqual([]);
});

for (const size of SIZES) {
  test(`${size.width}×${size.height}: no preview badge covers another or the prepared pane, on every workspace and in every mode`, async ({ page }) => {
    test.setTimeout(240_000);
    await page.setViewportSize(size);
    await openUi(page, studio);
    await expect(page.locator('#galvoBadge')).toBeVisible();
    // The cue with the longest name on the page (« Lissajous 1:2 · cyan → magenta », 31 characters), the one a narrow pane must still show whole.
    const cueId = await page.evaluate(() => [...document.querySelectorAll<HTMLElement>('#cues .cue[data-id]')]
      .map(c => ({ id: c.dataset.id!, n: (c.textContent ?? '').trim().length })).sort((a, b) => b.n - a.n)[0].id);
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
          const cue = prep === 'cue' || prep === 'cue-only';
          await checkBadges(page, label, expected, prep !== 'none', cue);
          if (mode === '2d') continue;
          await open3dSettings(page);
          await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
          await checkBadges(page, `${label} drawer open`, expected, prep !== 'none', cue);
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
