// Laser on/off: the LASER button, Space, and Escape (always a blackout,
// latched as an emergency stop since T-251).
// Preview only: "armed" is just a flag in a studio that has no output.
import { test, expect, useStudio, openUi, focusPage } from '../studio';

const studio = useStudio();
const armed = async () => (await studio.state()).armed as boolean;

test.beforeEach(async ({ page }) => {
  // Escape latches the emergency stop (T-251): clear it so each test starts armable.
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
  await openUi(page, studio);
});

test('starts disarmed, with the button and the badge saying so', async ({ page }) => {
  expect(await armed()).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await expect(page.locator('#offBadge')).toBeVisible();
  await expect(page.locator('#output')).toContainText('Aperçu seulement');
});

test('the LASER button toggles armed', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await expect(page.locator('#armBtn')).toHaveText('LASER ON');
  await expect(page.locator('#armBtn')).toHaveClass(/\bon\b/);
  await expect(page.locator('#offBadge')).toBeHidden();

  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await expect(page.locator('#offBadge')).toBeVisible();
});

test('Space toggles armed', async ({ page }) => {
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await expect(page.locator('#armBtn')).toHaveText('LASER ON');
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
});

test('Space does not arm while typing in a text field', async ({ page }) => {
  await page.locator('[data-kind="text"]').click();
  await page.locator('#text').click();
  await page.keyboard.press('Space');
  // Give the page a moment in which a wrong arm would have landed.
  await page.waitForTimeout(300);
  expect(await armed()).toBe(false);
});

test('Escape disarms after the button armed the laser', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
});

test('Escape disarms after Space armed the laser', async ({ page }) => {
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
});

test('Escape disarms even while typing in a text field', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await page.locator('[data-kind="text"]').click();
  await page.locator('#text').click();
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
});

test('Escape disarms while a slider has focus', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await page.locator('#mSize').focus();
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
});

test('Escape when already disarmed keeps it disarmed', async ({ page }) => {
  await focusPage(page);
  for (let i = 0; i < 3; i++) await page.keyboard.press('Escape');
  await page.waitForTimeout(200);
  expect(await armed()).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
});

test('Escape right after a fast Space burst always ends disarmed', async ({ page }) => {
  await focusPage(page);
  for (let i = 0; i < 5; i++) await page.keyboard.press('Space');
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  // And it stays disarmed once every in-flight request has landed.
  await page.waitForTimeout(300);
  expect(await armed()).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
});

test('Space right after clicking the LASER button toggles exactly once', async ({ page }) => {
  // The button keeps keyboard focus after a click: Space must toggle once
  // (keydown), not toggle and then also "click" the focused button.
  await page.locator('#armBtn').click();
  await expect(page.locator('#armBtn')).toHaveText('LASER ON');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(false);
  await page.waitForTimeout(300);
  expect(await armed()).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
});

// T-290: the toggle is computed from the page's local copy of "armed",
// which only changes once the POST has answered (and can be overwritten
// by an older /api/frame). Two quick Space presses both send on:true, so
// a quick "on, off" leaves the laser ON. The /api/arm round trip is slowed
// to 150 ms (a busy machine) so the race reproduces every time.
test('T-290: two quick Space presses end disarmed (on, then off)', async ({ page }) => {
  await page.route('**/api/arm', async route => {
    await new Promise(r => setTimeout(r, 150));
    await route.continue();
  });
  await focusPage(page);
  await page.keyboard.press('Space');
  await page.waitForTimeout(50); // a human double-tap
  await page.keyboard.press('Space');
  await page.waitForTimeout(500);
  expect(await armed()).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
});

// T-291: the "typing" guard treats every <input> as a text field, so after
// the operator touches a slider or a checkbox (which keeps focus), Space
// no longer turns the laser off, and cue keys / Enter / Backspace are dead
// until they click somewhere else. Escape still works.
test('T-291: Space still turns the laser off after using a slider', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await page.locator('#mSize').click(); // a mouse move on « Taille maître »
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(false);
});

test('a restarted studio always comes back disarmed', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await studio.restart();
  expect(await armed()).toBe(false);
  await openUi(page, studio);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
});

test('T-251: Escape latches the emergency stop until it is reset', async ({ page }) => {
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#estopBanner')).toBeVisible();
  // Space can't re-arm while the stop is latched.
  await page.keyboard.press('Space');
  await page.waitForTimeout(300);
  expect(await armed()).toBe(false);
  await page.locator('#estopReset').click();
  await expect(page.locator('#estopBanner')).toBeHidden();
  expect(await armed()).toBe(false); // reset never re-arms by itself
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
});

test('T-251: Shift+Escape is a plain disarm without latching', async ({ page }) => {
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Shift+Escape');
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#estopBanner')).toBeHidden();
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
});
