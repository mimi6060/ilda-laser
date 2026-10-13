// Output through an audio interface (T-498), from RÉGLAGES › Sorties.
// The test studio runs the fake output backend: the interfaces listed are
// fake ones and « Connecter » opens a stream-less output, never a sound
// card. Never --device, never a laser.
import { test, expect, useStudio, openUi, reveal } from '../studio';

const DEV = { kind: 'etherdream', id: 'etherdream:test', name: 'Ether Dream de test' };
const studio = useStudio({ output: 'none', devices: [DEV] });
const IFACE = 'Interface de test (8 canaux)';
const ID = `audio:${IFACE}`;

test('starts as an oscilloscope; laser mode only after the confirmation', async ({ page }) => {
  await openUi(page, studio);
  await page.locator('#audioOutPanel summary').evaluate(el => (el.parentElement as HTMLDetailsElement).open = true);
  await reveal(page, '#aoDevice');
  await expect(page.locator('#aoWarning')).toContainText('Seules les interfaces couplées en continu conviennent à un laser');
  await expect(page.locator('#aoMode')).toHaveValue('scope');
  await expect(page.locator(`#devList li[data-id^="audio:"]`)).toHaveCount(0);

  await page.locator('#aoDevice').selectOption(IFACE);
  // Laser mode, confirmation refused: nothing saved, back to oscilloscope.
  await page.locator('#aoMode').selectOption('laser');
  let asked = '';
  page.once('dialog', d => { asked = d.message(); d.dismiss(); });
  await page.locator('#aoSave').click();
  await expect(page.locator('#aoMode')).toHaveValue('scope');
  expect(asked).toContain('Passer en mode Laser');
  expect(asked).toContain('yeux');
  expect((await studio.get('/api/outputs/audio')).mode).toBe('scope');
  // Without the confirmation the API refuses too.
  expect(await studio.post('/api/outputs/audio', { device: IFACE, mode: 'laser' })).toBe(400);

  // Oscilloscope: saved and listed.
  await page.locator('#aoSave').click();
  await expect(page.locator('#aoMsg')).toContainText('Oscilloscope');
  const row = page.locator(`#devList li[data-id="${ID}"]`);
  await expect(row.locator('.dev-kind')).toHaveText('Audio');

  // Laser mode, confirmed.
  await page.locator('#aoMode').selectOption('laser');
  page.once('dialog', d => d.accept());
  await page.locator('#aoSave').click();
  await expect(page.locator('#aoMsg')).toContainText('Laser');
  expect((await studio.get('/api/outputs/audio')).mode).toBe('laser');

  // Connected: disarmed, settings locked until disconnected.
  await row.locator('.dev-connect').click();
  await expect(page.locator('#outState')).toContainText('Connectée : Interface audio');
  expect((await studio.state()).armed).toBe(false);
  await page.locator('#aoMode').selectOption('scope');
  await page.locator('#aoSave').click();
  await expect(page.locator('#aoMsg')).toContainText('Déconnectez');
  await page.locator('#outDisconnect').click();
  await expect(page.locator('#outState')).toHaveText('Aucune sortie : aperçu seulement');
  await page.locator('#aoSave').click();
  await expect(page.locator('#aoMsg')).toContainText('Oscilloscope');
});
