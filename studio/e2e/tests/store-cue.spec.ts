// T-405: « Enregistrer dans une case », user pages and grid editing.
// Preview only: nothing here arms the laser.
import { test, expect, useStudio, openUi, focusPage, type Point } from '../studio';

const studio = useStudio();
type Grid = { page: number; pages: { name: string; cells: (string | null)[]; user: boolean }[] };
const getGrid = () => studio.get<Grid>('/api/grid');
const tab = (page: import('@playwright/test').Page, i: number) => page.locator('#cueTabs button').nth(i);
const cell = (page: import('@playwright/test').Page, i: number) => page.locator('#cues [data-slot]').nth(i);
const activeCue = async () => (await studio.controlValues()).active_cue;
// The frame, rounded, so two renders of the same still look compare equal.
const frame = async () => (await studio.frame()).points.map((p: Point) => p.map(v => Math.round(v * 1000) / 1000).join(',')).join(' ');
const look = async (over: Record<string, unknown>) =>
  expect(await studio.post('/api/settings', { ...(await studio.state()).settings, rotation_speed: 0, brightness: 1, ...over })).toBe(200);

/** Adds a user page through the API; returns its index. */
async function addPage(name: string) {
  const r = await fetch(studio.url + '/api/grid/pages', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ op: 'add', name }) });
  expect(r.status).toBe(200);
  return (await r.json() as Grid).page;
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

test('a stored look replays identically, also after a restart', async ({ page }) => {
  const n = await addPage('Mes looks');
  await page.reload();
  await expect(tab(page, n).locator('.pn')).toHaveText('Mes looks (0)');
  await tab(page, n).click();
  await look({ content: { kind: 'shape', shape: 'square' }, color: [255, 0, 255], scale: 0.5 });
  // Magenta on screen: the new look is the one rendered.
  await expect.poll(async () => (await studio.frame()).points.some(p => p[2] > 0.3 && p[4] > 0.3 && p[3] === 0)).toBe(true);
  const stored = await frame();

  await page.locator('#cueStore').click();
  await expect(page.locator('#cueStore')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#cues')).toHaveClass(/storing/);
  await cell(page, 2).click();
  await expect(cell(page, 2)).toHaveClass(/cue/);
  await expect(cell(page, 2).locator('.n')).toHaveText('Cue 1');
  await expect(page.locator('#cues')).not.toHaveClass(/storing/);
  await expect(page.locator('#cueStoreMsg')).toContainText('Look enregistré');
  const g = await getGrid();
  expect(g.pages[n].cells[2]).toBe('u-1');
  expect(g.pages[n].user).toBe(true);
  expect(await activeCue()).toBeNull();

  // Another look, then the cell: the stored one comes back.
  await look({ content: { kind: 'shape', shape: 'circle' }, color: [0, 255, 0], scale: 0.3 });
  await expect.poll(frame).not.toBe(stored);
  await cell(page, 2).click();
  await expect.poll(activeCue).toBe('u-1');
  await expect.poll(frame).toBe(stored);

  // After a restart: same cell, same frame.
  await studio.restart();
  await openUi(page, studio);
  // The look brightness is the operator's, for every cue (as before T-405):
  // the same as when it was stored.
  await look({ brightness: 1 });
  await tab(page, n).click();
  await expect(cell(page, 2).locator('.n')).toHaveText('Cue 1');
  await cell(page, 2).click();
  await expect.poll(activeCue).toBe('u-1');
  await expect.poll(frame).toBe(stored);
  expect((await studio.frame()).armed).toBe(false);
});

test('a user cue is dragged to another page, renamed and deleted', async ({ page }) => {
  const a = await addPage('A');
  const b = await addPage('B');
  expect(await studio.post('/api/cues/store', { page: a, slot: 0, name: 'Mon look' })).toBe(200);
  await page.reload();
  await tab(page, a).click();
  await expect(cell(page, 0).locator('.n')).toHaveText('Mon look');
  const id = (await getGrid()).pages[a].cells[0]!;

  await page.locator('#gridMenuBtn').click();
  await page.locator('#gridEdit').click();
  await cell(page, 0).dragTo(tab(page, b));
  await expect.poll(async () => (await getGrid()).pages[b].cells[0]).toBe(id);
  expect((await getGrid()).pages[a].cells[0]).toBeNull();
  await page.locator('#gridEdit').click();
  await page.locator('#gridMenuBtn').click();

  // « Propriétés du cue »: rename, then delete (confirmed).
  await tab(page, b).click();
  await cell(page, 0).click({ button: 'right' });
  await expect(page.locator('#cueMenuUser')).toBeVisible();
  await expect(page.locator('#cueMenuToUser')).toBeHidden();
  await page.locator('#cueMenuNameInput').fill('Final');
  await page.locator('#cueMenuNameInput').press('Enter');
  await expect(cell(page, 0).locator('.n')).toHaveText('Final');
  await page.locator('#cueMenuClose').click();
  await cell(page, 0).click({ button: 'right' });
  page.once('dialog', d => d.accept());
  await page.locator('#cueMenuDelete').click();
  await expect(cell(page, 0)).toHaveClass(/cue-empty/);
  const presets = await studio.presets();
  expect(presets.presets.some(p => p.id === id)).toBe(false);
});

test('a built-in cue is never overwritten: a copy is offered instead', async ({ page }) => {
  const tunnels = (await getGrid()).pages[1].cells[0]!;
  await tab(page, 1).click();
  await focusPage(page);
  await page.keyboard.press('Control+e');
  await expect(page.locator('#cueStore')).toHaveAttribute('aria-pressed', 'true');
  await cell(page, 0).click();
  await expect(page.locator('#cueStoreText')).toHaveText('Les cues intégrés ne peuvent pas être remplacés.');
  await expect(page.locator('#cueStoreCopy')).toBeVisible();
  expect(await activeCue()).toBeNull();
  await page.locator('#cueStoreCopy').click();
  // An editable copy on a new user page, shown; the built-in cell is untouched.
  await expect(page.locator('#cueTabs button.active .pn')).toHaveText('Mes cues (1)');
  const g = await getGrid();
  const mine = g.pages.findIndex(p => p.name === 'Mes cues');
  expect(g.pages[mine].user).toBe(true);
  expect(g.pages[mine].cells[0]).toMatch(/^u-\d+$/);
  expect(g.pages[1].cells[0]).toBe(tunnels);
  await expect(page.locator('#cueStore')).toHaveAttribute('aria-pressed', 'false');
});

test('in show mode every store and grid edit is refused', async ({ page }) => {
  const n = await addPage('Mes looks');
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  await expect(page.locator('#cueStore')).toBeHidden();
  await expect(page.locator('#cuePageAdd')).toBeHidden();
  expect(await studio.post('/api/cues/store', { page: n, slot: 0 })).toBe(423);
  expect(await studio.post('/api/cues/update', {})).toBe(423);
  expect(await studio.post('/api/grid/edit', { op: 'copy_to_user', from: [1, 0] })).toBe(423);
  expect(await studio.post('/api/grid/pages', { op: 'add' })).toBe(423);
  await focusPage(page);
  await page.keyboard.press('Control+e');
  await expect(page.locator('#cueStore')).toHaveAttribute('aria-pressed', 'false');
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
  expect((await getGrid()).pages[n].cells.every(c => c === null)).toBe(true);
});
