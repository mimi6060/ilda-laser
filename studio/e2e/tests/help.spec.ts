// Built-in help (T-438): the « ? » of every panel opens its page, the
// help works offline, searches without accents, never reaches outside
// 127.0.0.1, and Échap with the help open is still the emergency stop.
// Preview only.
import { test, expect, useStudio, openUi, openWorkspace, type Page } from '../studio';

const studio = useStudio();
type HelpEntry = { slug: string; title_fr: string; group_fr: string };

async function helpPages(): Promise<HelpEntry[]> {
  return studio.get<HelpEntry[]>('/api/help');
}

/** Clicks every visible « ? » and checks the page it opens; returns the slugs seen. */
async function clickVisibleHelp(page: Page, titles: Map<string, string>, seen: Set<string>) {
  const buttons = page.locator('.help-q:visible');
  const n = await buttons.count();
  for (let i = 0; i < n; i++) {
    const b = buttons.nth(i);
    const slug = (await b.getAttribute('data-help-for'))!;
    await b.click();
    await expect(page.locator('#helpDrawer')).toBeVisible();
    await expect(page.locator('#helpTitle')).toHaveText(titles.get(slug)!);
    seen.add(slug);
    await page.locator('#helpClose').click(); // the drawer covers the right-hand panels
    await expect(page.locator('#helpDrawer')).toBeHidden();
  }
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await openUi(page, studio);
});

test('the « ? » of every panel opens its own page', async ({ page }) => {
  const pages = await helpPages();
  const titles = new Map(pages.map(p => [p.slug, p.title_fr]));
  const seen = new Set<string>();
  // LIVE: each tab of the panel, the grid and the layers.
  for (const tab of ['direct', 'lfo', 'fx', 'music', 'scenes', 'pools', 'cuelist', 'macros', 'running']) {
    await page.locator(`#liveTabs [data-panel-tab="${tab}"]`).click();
    await clickVisibleHelp(page, titles, seen);
  }
  await openWorkspace(page, 'timeline');
  await clickVisibleHelp(page, titles, seen);
  await openWorkspace(page, 'creation');
  for (const tab of ['look', 'abstract', 'figures', 'objects', 'media', 'program', 'macros']) {
    await page.locator(`#creationTabs [data-ctab="${tab}"]`).click();
    await clickVisibleHelp(page, titles, seen);
  }
  // The import dialog is modal: its « ? » brings the drawer into it.
  await page.locator('#creationTabs [data-ctab="figures"]').click();
  await page.locator('#figImport').click();
  await expect(page.locator('#figImportDlg')).toBeVisible();
  await page.locator('#figImportDlg .help-q').click();
  await expect(page.locator('#helpTitle')).toHaveText('Importer un SVG ou une image');
  seen.add('creation-import');
  await page.locator('#helpClose').click();
  await page.locator('#impClose').click();
  await expect(page.locator('#figImportDlg')).toBeHidden();
  await openWorkspace(page, 'settings');
  await clickVisibleHelp(page, titles, seen);
  // A « ? » in a summary opens the help, not the panel.
  await expect(page.locator('#midiPanel')).not.toHaveAttribute('open', '');
  for (const p of pages.filter(p => p.group_fr !== 'Général')) expect(seen, p.slug).toContain(p.slug);
});

test('the table of contents, search without accents and links between pages', async ({ page }) => {
  await page.locator('#helpBtn').click();
  await page.locator('#helpMenu').getByRole('menuitem', { name: "Sommaire de l'aide" }).click();
  await expect(page.locator('#helpToc')).toContainText('Premier show avec MadMapper et le ShowNET');
  await page.locator('#helpSearch').fill('securite');
  const hits = page.locator('#helpToc button');
  await expect(hits.filter({ hasText: /^Sécurité$/ })).toHaveCount(1);
  await expect(hits.filter({ hasText: 'Sécurité laser : armer, arrêter, réinitialiser' })).toHaveCount(1);
  await page.locator('#helpSearch').fill('zzz introuvable');
  await expect(page.locator('#helpToc')).toContainText('Aucune page ne correspond');
  await page.locator('#helpSearch').fill('');
  await hits.filter({ hasText: 'Premier show avec MadMapper et le ShowNET' }).click();
  await expect(page.locator('#helpTitle')).toHaveText('Premier show avec MadMapper et le ShowNET');
  await page.locator('#helpBody a[href="#aide/securite-laser"]').first().click();
  await expect(page.locator('#helpTitle')).toHaveText('Sécurité laser : armer, arrêter, réinitialiser');
  await page.locator('#helpTocBtn').click();
  await expect(page.locator('#helpToc')).toBeVisible();
  // The shortcuts overlay (T-284) leads to the help too.
  await page.locator('#helpClose').click();
  await page.locator('#keysBtn').click();
  await page.locator('#keysOpenHelp').click();
  await expect(page.locator('#keysHelp')).toBeHidden();
  await expect(page.locator('#helpDrawer')).toBeVisible();
});

test('the help opens offline and asks nothing outside 127.0.0.1', async ({ page, context }) => {
  const urls: string[] = [];
  page.on('request', r => urls.push(r.url()));
  await page.reload();
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).helpDebug.loaded())).toBe(true);
  await context.setOffline(true);
  try {
    await page.locator('#helpBtn').click();
    await page.locator('#helpMenu').getByRole('menuitem', { name: 'Premier show avec MadMapper et le ShowNET' }).click();
    await expect(page.locator('#helpTitle')).toHaveText('Premier show avec MadMapper et le ShowNET');
    await expect(page.locator('#helpBody svg')).toBeVisible(); // our own diagram, inline
    await page.locator('#helpTocBtn').click();
    await page.locator('#helpToc button', { hasText: 'Dépannage' }).click();
    await expect(page.locator('#helpBody')).toContainText("Le laser ne s'arme pas");
  } finally {
    await context.setOffline(false);
  }
  const outside = urls.filter(u => !u.startsWith('http://127.0.0.1:') && !u.startsWith('data:') && !u.startsWith('blob:'));
  expect(outside).toEqual([]);
});

test('Échap with the help open is the emergency stop, the help stays', async ({ page }) => {
  await page.locator('#cueRegion .help-q').click();
  await expect(page.locator('#helpTitle')).toHaveText('Grille de cues');
  await page.locator('#helpSearch').focus(); // even while typing in the help
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await studio.state()).estop).toBe(true);
  await expect(page.locator('#helpDrawer')).toBeVisible();
  expect((await studio.state()).armed).toBe(false);
  await studio.post('/api/estop/reset', {});
});
