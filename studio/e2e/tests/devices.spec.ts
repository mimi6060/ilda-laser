// DAC détectés (T-303): RÉGLAGES › Sorties lists the DACs a scan finds,
// read-only. The studio runs with --test-devices (a fixed list): no scan
// ever reaches the network or USB, nothing is opened or armed.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio({ devices: [{ kind: 'helios', id: 'helios:left', name: 'Helios DAC Left (1:2.3)' }] });

test('nothing is scanned until Rafraîchir, then the list shows the DAC', async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await expect(page.locator('#devList li')).toHaveText(['Pas encore cherché : cliquez sur Rafraîchir.']);
  expect((await studio.get('/api/devices')).scanned).toBe(false);
  await page.locator('#devScan').click();
  await expect(page.locator('#devList li')).toHaveCount(1);
  await expect(page.locator('#devList li .dev-kind')).toHaveText('Helios USB');
  await expect(page.locator('#devList li .dev-name')).toHaveText('Helios DAC Left (1:2.3)');
  await expect(page.locator('#devList li code')).toHaveText('helios:left');
  await expect(page.locator('#devScan')).toBeEnabled();
  await expect(page.locator('#outputsPanel')).toContainText('--device <id>');
  // Read-only: still preview only and disarmed.
  const st = await studio.state();
  expect(st.output).toBeNull();
  expect(st.armed).toBe(false);
});
