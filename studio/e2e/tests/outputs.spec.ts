// Output management (T-401): choose, connect, lose and get back an output
// from RÉGLAGES › Sorties et projecteurs. Fake outputs only: the studio
// starts with none, --test-devices lists a fake Ether Dream (« Connecter »
// opens a fake output, never a DAC), and --test-hooks lets the spec unplug
// it (POST /api/test/output). Never --device, never a laser.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi } from '../studio';

const DEV = { kind: 'etherdream', id: 'etherdream:test', name: 'Ether Dream de test' };
const studio = useStudio({ output: 'none', testHooks: true, devices: [DEV] });
const armed = async () => (await studio.state()).armed as boolean;
const lastDisarm = async () => (await studio.get('/api/arm')).last_disarm.reason as string;

test('without an output, arming is refused with « Aucune sortie connectée »', async ({ page }) => {
  await openUi(page, studio);
  await expect(page.locator('#output')).toHaveText('Aperçu seulement');
  await page.locator('#armBtn').click();
  await expect(page.locator('#armRefused')).toContainText('Aucune sortie connectée');
  expect(await armed()).toBe(false);
  expect(await studio.post('/api/arm', { on: true })).toBe(409);
  expect((await studio.get('/api/outputs')).state).toBe('available');
});

test('connect, arm, lose the output, get it back: never re-armed', async ({ page }) => {
  await openUi(page, studio);
  // The output in the top bar leads to the section.
  await page.locator('#output').click();
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toHaveClass(/active/);
  await expect(page.locator('#outState')).toHaveText('Aucune sortie : aperçu seulement');
  await page.locator('#devScan').click();
  const dev = page.locator(`#devList li[data-id="${DEV.id}"]`);
  await expect(dev.locator('.dev-kind')).toHaveText('Ether Dream');
  await dev.locator('.dev-connect').click();
  await expect(page.locator('#outState')).toHaveText(`Connectée : ${DEV.name}`);
  await expect(dev.locator('.dev-state')).toHaveText('Connectée');
  await expect(page.locator('#output')).toHaveText(`Sortie : ${DEV.name}`);
  expect(await lastDisarm()).toBe('output_changed');
  expect(await armed()).toBe(false);

  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);

  expect(await studio.post('/api/test/output', { action: 'drop' })).toBe(200);
  await expect(page.locator('#output')).toHaveText('Sortie perdue');
  await expect(page.locator('#output')).toHaveAttribute('data-kind', 'error');
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await expect(page.locator('#armInfo')).toContainText('Sortie perdue');
  await expect(page.locator('#outState')).toContainText('Sortie perdue — nouvelle tentative');
  await expect(dev.locator('.dev-state')).toHaveText('Perdue');
  expect(await armed()).toBe(false);
  expect(await lastDisarm()).toBe('output_lost');
  expect(await studio.post('/api/arm', { on: true })).toBe(409);
  // Retried with backoff while it stays away.
  await expect.poll(async () => (await studio.get('/api/outputs')).retry_in_ms, { timeout: 10_000 }).toBeGreaterThan(1000);

  expect(await studio.post('/api/test/output', { action: 'restore' })).toBe(200);
  await expect(page.locator('#outState')).toHaveText(`Connectée : ${DEV.name}`, { timeout: 20_000 });
  await expect(page.locator('#output')).toHaveText(`Sortie : ${DEV.name}`);
  // Back, but still disarmed: only the operator re-arms.
  expect(await armed()).toBe(false);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  expect(await lastDisarm()).toBe('output_lost');
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
});

test('Déconnecter goes back to preview, disarmed; auto-reconnect can be turned off', async ({ page }) => {
  await openUi(page, studio);
  await page.locator('#output').click();
  if (!(await armed())) await studio.post('/api/arm', { on: true });
  await expect.poll(armed).toBe(true);
  await page.locator('#outDisconnect').click();
  await expect(page.locator('#outState')).toHaveText('Aucune sortie : aperçu seulement');
  await expect(page.locator('#output')).toHaveText('Aperçu seulement');
  expect(await armed()).toBe(false);
  expect(await lastDisarm()).toBe('output_changed');
  await expect(page.locator('#outDisconnect')).toBeDisabled();

  await page.locator('#outAuto').uncheck();
  await expect.poll(async () => (await studio.get('/api/outputs')).auto_reconnect).toBe(false);
  await page.locator('#outAuto').check();
  await expect.poll(async () => (await studio.get('/api/outputs')).auto_reconnect).toBe(true);
});

test('the chosen output is remembered and reconnected, disarmed, at the next start', async ({ page }) => {
  expect(await studio.post('/api/outputs/select', { id: DEV.id })).toBe(200);
  await expect.poll(async () => (await studio.get('/api/outputs')).state).toBe('connected');
  const saved = JSON.parse(readFileSync(path.join(studio.dataDir, 'outputs.json'), 'utf8'));
  expect(saved.connection).toMatchObject({ selected: DEV.id, auto_reconnect: true });
  await studio.restart();
  await expect.poll(async () => (await studio.get('/api/outputs')).state).toBe('connected');
  await openUi(page, studio);
  await expect(page.locator('#output')).toHaveText(`Sortie : ${DEV.name}`);
  expect(await armed()).toBe(false);
  expect(await lastDisarm()).toBe('startup');
});
