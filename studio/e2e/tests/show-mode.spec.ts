// Show mode (T-283): the padlock locks editing (server answers 423, the UI
// hides the editing panels) while cues, Space, Escape and Shift+Escape keep
// working. A single click never unlocks; a 1 s hold does. Preview only:
// "armed" is just a flag in a studio that has no output.
import { test, expect, useStudio, openUi, focusPage, type Page } from '../studio';

const studio = useStudio();
const locked = async () => (await studio.get('/api/lock')).show_mode as boolean;
const armed = async () => (await studio.state()).armed as boolean;
const scenes = async () => ((await studio.state()).scenes as { name: string }[]).map(s => s.name);

async function holdLock(page: Page, ms: number) {
  await page.locator('#lockBtn').hover();
  await page.mouse.down();
  await page.waitForTimeout(ms);
  await page.mouse.up();
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  for (const name of await scenes()) await studio.post('/api/scenes/delete', { name });
  await studio.post('/api/scenes/save', { name: 'Garde', duration_secs: 8 });
  await openUi(page, studio);
});

test('locked: edits get 423, cues, Space and Escape still work', async ({ page }) => {
  await page.locator('#lockBtn').click();
  await expect.poll(locked).toBe(true);
  await expect(page.locator('#lockBtn')).toHaveClass(/on/);
  await expect(page.locator('#lockLabel')).toHaveText('Verrouillé');
  // Only the playing panels stay on screen.
  await expect(page.locator('#wsTabs [data-ws-tab="creation"]')).toBeHidden();
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toBeHidden();
  await expect(page.locator('#liveTabs [data-panel-tab="scenes"]')).toBeHidden();
  await expect(page.locator('#liveTabs [data-panel-tab="direct"]')).toBeVisible();
  await focusPage(page);
  await page.keyboard.press('F4');
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/);

  // The server refuses the edit, whoever asks; the scene is still there.
  expect(await studio.post('/api/scenes/delete', { name: 'Garde' })).toBe(423);
  expect(await scenes()).toEqual(['Garde']);
  // A cue right-click (properties) is refused with the French message.
  const cue = (await studio.presets()).presets[1];
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click({ button: 'right' });
  await expect(page.locator('#cueMenu')).toBeHidden();
  await expect(page.locator('#lockToast')).toHaveText('Verrouillé en mode spectacle');

  // Playing still works.
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(cue.id);
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Shift+Escape');
  await expect.poll(armed).toBe(false);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  expect((await studio.state()).estop).toBe(true);
  expect(await locked()).toBe(true);

  // Entering is in the safety log.
  const kinds = (await studio.get('/api/safety/log')).events.map((e: { kind: string }) => e.kind);
  expect(kinds).toContain('show_mode');
});

test('a single click on the padlock does not unlock; a 1 s hold does', async ({ page }) => {
  await page.locator('#lockBtn').click();
  await expect.poll(locked).toBe(true);
  await page.locator('#lockBtn').click();
  await expect(page.locator('#lockToast')).toHaveText('Maintenir pour déverrouiller');
  await holdLock(page, 400); // let go too early
  expect(await locked()).toBe(true);
  expect(await armed()).toBe(false);

  await holdLock(page, 1300);
  await expect.poll(locked).toBe(false);
  await expect(page.locator('#lockBtn')).not.toHaveClass(/on/);
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toBeVisible();
  expect(await armed()).toBe(false); // unlocking never arms
  expect(await studio.post('/api/scenes/delete', { name: 'Garde' })).toBe(200);
});

test('the lock is kept across a restart and another page follows it', async ({ page, context }) => {
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  // An open page follows a lock made elsewhere.
  await expect(page.locator('#lockBtn')).toHaveClass(/on/);
  await studio.restart();
  expect(await locked()).toBe(true);
  expect(await armed()).toBe(false);
  const other = await context.newPage();
  await openUi(other, studio);
  await expect(other.locator('#lockBtn')).toHaveClass(/on/);
  await expect(other.locator('#wsTabs [data-ws-tab="creation"]')).toBeHidden();
  await other.close();
});
