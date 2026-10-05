// T-433: closing or reloading the page with unsaved work asks first
// (the browser's own beforeunload question); saved, it asks nothing. A
// timeline show edited and not saved counts too; GET
// /api/project/quit-check says what quitting would lose, and the quit
// question is « Modifications non enregistrées » (#projAsk), where Échap
// stays the emergency stop. Preview only, temporary --data-dir.
import type { Page } from '@playwright/test';
import { test, expect, useStudio, openUi, focusPage } from '../studio';

const studio = useStudio();

declare function askBeforeQuit(): Promise<boolean>;

/** Reloads the page and returns the dialogs it raised (accepted). */
async function reloadCounting(page: Page): Promise<string[]> {
  const seen: string[] = [];
  const onDialog = (d: { type(): string }) => { seen.push(d.type()); };
  page.on('dialog', onDialog);
  await page.reload();
  page.off('dialog', onDialog);
  return seen;
}
const quitCheck = () => studio.get('/api/project/quit-check');

test.beforeAll(async () => {
  // A named project: « Enregistrer » saves in place.
  expect(await studio.post('/api/project/save-as', { path: 'Garde' })).toBe(200);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await openUi(page, studio);
});

test('a change asks before reloading, even right after it; saved, nothing is asked', async ({ page }) => {
  expect((await quitCheck()).modified).toBe(false);
  // Drag the top bar's master (also the user gesture the browser needs) and
  // reload at once. A drag, not a click: a click on a slider waits for the
  // double-click window before it sends (T-431).
  const box = (await page.locator('#topMaster').boundingBox())!;
  await page.mouse.move(box.x + box.width - 4, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 10, box.y + box.height / 2, { steps: 4 });
  await page.mouse.up();
  expect(await reloadCounting(page)).toEqual(['beforeunload']);
  await openUi(page, studio);
  await expect.poll(async () => (await quitCheck()).modified).toBe(true);
  await expect(page.locator('#projName')).toHaveText('Garde •');
  // Cmd+S, then reload: no question.
  await focusPage(page);
  await page.keyboard.press('ControlOrMeta+s');
  await expect(page.locator('#projName')).toHaveText('Garde');
  await expect.poll(async () => (await quitCheck()).modified).toBe(false);
  expect(await reloadCounting(page)).toEqual([]);
});

test('a timeline show edited and not saved also asks', async ({ page }) => {
  expect(await studio.post('/api/timeline/new', { name: 'Garde show', time_base: 'seconds' })).toBe(200);
  // The new show's file is part of the project: saved, so only the edit below is unsaved.
  expect(await studio.post('/api/project/save', {})).toBe(200);
  const show = (await studio.get('/api/timeline')).show;
  show.markers = [{ id: 1, at: 2, label: 'A' }];
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
  await expect.poll(async () => (await quitCheck()).unsaved).toEqual(['timeline']);
  await focusPage(page); // a gesture first
  expect(await reloadCounting(page)).toEqual(['beforeunload']);
  await openUi(page, studio);
  expect(await studio.post('/api/timeline/save', {})).toBe(200);
  expect(await studio.post('/api/timeline/stop', {})).toBe(200);
  expect(await studio.post('/api/project/save', {})).toBe(200);
  expect((await quitCheck()).modified).toBe(false);
});

test('quitting asks « Enregistrer / Ne pas enregistrer / Annuler »; Échap stops and leaves it open; Entrée saves', async ({ page }) => {
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 133 })).toBe(200);
  await expect.poll(async () => (await quitCheck())).toMatchObject({ modified: true, untitled: false, unsaved: ['projet'], name: 'Garde' });
  await page.evaluate(() => { (window as unknown as { quitAnswer: Promise<boolean> }).quitAnswer = askBeforeQuit(); });
  const ask = page.locator('#projAsk');
  await expect(ask).toBeVisible();
  await expect(ask.locator('#projAskWhat li')).toHaveText(['Projet « Garde »']);
  await expect(ask.locator('button')).toHaveText(['Enregistrer', 'Ne pas enregistrer', 'Annuler']);
  // Échap: the emergency stop, the question stays.
  await page.keyboard.press('Escape');
  await expect(page.locator('#estopBanner')).toBeVisible();
  await expect.poll(async () => (await studio.get('/api/arm')).estop !== null).toBe(true);
  await expect(ask).toBeVisible();
  // Entrée = « Enregistrer »: saved, quitting may go on.
  await page.keyboard.press('Enter');
  await expect(ask).toBeHidden();
  expect(await page.evaluate(() => (window as unknown as { quitAnswer: Promise<boolean> }).quitAnswer)).toBe(true);
  expect((await quitCheck()).modified).toBe(false);
  expect((await studio.get('/api/project')).modified).toBe(false);
  // « Annuler » stays.
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 127 })).toBe(200);
  await expect.poll(async () => (await quitCheck()).modified).toBe(true);
  await page.evaluate(() => { (window as unknown as { quitAnswer: Promise<boolean> }).quitAnswer = askBeforeQuit(); });
  await page.locator('#askCancel').click();
  expect(await page.evaluate(() => (window as unknown as { quitAnswer: Promise<boolean> }).quitAnswer)).toBe(false);
  expect((await quitCheck()).modified).toBe(true);
  expect((await studio.state()).armed).toBe(false);
});

test('« Nouveau » over a modified project asks the same question', async ({ page }) => {
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 111 })).toBe(200);
  await expect(page.locator('#projName')).toHaveText('Garde •');
  await page.locator('#projBtn').click();
  await page.locator('[data-proj="new"]').click();
  await expect(page.locator('#projAsk')).toBeVisible();
  await expect(page.locator('#projAskWhat li')).toHaveText(['Projet « Garde »']);
  await page.locator('#askCancel').click();
  await expect(page.locator('#projAsk')).toBeHidden();
  await expect(page.locator('#projName')).toHaveText('Garde •');
});
