// Vues enregistrées et préférences d'opérateur (T-426): a view brings
// back the screen's layout (panel tab, 2D/3D, cue page...) on Alt+F1…,
// never changes the output, never hides the laser button, Noir, ARRÊT or
// the e-stop banner; views and interface preferences live on the studio
// (<data-dir>/operators), so another browser finds them. Preview only:
// the fake test output, never a laser.
import AxeBuilder from '@axe-core/playwright';
import { test, expect, useStudio, openUi, openWorkspace, focusPage, type Studio } from '../studio';
import type { Page } from '@playwright/test';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const studio = useStudio();
type View = { name: string; slot: number | null; workspace: string; live_panel: string; stage_mode: string; cue_page: number };
type Op = { name: string; ready: boolean; views: View[]; last: string | null };
const DEFAULT = '/api/operators/' + encodeURIComponent('Défaut');

const op = (page: Page) => page.evaluate(() => (window as any).__operator() as Op);
async function opened(page: Page) {
  await openUi(page, studio);
  await expect.poll(async () => (await op(page)).ready).toBe(true);
}
const toast = (page: Page) => page.locator('#lockToast');
const tab = (page: Page, panel: string) => page.locator(`#liveTabs [data-panel-tab="${panel}"]`);
const stage = (page: Page, mode: string) => page.locator(`#viewSeg [data-view="${mode}"]`);
const pageTab = (page: Page, i: number) => page.locator('#cueTabs [role=tab]').nth(i);

/** What the output plays: must be identical before and after a recall. */
async function output(s: Studio) {
  const f = await s.frame() as any;
  const { rot_angle: _angle, ...live } = f.live; // the rotation's current angle moves on its own
  const active = f.cues.active.map((c: { cue: string; layer: number; level: number }) => [c.cue, c.layer, c.level]);
  return { active, rev: f.settings_rev, live, bpm: f.tempo.bpm, armed: f.armed };
}

/** The top bar's safety controls, on screen whatever the view. */
async function safetyShown(page: Page) {
  for (const id of ['#armBtn', '#blackoutBtn', '#estopBtn']) await expect(page.locator(id), id).toBeVisible();
}

test.beforeEach(async () => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  await studio.post('/api/control', { id: 'page.1' });
});

test('a view restores the panel, 3D and the page, never the output; another browser finds it', async ({ page, browser }) => {
  await opened(page);
  expect(await pageTab(page, 2).count()).toBe(1);
  // LIVE › Musique, 3D, page 3; saved in view 1.
  await tab(page, 'music').click();
  await stage(page, '3d').click();
  await pageTab(page, 2).click();
  await focusPage(page);
  await page.keyboard.press('Alt+Shift+F1');
  await expect(toast(page)).toHaveText('Vue « Vue 1 » enregistrée');
  await expect.poll(async () => (await studio.get(DEFAULT)).views).toMatchObject([{ name: 'Vue 1', slot: 1, workspace: 'live', live_panel: 'music', stage_mode: '3d', cue_page: 2 }]);

  // Change everything, with a cue playing.
  await tab(page, 'lfo').click();
  await stage(page, '2d').click();
  await pageTab(page, 0).click();
  await page.locator('#cues .cue').first().click();
  await expect.poll(async () => (await studio.frame() as any).cues.active.length).toBeGreaterThan(0);
  await openWorkspace(page, 'timeline');
  const before = await output(studio);

  await focusPage(page);
  await page.keyboard.press('Alt+F1');
  await expect(toast(page)).toHaveText('Vue rappelée');
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);
  await expect(tab(page, 'music')).toHaveAttribute('aria-selected', 'true');
  await expect(stage(page, '3d')).toHaveClass(/active/);
  await expect(pageTab(page, 2)).toHaveAttribute('aria-selected', 'true');
  await safetyShown(page);
  expect(await output(studio)).toEqual(before);
  expect(before.armed).toBe(false);

  // F2 alone is still TIMELINE; Alt+F2 (no view there) changes nothing.
  await page.keyboard.press('F2');
  await expect(page.locator('#wsTabs [data-ws-tab="timeline"]')).toHaveClass(/active/);
  await page.keyboard.press('Alt+F2');
  await expect(toast(page)).toHaveText('Aucune vue sur Alt+F2');
  await expect(page.locator('#wsTabs [data-ws-tab="timeline"]')).toHaveClass(/active/);
  await openWorkspace(page, 'live');

  // The preferences reach the studio (500 ms after the last change)...
  await expect.poll(async () => (await studio.get(DEFAULT)).prefs['laserStudio.stageTab']).toBe('3d');
  await expect.poll(async () => (await studio.get(DEFAULT)).prefs['laserStudio.livePanel']).toBe('music');
  // ...and another browser, with an empty localStorage, opens them.
  const ctx = await browser.newContext();
  try {
    const p2 = await ctx.newPage();
    await opened(p2);
    expect((await op(p2)).views.map(v => v.name)).toEqual(['Vue 1']);
    await expect(stage(p2, '3d')).toHaveClass(/active/);
    await expect(tab(p2, 'music')).toHaveAttribute('aria-selected', 'true');
    await tab(p2, 'direct').click();
    await focusPage(p2);
    await p2.keyboard.press('Alt+F1');
    await expect(tab(p2, 'music')).toHaveAttribute('aria-selected', 'true');
  } finally {
    await ctx.close();
  }
  expect(await output(studio)).toEqual(before);
});

test('no saved view ever hides the laser button, Noir, ARRÊT or the e-stop banner', async ({ page }) => {
  await opened(page);
  const keys = ['F1', 'F2', 'F3', 'F4', 'F6'];
  const spots: [string, string?][] = [['live', 'running'], ['timeline'], ['creation'], ['settings'], ['desk']];
  for (let i = 0; i < spots.length; i++) {
    const [ws, panel] = spots[i];
    await openWorkspace(page, ws as any);
    if (panel) await tab(page, panel).click();
    await focusPage(page);
    await page.keyboard.press('Alt+Shift+' + keys[i]);
    await expect(toast(page)).toHaveText(`Vue « Vue ${i + 1} » enregistrée`);
  }
  await openWorkspace(page, 'live');
  const sky = await page.locator('#skyBtn').isVisible();
  await focusPage(page);
  await page.keyboard.press('Escape'); // the e-stop, latched
  await expect(page.locator('#estopBanner')).toBeVisible();
  for (let i = 0; i < spots.length; i++) {
    await page.keyboard.press('Alt+' + keys[i]);
    await expect(page.locator(`#wsTabs [data-ws-tab="${spots[i][0]}"]`)).toHaveClass(/active/);
    await safetyShown(page);
    await expect(page.locator('#estopBanner')).toBeVisible();
    expect(await page.locator('#skyBtn').isVisible()).toBe(sky);
    if (spots[i][0] === 'desk') await openWorkspace(page, 'live'); // leaves PUPITRE (Alt+F keys work there too)
    await focusPage(page);
  }
  expect((await studio.state()).estop).toBeTruthy();
  expect((await studio.state()).armed).toBe(false);
});

test('a view naming a tab that no longer exists applies the rest, without error', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(String(e)));
  await studio.post(DEFAULT, { views: [{ name: 'Ancienne', slot: 2, workspace: 'nulle-part', live_panel: 'disparu', creation_tab: 'parti', stage_mode: '3d', cue_page: 1 }], prefs: {} });
  await opened(page);
  await tab(page, 'lfo').click();
  await focusPage(page);
  await page.keyboard.press('Alt+F2');
  await expect(toast(page)).toHaveText('Vue rappelée');
  await expect(stage(page, '3d')).toHaveClass(/active/);
  await expect(pageTab(page, 1)).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);
  await expect(tab(page, 'lfo')).toHaveAttribute('aria-selected', 'true');
  expect(errors).toEqual([]);
});

test('the « Vues » menu saves, renames and deletes; operators in RÉGLAGES', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await opened(page);
  // « Vues ▾ » fits in the tab bar at 1280 px, the top bar unchanged.
  const fit = await page.evaluate(() => {
    const b = document.getElementById('viewsBtn')!.getBoundingClientRect();
    const bar = (id: string) => { const e = document.getElementById(id)!; return e.scrollWidth <= e.clientWidth + 1; };
    return { inside: b.left >= 0 && b.right <= innerWidth, tabs: bar('wsTabs'), top: bar('top'), page: document.documentElement.scrollWidth <= innerWidth };
  });
  expect(fit).toEqual({ inside: true, tabs: true, top: true, page: true });
  await tab(page, 'scenes').click();
  await page.locator('#viewsBtn').click();
  await expect(page.locator('#viewsBtn')).toHaveAttribute('aria-expanded', 'true');
  const axe = await new AxeBuilder({ page }).include('#viewsMenu').withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(axe.violations.filter(v => v.impact === 'serious' || v.impact === 'critical').map(v => v.id)).toEqual([]);
  page.once('dialog', d => d.accept('Jeu'));
  await page.locator('#viewsMenu [role=menuitem]', { hasText: 'Enregistrer la vue' }).click();
  await expect(toast(page)).toHaveText('Vue « Jeu » enregistrée');
  await expect.poll(async () => (await studio.get(DEFAULT)).views.map((v: View) => [v.name, v.slot, v.live_panel])).toEqual([['Jeu', 1, 'scenes']]);

  await tab(page, 'direct').click();
  await page.locator('#viewsBtn').click();
  await expect(page.locator('#viewsMenu [role=menuitem]', { hasText: 'Jeu' }).first().locator('kbd')).toHaveText('Alt+F1');
  await page.locator('#viewsMenu [role=menuitem]', { hasText: /^JeuAlt\+F1$/ }).click();
  await expect(tab(page, 'scenes')).toHaveAttribute('aria-selected', 'true');
  await page.locator('#viewsBtn').click();
  page.once('dialog', d => d.accept('Spectacle'));
  await page.locator('#viewsMenu [role=menuitem]', { hasText: 'Renommer' }).click();
  await expect.poll(async () => (await studio.get(DEFAULT)).views.map((v: View) => v.name)).toEqual(['Spectacle']);
  await page.locator('#viewsBtn').click();
  page.once('dialog', d => d.accept());
  await page.locator('#viewsMenu [role=menuitem]', { hasText: 'Supprimer' }).click();
  await expect.poll(async () => (await studio.get(DEFAULT)).views.length).toBe(0);

  // A second operator: its own views, the first one's kept.
  await page.keyboard.press('Alt+Shift+F3');
  await expect.poll(async () => (await studio.get(DEFAULT)).views.length).toBe(1);
  await openWorkspace(page, 'settings');
  await page.locator('#operatorPanel summary').click();
  await expect(page.locator('#opDelete')).toBeDisabled();
  page.once('dialog', d => d.accept('Marie'));
  await page.locator('#opNew').click();
  await expect(page.locator('#opSelect')).toHaveValue('Marie');
  expect((await op(page)).views).toEqual([]);
  expect((await studio.get('/api/operators')).current).toBe('Marie');
  await page.locator('#opSelect').selectOption('Défaut');
  await expect.poll(async () => (await op(page)).views.length).toBe(1);
  await page.locator('#opSelect').selectOption('Marie');
  await expect(page.locator('#opDelete')).toBeEnabled();
  page.once('dialog', d => d.accept());
  await page.locator('#opDelete').click();
  await expect(page.locator('#opSelect')).toHaveValue('Défaut');
  expect((await studio.get('/api/operators')).names).toEqual(['Défaut']);
  expect((await studio.state()).armed).toBe(false);
});

test('a damaged operator file gives the empty « Défaut » profile, nothing blocked', async ({ page }) => {
  const dir = path.join(studio.dataDir, 'operators');
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'Défaut.json'), '{ pas du json');
  writeFileSync(path.join(dir, '_current.json'), '[]');
  await opened(page);
  expect(await op(page)).toMatchObject({ name: 'Défaut', views: [] });
  await page.locator('#cues .cue').first().click();
  await expect.poll(async () => (await studio.frame() as any).cues.active.length).toBeGreaterThan(0);
});
