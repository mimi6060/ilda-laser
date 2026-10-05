// Ligne de commande (T-424): Cmd+Maj+K opens the search box in « commande »
// mode. A typed line drives a control (clamped), a cue, the page, the tempo
// or the stops, through POST /api/command; arming is refused, Échap stays
// the e-stop with the palette open, ↑ recalls the last lines. Preview only.
import { test, expect, useStudio, openUi, focusPage, openWorkspace, type Workspace } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
type Grid = { cols: number; rows: number; pages: { name: string; cells: (string | null)[] }[] };

async function openCommand(page: Page) {
  await focusPage(page);
  await page.keyboard.press('Control+Shift+K');
  await expect(page.locator('#searchBox')).toBeVisible();
  await expect(page.locator('#searchBox')).toHaveClass(/cmd/);
  await expect(page.locator('#searchInput')).toBeFocused();
  await expect(page.locator('#searchBox')).toHaveAttribute('data-ready', /\d+/);
}
async function run(page: Page, line: string) {
  await page.locator('#searchInput').fill(line);
  await page.keyboard.press('Enter');
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/control', { id: 'running.stop_all' });
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
  await page.evaluate(() => localStorage.removeItem('laserStudio.cmdHistory'));
});

test('master.size 80 sets the live size; a comma decimal and a clamp; ↑ recalls the line', async ({ page }) => {
  await openCommand(page);
  await run(page, 'master.size 80');
  await expect(page.locator('#cmdEcho')).toHaveText('✓ master.size = 80 %');
  await expect(page.locator('#searchInput')).toHaveValue('');
  expect((await studio.frame()).live.size).toBeCloseTo(0.8, 5);
  await run(page, 'master.size 80,5');
  await expect(page.locator('#cmdEcho')).toHaveText('✓ master.size = 80,5 %');
  expect((await studio.live()).size).toBeCloseTo(0.805, 5);
  await run(page, 'master.size 900');
  await expect(page.locator('#cmdEcho')).toHaveText('✓ master.size = 200 %');
  expect((await studio.live()).size).toBeCloseTo(2, 5);
  // History: newest first, by ↑ on an empty field, back with ↓.
  await page.keyboard.press('ArrowUp');
  await expect(page.locator('#searchInput')).toHaveValue('master.size 900');
  await page.keyboard.press('ArrowUp');
  await expect(page.locator('#searchInput')).toHaveValue('master.size 80,5');
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#searchInput')).toHaveValue('master.size 900');
  // The history is this browser's: still there after a reload.
  await page.reload();
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await openCommand(page);
  await page.keyboard.press('ArrowUp');
  await expect(page.locator('#searchInput')).toHaveValue('master.size 900');
  expect((await studio.frame()).armed).toBe(false);
});

test('armer is refused, the laser stays disarmed; an unknown word is named', async ({ page }) => {
  await openCommand(page);
  await run(page, 'armer');
  await expect(page.locator('#cmdErr')).toBeVisible();
  await expect(page.locator('#cmdErr')).toHaveText("Commande refusée : le laser ne s'arme qu'à l'écran ou avec Espace");
  await expect(page.locator('#cmdErr')).toHaveAttribute('data-refused', 'true');
  for (const line of ['transport.arm on', 'ALLUMER', 'safety.hold on']) {
    await run(page, line);
    await expect(page.locator('#cmdErr')).toHaveAttribute('data-refused', 'true');
  }
  expect((await studio.frame()).armed).toBe(false);
  // Space in the field types a space: it never arms either.
  await page.locator('#searchInput').fill('');
  await page.keyboard.press('Space');
  await expect(page.locator('#searchInput')).toHaveValue(' ');
  expect((await studio.frame()).armed).toBe(false);
  await run(page, 'mastr.size 80');
  await expect(page.locator('#cmdErr')).toHaveText('Mot inconnu : « mastr.size »');
  await expect(page.locator('#cmdErr')).toHaveAttribute('data-refused', 'false');
  // A refused line is not remembered.
  await page.locator('#searchInput').fill('');
  await page.keyboard.press('ArrowUp');
  await expect(page.locator('#searchInput')).toHaveValue('');
});

test('Échap with the palette open is the e-stop, then closes it', async ({ page }) => {
  await openCommand(page);
  await page.locator('#searchInput').fill('master.size');
  await page.keyboard.press('Escape');
  await expect(page.locator('#searchBox')).toBeHidden();
  await expect.poll(async () => (await studio.state()).estop).toBeTruthy();
  expect((await studio.frame()).armed).toBe(false);
});

test('noir latches the stop at once', async ({ page }) => {
  await openCommand(page);
  await run(page, 'noir');
  await expect(page.locator('#cmdEcho')).toContainText("Arrêt d'urgence");
  await expect.poll(async () => (await studio.state()).estop).toBeTruthy();
  await expect(page.locator('#estopBanner')).toBeVisible();
});

test('cue 2.14 go plays the 14th cell of page 2; tout arrêter stops it', async ({ page }) => {
  const grid = await studio.get<Grid>('/api/grid');
  const id = grid.pages[1].cells[13];
  expect(id).toBeTruthy();
  await openCommand(page);
  await run(page, 'cue 2.14 go');
  await expect(page.locator('#cmdEcho')).toContainText('cue 2.14');
  const active = async () => ((await studio.frame()) as any).cues.active.map((c: { cue: string }) => c.cue) as string[];
  await expect.poll(active).toContain(id);
  await run(page, 'tout arrêter');
  await expect(page.locator('#cmdEcho')).toHaveText('✓ Tout arrêté');
  await expect.poll(active).not.toContain(id);
  expect((await studio.frame()).armed).toBe(false);
});

test('the palette finds actions without accents: « taille » → Taille maître, « teinte » → Teinte; choosing waits for the value', async ({ page }) => {
  await openCommand(page);
  await page.locator('#searchInput').fill('taille');
  await expect(page.locator('#searchResults .sr .l', { hasText: /^Taille maître$/ })).toBeVisible();
  await page.locator('#searchInput').fill('TEINTE');
  const first = page.locator('#searchResults .sr').first();
  await expect(first.locator('.l')).toHaveText('Teinte');
  await expect(first.locator('.w code')).toHaveText('master.color.hue');
  // Fuzzy: letters in order.
  await page.locator('#searchInput').fill('tllmtr');
  await expect(page.locator('#searchResults .sr .l', { hasText: /^Taille maître$/ })).toBeVisible();
  // Choosing an action with a value fills its id and waits.
  await page.locator('#searchResults .sr', { hasText: 'Taille maître' }).first().click();
  await expect(page.locator('#searchInput')).toHaveValue('master.size ');
  await expect(page.locator('#searchInput')).toBeFocused();
  await page.keyboard.type('50');
  await page.keyboard.press('Enter');
  await expect(page.locator('#cmdEcho')).toHaveText('✓ master.size = 50 %');
  // The list shows the value now.
  await page.locator('#searchInput').fill('master.size');
  await expect(page.locator('#searchResults .sr').first().locator('.w')).toContainText('50 %');
  // Tab completes the selected action.
  await page.locator('#searchInput').fill('teinte');
  await page.keyboard.press('Tab');
  await expect(page.locator('#searchInput')).toHaveValue('master.color.hue ');
});

test('Cmd+Maj+K opens and closes the palette from every workspace, without changing the output', async ({ page }) => {
  const before = await studio.frame();
  for (const ws of ['live', 'timeline', 'creation', 'settings'] as Workspace[]) {
    await openWorkspace(page, ws);
    await openCommand(page);
    await expect(page.locator('#cmdBtn')).toHaveAttribute('aria-expanded', 'true');
    await page.keyboard.press('Control+Shift+K');
    await expect(page.locator('#searchBox')).toBeHidden();
    await expect(page.locator('#wsTabs [data-ws-tab="' + ws + '"]')).toHaveClass(/active/);
  }
  // Cmd+K is still the search, and « > » switches it to commands.
  await focusPage(page);
  await page.keyboard.press('Control+k');
  await expect(page.locator('#searchBox')).toBeVisible();
  await expect(page.locator('#searchBox')).not.toHaveClass(/cmd/);
  await page.locator('#searchInput').fill('>tempo 128');
  await expect(page.locator('#searchBox')).toHaveClass(/cmd/);
  await expect(page.locator('#searchInput')).toHaveValue('tempo 128');
  await page.keyboard.press('Enter');
  await expect(page.locator('#cmdEcho')).toHaveText('✓ Tempo = 128 BPM');
  // The button too.
  await page.locator('#cmdBtn').click();
  await expect(page.locator('#searchBox')).toBeHidden();
  await page.locator('#cmdBtn').click();
  await expect(page.locator('#searchBox')).toHaveClass(/cmd/);
  const after = await studio.frame();
  expect(after.armed).toBe(false);
  expect(after.output).toBe(before.output);
});

test('show mode: the line still plays', async ({ page }) => {
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  await openCommand(page);
  await run(page, 'tempo 100');
  await expect(page.locator('#cmdEcho')).toHaveText('✓ Tempo = 100 BPM');
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
});
