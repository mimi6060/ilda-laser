// Executor bank (T-418): LIVE › Calques › Exécuteurs. A column is
// assigned with a right click (« Affecter »); a *Fondu au curseur* fader
// starts its cue when raised from 0, the cue's level follows it, back to 0
// stops it; changing page cuts nothing and marks the page; the digits hold
// button 1; a track fader of the simulated APC40 mkII (--midi-test) learned
// on `exec.cur.1.fader` does the same as the one on screen. Preview only:
// the studio is never armed here.
import { test, expect, useStudio, openUi, reveal, focusPage, isLit, type Point } from '../studio';
import { Apc, TEST_PORT } from '../midi';

const studio = useStudio({ midiTest: true });
const apc = new Apc(studio);

interface ActiveCue { cue: string; held: boolean; level: number }
interface ExecState { page: number; levels: number[]; held: boolean[][]; active: boolean[]; active_pages: boolean[] }
const frame = async () => (await studio.frame()) as unknown as { points: Point[]; armed: boolean; cues: { active: ActiveCue[] }; executors: ExecState };
const active = async () => (await frame()).cues.active.map(a => ({ ...a, level: Math.round(a.level * 1000) / 1000 }));
const peak = (pts: Point[]) => pts.filter(isLit).reduce((m, p) => Math.max(m, p[2], p[3], p[4]), 0);
const EMPTY = { pages: [{ name: 'Page 1', handles: [] }] };
const presetId = async (i: number) => (await studio.get<{ presets: { id: string }[] }>('/api/presets')).presets[i].id;

/** A cue of the grid's first page, the way the menu lists it. */
async function firstCue(page: import('@playwright/test').Page) {
  const value = await page.locator('#execMenuTarget optgroup').first().locator('option').first().getAttribute('value');
  return JSON.parse(value!) as { type: 'cue'; id: string };
}

/** Right click on column n (1-based) → « Affecter ». */
async function openMenu(page: import('@playwright/test').Page, n: number) {
  await reveal(page, '#execBank');
  await page.locator(`#execRow .exec-col[data-n="${n}"] .t`).click({ button: 'right' });
  await expect(page.locator('#execMenu')).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/midi/learn/cancel');
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  expect(await studio.post('/api/control', { id: 'cue.stop_all' })).toBe(200);
  expect(await studio.post('/api/executors', EMPTY)).toBe(200);
  await openUi(page, studio);
});

test('Fondu au curseur: raising the fader starts the cue at its level, back to 0 stops it', async ({ page }) => {
  await openMenu(page, 1);
  const cue = await firstCue(page);
  await page.locator('#execMenuTarget').selectOption(JSON.stringify(cue));
  await expect(page.locator('#execMenuFader')).toHaveValue('fade_in');
  await expect(page.locator('#execMenuB1')).toHaveValue('flash');
  const bank = await studio.get<{ pages: { handles: { target: { type: string; id: string }; fader: string }[] }[] }>('/api/executors');
  expect(bank.pages[0].handles[0]).toMatchObject({ target: cue, fader: 'fade_in' });
  await page.locator('#execMenuClose').click();
  await expect(page.locator('#execMenu')).toBeHidden();
  const col = page.locator('#execRow .exec-col[data-n="1"]');
  await expect(col.locator('.t')).not.toHaveText('—');
  await expect(col.locator('.b1')).toHaveText('Flash');

  // Up to 100 %: the cue plays at full level.
  const fader = page.locator('#execF1');
  await expect(fader).toHaveAttribute('data-control', 'exec.cur.1.fader');
  await fader.fill('100');
  await expect.poll(async () => (await active()).map(a => [a.cue, a.level])).toEqual([[cue.id, 1]]);
  await expect(col).toHaveClass(/\bon\b/);
  let full = 0;
  await expect.poll(async () => (full = peak((await frame()).points))).toBeGreaterThan(0.1);

  // 50 %: same cue (no restart), half its light.
  await fader.fill('50');
  await expect.poll(async () => (await active()).map(a => a.level)).toEqual([0.5]);
  await expect.poll(async () => Math.abs(peak((await frame()).points) - full * 0.5) < 0.05).toBe(true);
  expect((await studio.controlValues()).values['exec.1.1.fader']).toBeCloseTo(0.5, 3);

  // Back to 0: stopped.
  await fader.fill('0');
  await expect.poll(async () => (await active()).length).toBe(0);
  expect((await frame()).armed).toBe(false);
});

test('a page change cuts nothing: the cue keeps playing, its tab is marked, the fader comes back to its value', async ({ page }) => {
  const cue = await presetId(0);
  expect(await studio.post('/api/executors', { pages: [{ name: 'Beams', handles: [{ target: { type: 'cue', id: cue }, fader: 'fade_in', buttons: ['flash', 'black_held'] }] }] })).toBe(200);
  await reveal(page, '#execBank');
  await expect(page.locator('#execTabs [role=tab]')).toHaveText(['Beams']);
  await page.locator('#execF1').fill('70');
  await expect.poll(async () => (await active()).map(a => a.level)).toEqual([0.7]);

  // « + Page »: a second page, shown at once; page 1's cue still plays.
  await page.locator('#execPageAdd').click();
  await expect(page.locator('#execTabs [role=tab]')).toHaveText(['Beams', 'Page 2']);
  await expect.poll(async () => (await frame()).executors.page).toBe(1);
  await expect(page.locator('#execTabs [role=tab]').nth(1)).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('#execTabs [role=tab]').first()).toHaveClass(/\blive\b/);
  await expect(page.locator('#execF1')).toHaveValue('0');
  expect((await active()).map(a => a.cue)).toEqual([cue]);

  // Back to page 1 (Alt + Page précédente): the fader shows 70 % again.
  await focusPage(page);
  await page.keyboard.press('Alt+PageUp');
  await expect.poll(async () => (await frame()).executors.page).toBe(0);
  await expect(page.locator('#execF1')).toHaveValue('70');

  // Noir tenu (button 2), held with the pointer: dark while held.
  const b2 = page.locator('#execRow .exec-col[data-n="1"] .b2');
  await expect(b2).toHaveText('Noir tenu');
  await b2.hover();
  await page.mouse.down();
  await expect.poll(async () => (await active()).map(a => a.level)).toEqual([0]);
  await expect.poll(async () => (await frame()).points.filter(isLit).length).toBe(0);
  await page.mouse.up();
  await expect.poll(async () => (await active()).map(a => a.level)).toEqual([0.7]);

  // Show mode: playing allowed, assigning refused.
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  expect(await studio.post('/api/executors', EMPTY)).toBe(423);
  expect(await studio.post('/api/control', { id: 'exec.cur.1.fader', value: 0 })).toBe(200);
  await expect.poll(async () => (await active()).length).toBe(0);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
  expect((await frame()).armed).toBe(false);
});

test('the digit 1 holds button 1 (Flash) for as long as the key is down', async ({ page }) => {
  const cue = await presetId(1);
  expect(await studio.post('/api/executors', { pages: [{ handles: [{ target: { type: 'cue', id: cue }, fader: 'fade_in', buttons: ['flash', 'none'] }] }] })).toBe(200);
  await reveal(page, '#execBank');
  await expect(page.locator('#execRow .exec-col[data-n="1"] .b1')).toHaveText('Flash');
  await focusPage(page);
  await page.keyboard.down('1');
  await expect.poll(async () => (await active()).map(a => [a.cue, a.held, a.level])).toEqual([[cue, true, 1]]);
  await expect(page.locator('#execRow .exec-col[data-n="1"] .b1')).toHaveClass(/\bheld\b/);
  await page.keyboard.up('1');
  await expect.poll(async () => (await active()).length).toBe(0);
  // In the shortcut table, under « Exécuteurs ».
  const list = await page.evaluate(() => (window as any).shortcutsDebug?.list?.() ?? null);
  if (list) expect(list.some((s: { id: string; group: string }) => s.id === 'exec.button' && s.group === 'Exécuteurs')).toBe(true);
});

test('MIDI: a track fader learned on exec.cur.1.fader starts and follows the cue', async ({ page }) => {
  await expect.poll(async () => {
    const d = (await studio.get<{ devices: { name: string; connected: boolean }[] }>('/api/midi')).devices.find(x => x.name === TEST_PORT);
    return d?.connected;
  }).toBe(true);
  const cue = await presetId(2);
  expect(await studio.post('/api/executors', { pages: [{ handles: [{ target: { type: 'cue', id: cue }, fader: 'fade_in', buttons: ['none', 'none'] }] }] })).toBe(200);
  // Track fader 8 (CC 7, channel 7): its default use (audio.size) is
  // replaced when learning, after the question.
  await openMenu(page, 1);
  await page.locator('#execMenuLearnF').click();
  await expect(page.locator('#execMenu')).toBeHidden();
  await expect(page.locator('#learnBanner')).toContainText('touchez un bouton, un potard ou un fader');
  await apc.moveFader(7, 0);
  await expect(page.locator('#learnBanner')).toContainText('Remplacer');
  await page.locator('#learnYes').click();
  await expect.poll(async () => (await studio.get<{ mappings: { target: string; channel: number | null }[] }>('/api/midi')).mappings.some(m => m.target === 'exec.cur.1.fader' && m.channel === 7)).toBe(true);
  // Picked up at 0, then up: the cue plays at the fader's level.
  await apc.moveFader(7, 1);
  await apc.moveFader(7, 64);
  await expect.poll(async () => (await active()).map(a => a.cue)).toEqual([cue]);
  await expect.poll(async () => Math.abs((await active())[0].level - 64 / 127) < 0.01).toBe(true);
  await apc.moveFader(7, 0);
  await expect.poll(async () => (await active()).length).toBe(0);
  expect((await frame()).armed).toBe(false);
});
