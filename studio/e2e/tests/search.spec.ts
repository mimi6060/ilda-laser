// Recherche globale (T-429): Cmd+K / Cmd+F opens one field that finds
// cues, pages, figures, scenes, shows, controls, RÉGLAGES sections and help
// pages, without accents. Choosing a result only shows it (workspace, page,
// highlighted cell); « Jouer » is the one action and never arms. Échap stays
// the e-stop and closes the list. Preview only.
import { test, expect, useStudio, openUi, focusPage, openWorkspace, type Workspace } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
type Grid = { cols: number; rows: number; pages: { name: string; cells: (string | null)[] }[] };
type Preset = { id: string; name: string };
const activeCues = async () => ((await studio.frame()) as unknown as { cues: { active: unknown[] } }).cues.active.length;
const fold = (t: string) => t.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

async function openSearch(page: Page, key = 'Control+k') {
  await focusPage(page);
  await page.keyboard.press(key);
  await expect(page.locator('#searchBox')).toBeVisible();
  await expect(page.locator('#searchInput')).toBeFocused();
  // The fresh index (lists fetched again) is in.
  await expect(page.locator('#searchBox')).toHaveAttribute('data-ready', /\d+/);
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('a cue of another page: Entrée goes to its page and outlines it, never plays it', async ({ page }) => {
  const grid = await studio.get<Grid>('/api/grid');
  const { presets } = await studio.get<{ presets: Preset[] }>('/api/presets');
  const byId = new Map(presets.map(p => [p.id, p]));
  // A cue of page 3 whose name is on no earlier page (the first result is then this one).
  const earlier = new Set(grid.pages.slice(0, 2).flatMap(pg => pg.cells).filter(Boolean).map(id => fold(byId.get(id!)!.name)));
  const id = grid.pages[2].cells.find(c => c && byId.has(c) && !earlier.has(fold(byId.get(c)!.name)))!;
  const cue = byId.get(id)!;
  await openSearch(page);
  await page.locator('#searchInput').fill(fold(cue.name)); // lower case, no accents
  const first = page.locator('#searchResults .sr').first();
  await expect(first).toHaveAttribute('data-kind', 'cue');
  await expect(first.locator('.l')).toHaveText(cue.name);
  await expect(first.locator('.w')).toContainText(grid.pages[2].name);
  await page.keyboard.press('Enter');
  await expect(page.locator('#searchBox')).toBeHidden();
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);
  await expect.poll(async () => (await studio.frame()).cue_page).toBe(2);
  await expect(page.locator(`#cues .cue[data-id="${id}"]`)).toHaveClass(/found/);
  expect(await activeCues()).toBe(0);
  expect((await studio.controlValues()).active_cue).toBeNull();
  // The outline goes after 2 s.
  await expect(page.locator(`#cues .cue[data-id="${id}"]`)).not.toHaveClass(/found/, { timeout: 4000 });
  expect(await activeCues()).toBe(0);
});

test('« Jouer » plays the cue through the normal press, and never arms', async ({ page }) => {
  const grid = await studio.get<Grid>('/api/grid');
  const { presets } = await studio.get<{ presets: Preset[] }>('/api/presets');
  const cue = presets.find(p => p.id === grid.pages[1].cells.find(Boolean))!;
  await openSearch(page);
  await page.locator('#searchInput').fill(cue.name);
  const row = page.locator('#searchResults .sr[data-kind="cue"]').filter({ has: page.locator('.l', { hasText: cue.name }) }).first();
  await row.locator('.play').click();
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(cue.id);
  expect((await studio.state()).armed).toBe(false);
});

test('« securite » finds the Sécurité section of RÉGLAGES and unfolds it', async ({ page }) => {
  await openWorkspace(page, 'live');
  await expect(page.locator('#safetyPanel')).not.toHaveAttribute('open', '');
  await openSearch(page);
  await page.locator('#searchInput').fill('securite');
  const row = page.locator('#searchResults .sr[data-kind="setting"]').filter({ has: page.locator('.l', { hasText: /^Sécurité$/ }) });
  await expect(row).toHaveCount(1);
  await expect(row.locator('.w')).toHaveText('RÉGLAGES');
  await row.click();
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toHaveClass(/active/);
  await expect(page.locator('#safetyPanel')).toHaveAttribute('open', '');
  await expect(page.locator('#safetyPanel')).toHaveClass(/found/);
});

test('« brume » finds the 3D settings: the view goes 3D and the drawer opens on it (T-503)', async ({ page }) => {
  await expect(page.locator('#render3d')).toBeHidden();
  await openSearch(page);
  await page.locator('#searchInput').fill('brume');
  const row = page.locator('#searchResults .sr[data-kind="setting"]').filter({ has: page.locator('.l', { hasText: /^Brume : densité$/ }) });
  await expect(row).toHaveCount(1);
  await expect(row.locator('.w')).toHaveText('Aperçu › Réglages 3D');
  await row.click();
  await expect(page.locator('#viewSeg [data-view="3d"]')).toHaveClass(/active/);
  await expect(page.locator('#beams')).toBeVisible();
  await expect(page.locator('#render3d')).toBeVisible();
  await expect(page.locator('#render3dBtn')).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('#render3d label:has(#r3hazeV)')).toHaveClass(/found/);
  // Display only: the studio is unchanged and still disarmed.
  expect((await studio.state()).armed).toBe(false);
  await page.locator('#viewSeg [data-view="2d"]').click();
  await expect(page.locator('#render3d')).toBeHidden();
});

test('newer panels: « Copie de sauvegarde », a font, « Préparer »', async ({ page }) => {
  await openSearch(page);
  await page.locator('#searchInput').fill('copie sauvegarde');
  await page.locator('#searchResults .sr[data-kind="setting"]').filter({ has: page.locator('.l', { hasText: /^Copie de sauvegarde$/ }) }).click();
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toHaveClass(/active/);
  await expect(page.locator('#backupPanel')).toHaveAttribute('open', '');
  await expect(page.locator('#backupPanel')).toHaveClass(/found/);

  await openSearch(page);
  await page.locator('#searchInput').fill('hershey double');
  const font = page.locator('#searchResults .sr[data-kind="setting"]').first();
  await expect(font.locator('.l')).toHaveText('Hershey double');
  await expect(font.locator('.w')).toContainText('CRÉATION › Look');
  await page.keyboard.press('Enter');
  await expect(page.locator('#wsTabs [data-ws-tab="creation"]')).toHaveClass(/active/);
  await expect(page.locator('#textFont')).toHaveClass(/found/);

  await openSearch(page);
  await page.locator('#searchInput').fill('prepare');
  await expect(page.locator('#searchResults .sr[data-kind="setting"] .w').first()).toHaveText('Aperçu');
});

test('a control by its French label and by its id', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await openSearch(page, 'Meta+f');
  await page.locator('#searchInput').fill('taille maitre');
  await expect(page.locator('#searchResults .sr[data-kind="control"] .l').first()).toHaveText('Taille maître');
  await page.locator('#searchInput').fill('master.size');
  const row = page.locator('#searchResults .sr[data-kind="control"]').first();
  await expect(row.locator('.l')).toHaveText('Taille maître');
  await expect(row.locator('.w')).toContainText('LIVE › Direct');
  await page.keyboard.press('Enter');
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);
  await expect(page.locator('#mSize')).toBeVisible();
  await expect(page.locator('#mSize')).toHaveClass(/found/);
});

test('Cmd+K opens from the four workspaces; nothing found is said; Cmd+K closes', async ({ page }) => {
  for (const ws of ['live', 'timeline', 'creation', 'settings'] as Workspace[]) {
    await openWorkspace(page, ws);
    await openSearch(page, ws === 'creation' ? 'Meta+k' : 'Control+k');
    await page.locator('#searchInput').fill('zzqqxx');
    await expect(page.locator('#searchResults .none')).toHaveText('Rien trouvé pour « zzqqxx »');
    await page.keyboard.press('Control+k');
    await expect(page.locator('#searchBox')).toBeHidden();
  }
  // Registered in the single table, no conflict.
  const ids = await page.evaluate(() => (window as unknown as { shortcutsDebug: { list: () => { id: string }[]; conflicts: () => unknown[] } }).shortcutsDebug.list().map(s => s.id));
  expect(ids).toContain('search');
  expect(await page.evaluate(() => (window as unknown as { shortcutsDebug: { conflicts: () => unknown[] } }).shortcutsDebug.conflicts())).toEqual([]);
});

test('Échap with the list open: the e-stop, and the list closes', async ({ page }) => {
  await openSearch(page);
  await page.locator('#searchInput').fill('tunnel');
  await page.keyboard.press('Escape');
  await expect(page.locator('#searchBox')).toBeHidden();
  await expect.poll(async () => (await studio.state()).estop).toBeTruthy();
  await expect(page.locator('#estopBanner')).toBeVisible();
});

test('a figure saved after the page loaded is found without reloading', async ({ page }) => {
  expect(await studio.post('/api/figures', { name: 'Comète bleue' })).toBe(200);
  await openSearch(page);
  await page.locator('#searchInput').fill('comete');
  await expect(page.locator('#searchResults .sr[data-kind="figure"] .l')).toHaveText(['Comète bleue']);
});

test('a cue is found by its notes and tags (T-430)', async ({ page }) => {
  const grid = await studio.get<Grid>('/api/grid');
  const id = grid.pages[0].cells.find(Boolean)!;
  expect(await studio.post('/api/cues/slot', { id, notes: 'seulement avec brouillard épais', tags: ['Ouverture'] })).toBe(200);
  await openUi(page, studio);
  await openSearch(page);
  await page.locator('#searchInput').fill('brouillard epais');
  await expect(page.locator('#searchResults .sr[data-kind="cue"]')).toHaveCount(1);
  await page.locator('#searchInput').fill('ouverture');
  await expect(page.locator('#searchResults .sr[data-kind="cue"]').first()).toBeVisible();
  await studio.post('/api/cues/slot', { id, notes: '', tags: [] });
});

test('500 cues:a query answers in well under 100 ms', async ({ page }) => {
  const ms = await page.evaluate(() => {
    const s = (window as unknown as { __search: { build: (i: object[]) => number; query: (t: string) => object[] } }).__search;
    const words = ['Tunnel', 'Vague', 'Éclair', 'Spirale', 'Étoile', 'Faisceau', 'Cône', 'Grille'];
    const items = Array.from({ length: 500 }, (_, i) => ({
      kind: 'cue', label: `${words[i % words.length]} ${['bleu', 'rouge', 'vert', 'doré'][i % 4]} ${i}`, sub: `page ${Math.floor(i / 40) + 1}`, target: { cue: 'c' + i } }));
    s.build(items);
    const queries = ['tun', 'eclair', 'etoile dore', 'cone', 'vert 4', 'page 3', 'spi rouge', 'x', 'grille', 'faisceau bleu'];
    const t0 = performance.now();
    let n = 0;
    for (let k = 0; k < 20; k++) n += s.query(queries[k % queries.length]).length;
    if (!n) throw new Error('no results');
    if (s.query('etoile').length !== 30) throw new Error('30 results at most');
    return (performance.now() - t0) / 20;
  });
  expect(ms).toBeLessThan(100);
});
