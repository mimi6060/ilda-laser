// LaserCube outputs (T-496): a scan lists them as « LaserCube », with the
// reminder that the cube's own interlock is the real stop, and a LaserCube
// reached over the Wi-Fi gets the Wi-Fi pill. Fake outputs only:
// --test-devices lists the cubes (« Connecter » opens a fake output, never a
// DAC) and the Wi-Fi check is simulated (--test-hooks). The address below is
// a documentation one: nothing is ever sent to it.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const NET = { kind: 'lasercube', id: 'lasercube-network:010203040506', name: 'LaserCube 192.0.2.50', ip: '192.0.2.50' };
const USB = { kind: 'lasercube', id: 'lasercube-usb:LC-1', name: 'LaserCube USB LC-1' };
const studio = useStudio({ output: 'none', testHooks: true, devices: [NET, USB] });

test('LaserCubes are listed with their kind and the hardware-stop reminder', async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await expect(page.locator('#lcHelp')).toBeHidden();
  await page.locator('#devScan').click();
  for (const d of [NET, USB]) {
    const li = page.locator(`#devList li[data-id="${d.id}"]`);
    await expect(li.locator('.dev-kind')).toHaveText('LaserCube');
    await expect(li.locator('.dev-name')).toHaveText(d.name);
  }
  await expect(page.locator('#lcHelp')).toBeVisible();
  await expect(page.locator('#lcHelp')).toContainText('interlock matériel');
  const st = await studio.state();
  expect(st.armed).toBe(false);
});

test('a LaserCube reached over the Wi-Fi shows the Wi-Fi pill, never arms', async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  expect(await studio.post('/api/test/wifi', { wireless: true, interface: 'en0' })).toBe(200);
  await expect(page.locator('#wifiPill')).toBeHidden();
  await page.locator('#devScan').click();
  await page.locator(`#devList li[data-id="${NET.id}"] .dev-connect`).click();
  await expect(page.locator('#outState')).toHaveText(`Connectée : ${NET.name}`);
  await expect(page.locator('#wifiPill')).toBeVisible();
  await expect(page.locator('#wifiPill')).toHaveAttribute('title', /LaserCube.*préférez l'Ethernet/);
  const st = await studio.state();
  expect(st.shownet.wifi_device).toBe('lasercube');
  expect(st.armed).toBe(false);
  // The USB cube has no network link: no pill.
  await page.locator(`#devList li[data-id="${USB.id}"] .dev-connect`).click();
  await expect(page.locator('#outState')).toHaveText(`Connectée : ${USB.name}`);
  await expect(page.locator('#wifiPill')).toBeHidden();
  await page.locator('#outDisconnect').click();
  await expect(page.locator('#outState')).toHaveText('Aucune sortie : aperçu seulement');
});
