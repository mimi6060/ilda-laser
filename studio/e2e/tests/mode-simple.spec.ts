// T-530: Mode Simple is the first-launch default (a clean view with the big
// preview, a few looks, the master brightness and the top bar's safety
// buttons), with a one-click switch to Mode Pro that is remembered. The
// « Prêt à armer » assistant lists the arming blocks and each line leads to
// the control that lifts it. Preview only, no real laser, no device.
import { test, expect, useStudio, focusPage } from '../studio';

// No output: arming is blocked by « Aucune sortie connectée » (anchor
// outputsPanel), which the assistant leads to.
const studio = useStudio({ output: 'none' });
// This file checks the real default, so it does not force Pro.
test.use({ uiMode: 'default' });

const armed = async () => (await studio.state()).armed as boolean;
const content = async () => (await studio.state()).settings.content as { kind: string; shape?: string; generator?: string };

async function openSimple(page) {
  await page.goto(studio.url + '/');
  await expect(page.locator('body')).toHaveClass(/mode-simple/);
  await expect(page.locator('#stPoints')).not.toHaveText('0');
}

test('first launch opens the Simple view; the toggle switches to Pro and is remembered', async ({ page }) => {
  await openSimple(page);
  // The clean view: the big preview and the simple panel, no cue grid.
  await expect(page.locator('#simplePanel')).toBeVisible();
  await expect(page.locator('#preview')).toBeVisible();
  await expect(page.locator('#cueRegion')).not.toBeVisible();
  await expect(page.locator('#wsTabs .ws-tablist')).not.toBeVisible();
  // Switch to Pro.
  await page.locator('#modeBtn').click();
  await expect(page.locator('body')).toHaveClass(/mode-pro/);
  await expect(page.locator('#wsTabs .ws-tablist')).toBeVisible();
  await expect(page.locator('#simplePanel')).not.toBeVisible();
  await expect(page.locator('#modeBtn')).toHaveText('Mode Simple');
  // Remembered across a reload.
  await page.reload();
  await expect(page.locator('body')).toHaveClass(/mode-pro/);
  // Back to Simple, also remembered.
  await page.locator('#modeBtn').click();
  await expect(page.locator('body')).toHaveClass(/mode-simple/);
  await page.reload();
  await expect(page.locator('body')).toHaveClass(/mode-simple/);
});

test('clicking a look changes the content (shape, then generator), seen on the frame', async ({ page }) => {
  await openSimple(page);
  await page.locator('#simpleLooks [data-simple-shape="square"]').click();
  await expect.poll(async () => (await content()).shape).toBe('square');
  await expect(page.locator('#simpleLooks [data-simple-shape="square"]')).toHaveClass(/active/);
  expect((await studio.frame()).points.length).toBeGreaterThan(0);

  await page.locator('#simpleLooks [data-simple-gen="tunnel"]').click();
  await expect.poll(async () => (await content()).kind).toBe('generator');
  expect((await content()).generator).toBe('tunnel');
  expect((await studio.frame()).points.length).toBeGreaterThan(0);
});

test('the « Prêt à armer » assistant lists the block and its line leads to the control', async ({ page }) => {
  await openSimple(page);
  // Arming is refused (no output); the assistant stays with the block.
  await page.locator('#armBtn').click();
  const box = page.locator('#armRefused');
  await expect(box).toBeVisible();
  await expect(box.locator('li')).toContainText(['Aucune sortie connectée']);
  expect(await armed()).toBe(false);
  // Its « Régler » button switches to Pro and opens Sorties et projecteurs.
  await box.locator('li', { hasText: 'Aucune sortie connectée' }).locator('button.go').click();
  await expect(page.locator('body')).toHaveClass(/mode-pro/);
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toHaveClass(/active/);
  await expect(page.locator('#outputsPanel')).toBeVisible();
  // Still nothing armed: the assistant never arms, and no block was bypassed.
  expect(await armed()).toBe(false);
});

test('Simple mode never bypasses an interlock: Space is still refused', async ({ page }) => {
  await openSimple(page);
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect(page.locator('#armRefused')).toContainText('Aucune sortie connectée');
  expect(await armed()).toBe(false);
  expect(await studio.post('/api/arm', { on: true })).toBe(409);
  expect(await armed()).toBe(false);
});
