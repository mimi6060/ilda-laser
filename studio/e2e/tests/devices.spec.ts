// Sorties (T-303, T-401): RÉGLAGES › Sorties lists what can be driven. The
// studio runs with --test-devices (a fixed list): no scan ever reaches the
// network or USB, and a scan connects nothing.
import { test, expect, useStudio, openUi, openWorkspace, FAKE_OUTPUT } from '../studio';

const studio = useStudio({ devices: [{ kind: 'helios', id: 'helios:left', name: 'Helios DAC Left (1:2.3)' }] });

test('nothing is scanned until Rechercher, then the list shows the DAC', async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  // Before any scan: only the fake output this studio started with.
  await expect(page.locator('#devList li')).toHaveCount(1);
  await expect(page.locator('#devList li[data-id="test"] .dev-state')).toHaveText('Connectée');
  await expect(page.locator('#devList li[data-id="test"] .dev-connect')).toBeHidden();
  await expect(page.locator('#outState')).toHaveText(`Connectée : ${FAKE_OUTPUT}`);
  expect((await studio.get('/api/devices')).scanned).toBe(false);
  await page.locator('#devScan').click();
  await expect(page.locator('#devList li')).toHaveCount(2);
  const helios = page.locator('#devList li[data-id="helios:left"]');
  await expect(helios.locator('.dev-kind')).toHaveText('Helios USB');
  await expect(helios.locator('.dev-name')).toHaveText('Helios DAC Left (1:2.3)');
  await expect(helios.locator('code')).toHaveText('helios:left');
  await expect(helios.locator('.dev-connect')).toBeVisible();
  await expect(page.locator('#devScan')).toBeEnabled();
  await expect(page.locator('#outputsPanel')).toContainText('--device <id>');
  // A scan connects nothing: still the fake output, disarmed.
  const st = await studio.state();
  expect(st.output).toBe(FAKE_OUTPUT);
  expect(st.armed).toBe(false);
});
