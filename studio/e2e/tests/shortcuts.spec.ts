// Keyboard shortcuts (T-284): one table drives the keys and the « ? »
// help. No two entries share a key press in the same place; Échap stays
// first, even from a text field; nothing else fires while typing. Preview
// only: "armed" is just a flag in a studio that has no output.
import { test, expect, useStudio, openUi, focusPage, openWorkspace } from '../studio';

const studio = useStudio();
const armed = async () => (await studio.state()).armed as boolean;
const activeCue = async () => (await studio.controlValues()).active_cue;
type Conflict = { a: string; b: string; keys: string[] };
const conflicts = (page: import('@playwright/test').Page, extra: object[] = []) =>
  page.evaluate(x => (window as unknown as { shortcutsDebug: { conflicts: (e: object[]) => Conflict[] } }).shortcutsDebug.conflicts(x), extra);

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('the table has no conflicts, and a duplicate is caught', async ({ page }) => {
  expect(await conflicts(page)).toEqual([]);
  const list = await page.evaluate(() => (window as unknown as { shortcutsDebug: { list: () => { id: string; group: string; show: boolean }[] } }).shortcutsDebug.list());
  expect(list[0].id).toBe('estop');
  expect(list[1].id).toBe('disarm');
  expect(new Set(list.map(s => s.id)).size).toBe(list.length);
  // Same key, same place: a second Espace, Alt+S in LIVE, Cmd+Z in TIMELINE,
  // a character on a cue position (AZERTY « a » = KeyQ).
  const dupes = [
    { id: 'dup.space', codes: ['Space'], zones: '*' },
    { id: 'dup.alts', codes: ['KeyS'], alt: true, zones: ['live'] },
    { id: 'dup.undo', keys: ['z'], meta: true, zones: ['timeline'] },
    { id: 'dup.letter', keys: ['a'], shift: '*', zones: ['live'] },
  ];
  for (const d of dupes) {
    const found = await conflicts(page, [d]);
    expect(found.length, d.id).toBeGreaterThan(0);
    expect(found.every(c => c.b === d.id), d.id).toBe(true);
  }
  // Different place or modifiers: no clash.
  expect(await conflicts(page, [{ id: 'ok.creation', codes: ['KeyQ'], zones: ['creation'] }])).toEqual([]);
  expect(await conflicts(page, [{ id: 'ok.ctrl', codes: ['KeyQ'], meta: true, alt: true, zones: ['live'] }])).toEqual([]);
});

test('« ? » and the button open the help; Échap closes it AND stops', async ({ page }) => {
  await focusPage(page);
  await expect(page.locator('#keysHelp')).toBeHidden();
  await page.keyboard.press('Shift+Slash'); // « ? » on QWERTY
  await expect(page.locator('#keysHelp')).toBeVisible();
  await expect(page.locator('#keysTitle')).toHaveText('Raccourcis');
  // Grouped, safety first, built from the table.
  await expect(page.locator('#keysList .keys-group h4').first()).toHaveText('Sécurité');
  for (const g of ['Sécurité', 'Cues et pages', 'Tempo', 'Direct', 'Projet']) {
    await expect(page.locator(`#keysList .keys-group[data-group="${g}"]`)).toBeVisible();
  }
  await expect(page.locator('#keysList [data-shortcut="estop"] kbd')).toHaveText('Échap');
  await expect(page.locator('#keysList [data-shortcut="disarm"] kbd')).toHaveText(['Maj', 'Échap']);
  await expect(page.locator('#keysList [data-shortcut="arm"]')).toContainText('Armer');

  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Escape');
  await expect(page.locator('#keysHelp')).toBeHidden();
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#estopBanner')).toBeVisible();
  await studio.post('/api/estop/reset', {});

  // The button, and « ? » again, toggle it.
  await page.locator('#keysBtn').click();
  await expect(page.locator('#keysHelp')).toBeVisible();
  await page.locator('#keysClose').click();
  await expect(page.locator('#keysHelp')).toBeHidden();
  await focusPage(page);
  await page.keyboard.press('Shift+Slash');
  await expect(page.locator('#keysHelp')).toBeVisible();
  await page.keyboard.press('Shift+Slash');
  await expect(page.locator('#keysHelp')).toBeHidden();
});

test('in a text field only Échap works; a letter plays its cue outside', async ({ page }) => {
  const cat = await studio.presets();
  const cue = cat.presets.filter(p => p.category === cat.categories[0])[0];
  await focusPage(page);
  await page.locator('#bpm').focus();
  // Space, a cue letter, and « ? » are typing, not shortcuts.
  await page.keyboard.press('Space');
  await page.keyboard.press('KeyQ');
  await page.keyboard.press('Shift+Slash');
  await page.waitForTimeout(300);
  expect(await armed()).toBe(false);
  expect(await activeCue()).toBeNull();
  await expect(page.locator('#keysHelp')).toBeHidden();

  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.locator('#bpm').focus();
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#estopBanner')).toBeVisible();

  // Outside the field: the first cue key (AZERTY A, e.code KeyQ) plays cell 1.
  await focusPage(page);
  await page.keyboard.press('KeyQ');
  await expect.poll(activeCue).toBe(cue.id);
});

test('the keys follow the workspace: letters are cues only in LIVE', async ({ page }) => {
  const zone = () => page.evaluate(() => (window as unknown as { shortcutsDebug: { zone: () => string } }).shortcutsDebug.zone());
  expect(await zone()).toBe('live');
  await openWorkspace(page, 'settings');
  expect(await zone()).toBe('settings');
  await focusPage(page);
  await page.keyboard.press('KeyQ');
  await page.waitForTimeout(300);
  expect(await activeCue()).toBeNull();
  await page.keyboard.press('F1');
  await expect.poll(zone).toBe('live');
});
