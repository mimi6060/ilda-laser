// ShowNET projector sheet and hardware-stop reminders (T-472). The fake
// output (--test-output, a log file, never a laser) is declared ShowNET on
// the projector sheet: arming is refused until « Arrêt matériel testé » is
// ticked. The Wi-Fi check is simulated (--test-hooks): no command is run,
// nothing is sent on the network.
import { test, expect, useStudio, openUi, reveal, focusPage } from '../studio';

const HW = 'Arrêt matériel non vérifié';
const studio = useStudio({ testHooks: true });

test('the ShowNET model fills its figures and the hardware stop gates arming', async ({ page }) => {
  await openUi(page, studio);
  // Not a ShowNET yet: no ShowNET items, the laser arms as before.
  let c = await studio.get('/api/checklist');
  expect(c.items.filter((i: { visible: boolean; only_for: string | null }) => i.visible && i.only_for)).toHaveLength(0);
  expect((await studio.state()).shownet).toEqual({ active: false, hw_stop_ok: true, wifi: null, wifi_device: null });

  await reveal(page, '#outputsPanel');
  await expect(page.locator('#pjModel option')).toHaveCount(2);
  await expect(page.locator('#pjShownetHelp')).toBeHidden();
  await page.locator('#pjModel').selectOption('shownet');
  await expect(page.locator('#pjXyBits')).toHaveValue('12');
  await expect(page.locator('#pjColorBits')).toHaveValue('8');
  await expect(page.locator('#pjMaxPps')).toHaveValue('100000');
  await expect(page.locator('#pjShownetHelp')).toBeVisible();
  await expect.poll(async () => {
    const j = await studio.get('/api/outputs/limits');
    const p = j.outputs.find((o: { id: string }) => o.id === j.active).projector;
    return [p.dac_model, p.xy_bits, p.color_bits, p.max_pps];
  }).toEqual(['shownet', 12, 8, 100000]);

  // Arming is refused, with the reason, and logged.
  await expect(page.locator('#armBtn')).toHaveText('Arrêt matériel à vérifier');
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect(page.locator('#armRefused')).toContainText(HW);
  expect((await studio.state()).armed).toBe(false);
  expect((await studio.get('/api/arm')).blocking).toContain(HW);
  const log = await studio.get('/api/safety/log');
  const refused = log.events.filter((e: { kind: string; detail: { blocking?: string[] } }) => e.kind === 'arm_refused' && e.detail.blocking?.includes(HW));
  expect(refused.length).toBeGreaterThan(0);

  // The button opens the list: the three ShowNET items and the help text.
  await page.locator('#armBtn').click();
  await expect(page.locator('#checklistPanel')).toHaveAttribute('open', '');
  await expect(page.locator('#clShownetHelp')).toBeVisible();
  await expect(page.locator('#clLed')).toHaveText('Arrêt matériel à vérifier');
  for (const id of ['shownet_hw_stop', 'shownet_wired', 'shownet_exclusive']) {
    await expect(page.locator(`#clItems li[data-item="${id}"]`)).toHaveCount(1);
  }
  const hw = page.locator('#clItems li[data-item="shownet_hw_stop"]');
  await expect(hw).toContainText('Arrêt matériel testé (interlock ou arrêt d\'urgence du projecteur)');
  await hw.locator('input[type=checkbox]').check();
  await expect(hw).toHaveClass(/done/);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  expect((await studio.state()).armed).toBe(false);

  // Accepted now.
  await page.locator('#armBtn').click();
  await expect.poll(async () => (await studio.state()).armed).toBe(true);
  // Unticking it disarms at once.
  await hw.locator('input[type=checkbox]').uncheck();
  await expect.poll(async () => (await studio.state()).armed).toBe(false);
  expect((await studio.get('/api/arm')).last_disarm.reason_fr).toBe(`Verrou : ${HW}`);

  // Back to « other »: no ShowNET item, no lock.
  await reveal(page, '#outputsPanel');
  await page.locator('#pjModel').selectOption('other');
  await expect.poll(async () => (await studio.state()).shownet.active).toBe(false);
  expect((await studio.get('/api/arm')).blocking).not.toContain(HW);
  c = await studio.get('/api/checklist');
  expect(c.items.filter((i: { visible: boolean; only_for: string | null }) => i.visible && i.only_for)).toHaveLength(0);
});

test('a Wi-Fi link to a ShowNET output shows a permanent orange pill', async ({ page }) => {
  await openUi(page, studio);
  expect(await studio.post('/api/test/wifi', { wireless: true, interface: 'en0' })).toBe(200);
  // Not a ShowNET output: no pill.
  expect(await studio.post('/api/outputs/limits', { projector: { ...(await sheet()), dac_model: 'other' } })).toBe(200);
  await expect(page.locator('#wifiPill')).toBeHidden();
  expect(await studio.post('/api/outputs/limits', { projector: { ...(await sheet()), dac_model: 'shownet' } })).toBe(200);
  await expect(page.locator('#wifiPill')).toBeVisible();
  await expect(page.locator('#wifiPill')).toHaveText('Wi-Fi');
  await expect(page.locator('#wifiPill')).toHaveAttribute('title', /en0/);
  // Wired: gone.
  expect(await studio.post('/api/test/wifi', { wireless: false, interface: 'en7' })).toBe(200);
  await expect(page.locator('#wifiPill')).toBeHidden();
  expect(await studio.post('/api/outputs/limits', { projector: { ...(await sheet()), dac_model: 'other' } })).toBe(200);
});

test('no interface text says the box stops by itself', async ({ page }) => {
  await openUi(page, studio);
  const html = (await page.content()).toLowerCase();
  for (const claim of ['s\'éteint seul', 's\'éteint tout seul', 'se coupe seul', 'se coupe tout seul', 'blanks by itself', 'coupe automatiquement le faisceau']) {
    expect(html).not.toContain(claim);
  }
});

async function sheet() {
  const j = await studio.get('/api/outputs/limits');
  return j.outputs.find((o: { id: string }) => o.id === j.active).projector;
}
