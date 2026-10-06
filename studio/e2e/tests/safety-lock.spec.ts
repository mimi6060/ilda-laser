// Safety lock (T-261): the safety officer locks the safety settings with a
// code. Locked, what loosens gets 423 (a zone removed, a higher cap) while
// what stops or tightens keeps working (Escape, disarm, « Ciel coupé », a
// tighter horizon). The code is stored hashed, never in clear; 5 wrong
// codes make it wait; deleting the file by hand (studio stopped) is the
// recovery, logged. Preview only: "armed" is just a flag, no output.
import { existsSync, readFileSync, rmSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi, openWorkspace, focusPage, type Page } from '../studio';

const studio = useStudio();
const CODE = '482916';
const BAND = [[-1, -1], [1, -1], [1, -0.6], [-1, -0.6]];
const lockFile = () => path.join(studio.dataDir, 'safety-lock.json');
const lockState = async () => (await studio.get('/api/safety/lock')) as { set: boolean; locked: boolean; wait_s: number | null };
const armed = async () => (await studio.state()).armed as boolean;

async function openSafety(page: Page) {
  await openWorkspace(page, 'settings');
  if ((await page.locator('#safetyPanel').getAttribute('open')) === null) await page.locator('#safetyPanel > summary').click();
  await expect(page.locator('#slkStatus')).not.toBeEmpty();
}

test.beforeEach(async ({ page }) => {
  // The documented recovery: studio stopped, file deleted, restarted.
  if ((await lockState()).set) {
    await studio.stop();
    rmSync(lockFile(), { force: true });
    await studio.start();
  }
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  const { settings } = await studio.get('/api/safety');
  settings.zones = [{ id: 0, name: 'Bas', kind: 'blank', level: 0, points: BAND }];
  settings.horizon.y = 0.2;
  expect(await studio.post('/api/safety', { ...settings, confirm_loosen: true })).toBe(200);
  await openUi(page, studio);
});

test('lock with a code: loosening refused, stopping and tightening allowed, unlock with the code', async ({ page }) => {
  await openSafety(page);
  await expect(page.locator('#slkStatus')).toHaveText('Aucun code : les réglages de sécurité sont modifiables.');
  await page.locator('#slkLockBtn').click();
  await expect(page.locator('#slkDialog')).toBeVisible();
  await page.locator('#slkCode').fill(CODE);
  await page.locator('#slkCode2').fill('000000');
  await page.locator('#slkOk').click();
  await expect(page.locator('#slkError')).toHaveText('Les deux codes diffèrent.');
  await page.locator('#slkCode').fill(CODE);
  await page.locator('#slkCode2').fill(CODE);
  await page.locator('#slkOk').click();
  await expect(page.locator('#slkDialog')).toBeHidden();
  await expect.poll(async () => (await lockState()).locked).toBe(true);
  await expect(page.locator('body')).toHaveClass(/safety-locked/);
  await expect(page.locator('#slkLed')).toBeVisible();
  await expect(page.locator('#safetyPanel')).toHaveAttribute('title', 'Verrouillé par le responsable sécurité');
  await expect(page.locator('#slkUnlockBtn')).toBeVisible();
  // Hashed and salted on disk: never the code.
  const file = readFileSync(lockFile(), 'utf8');
  expect(file).not.toContain(CODE);
  expect(JSON.parse(file)).toMatchObject({ hash: expect.stringMatching(/^[0-9a-f]{64}$/), salt: expect.stringMatching(/^[0-9a-f]{32}$/) });

  // Loosening: the zone removed, a higher cap, the calibration: 423.
  const { settings } = await studio.get('/api/safety');
  expect(await studio.post('/api/safety', { ...settings, zones: [], confirm_loosen: true })).toBe(423);
  expect(await studio.post('/api/outputs/limits', { limits: { max_power: 1, max_color: [1, 1, 1] }, confirm_loosen: true })).toBe(423);
  expect(await studio.post('/api/calibration', { offset_x: 0.5 })).toBe(423);
  expect((await studio.get('/api/safety')).settings.zones).toHaveLength(1);
  // Tightening and stopping: allowed.
  expect(await studio.post('/api/safety', { ...settings, horizon: { ...settings.horizon, y: 0.4 } })).toBe(200);
  expect(await studio.post('/api/outputs/limits', { limits: { max_power: 0.3, max_color: [1, 1, 1] } })).toBe(200);
  expect(await studio.post('/api/safety/sky', { on: true, source: 'ui' })).toBe(200);
  expect(await studio.post('/api/safety/sky', { on: false, source: 'ui' })).toBe(423);
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  expect((await studio.state()).estop).toBe(true);
  expect(await studio.post('/api/estop/reset', {})).toBe(200);

  // Unlock with the code: edits run again.
  await openSafety(page);
  await page.locator('#slkUnlockBtn').click();
  await page.locator('#slkCode').fill(CODE);
  await page.locator('#slkOk').click();
  await expect(page.locator('#slkDialog')).toBeHidden();
  await expect.poll(async () => (await lockState()).locked).toBe(false);
  await expect(page.locator('body')).not.toHaveClass(/safety-locked/);
  await expect(page.locator('#slkStatus')).toContainText('reverrouillage dans 10 min');
  expect(await studio.post('/api/safety/sky', { on: false, source: 'ui' })).toBe(200);
  expect(await studio.post('/api/safety', { ...settings, zones: [], confirm_loosen: true })).toBe(200);
  // Lock again in one click, then remove the code (unlocked first).
  await page.locator('#slkLockBtn').click();
  await expect.poll(async () => (await lockState()).locked).toBe(true);
  expect(await studio.post('/api/safety/unlock', { code: CODE })).toBe(200);
  await expect(page.locator('#slkRemoveBtn')).toBeVisible();
  await page.locator('#slkRemoveBtn').click();
  await expect.poll(async () => (await lockState()).set).toBe(false);
  expect(existsSync(lockFile())).toBe(false);
});

test('five wrong codes make it wait; Escape stays the e-stop in the dialog', async ({ page }) => {
  expect(await studio.post('/api/safety/lock', { action: 'set', code: CODE })).toBe(200);
  await openSafety(page);
  await page.locator('#slkUnlockBtn').click();
  for (let i = 0; i < 4; i++) {
    await page.locator('#slkCode').fill('1111');
    await page.locator('#slkOk').click();
    await expect(page.locator('#slkError')).toContainText('Code incorrect');
    await expect(page.locator('#slkCode')).toHaveValue('');
  }
  await page.locator('#slkCode').fill('1111');
  await page.locator('#slkOk').click();
  await expect(page.locator('#slkError')).toHaveText('Code incorrect : attendez 60 s');
  // Even the right code waits now.
  await page.locator('#slkCode').fill(CODE);
  await page.locator('#slkOk').click();
  await expect(page.locator('#slkError')).toContainText('Trop d\'essais');
  expect((await lockState()).locked).toBe(true);
  expect((await lockState()).wait_s).toBeGreaterThan(50);
  // Never from a controller, whatever the code.
  expect(await studio.post('/api/safety/unlock', { code: CODE, source: 'midi' })).toBe(403);
  // Escape in the code field: the e-stop, not « close ».
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await page.locator('#slkCode').focus();
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  expect((await studio.state()).estop).toBe(true);
  await expect(page.locator('#slkDialog')).toBeVisible();
  await page.locator('#slkCancel').click();
  await expect(page.locator('#slkDialog')).toBeHidden();
  expect(await studio.post('/api/estop/reset', {})).toBe(200);
});

test('a restart keeps it locked; the file deleted by hand starts unlocked, logged', async () => {
  expect(await studio.post('/api/safety/lock', { action: 'set', code: CODE })).toBe(200);
  await studio.restart();
  expect(await lockState()).toMatchObject({ set: true, locked: true });
  expect(await studio.post('/api/safety', (await studio.get('/api/safety')).settings)).toBe(200);
  const { settings } = await studio.get('/api/safety');
  expect(await studio.post('/api/safety', { ...settings, zones: [], confirm_loosen: true })).toBe(423);
  await studio.stop();
  rmSync(lockFile());
  await studio.start();
  expect(await lockState()).toMatchObject({ set: false, locked: false });
  const log = await studio.get<{ events: { kind: string; detail: { event?: string }; text: string }[] }>('/api/safety/log');
  const kinds = log.events.filter(e => e.kind === 'safety_lock').map(e => e.detail.event);
  expect(kinds).toEqual(expect.arrayContaining(['code_set', 'file_removed']));
  expect(JSON.stringify(log)).not.toContain(CODE);
});
