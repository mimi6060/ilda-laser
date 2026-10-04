// ShowNET SD card over Art-Net (T-301): the operator types the box's
// address in RÉGLAGES › Sorties, connects it, and the studio sends ArtDmx
// to it. Here the "box" is this spec's own UDP receiver on 127.0.0.1 and a
// port the OS picks (never 6454, never a real ShowNET; the test studio's
// backend refuses any other address). Disarmed, Escape, a cue without a
// bank number and disconnecting all give intensity 0 and pattern 0, sent
// continuously; armed on a bank cue: its number and an intensity.
import { test, expect, useStudio, openUi, reveal, openWorkspace } from '../studio';
import { createSocket, type Socket } from 'node:dgram';

let rx: Socket;
let packets: Buffer[] = [];

test.beforeAll(async () => {
  rx = createSocket('udp4');
  rx.on('message', m => { packets.push(m); });
  await new Promise<void>(resolve => rx.bind(0, '127.0.0.1', () => resolve()));
});
test.afterAll(() => { rx?.close(); });

// No fake output: the Art-Net one is connected from the UI.
const studio = useStudio({ output: 'none' });
const port = () => rx.address().port;
const id = () => `artnet:127.0.0.1:${port()}`;

/** Channels 1 and 2 (start address 1) of one ArtDmx packet. */
function dmx(d: Buffer): [number, number] {
  expect(d.subarray(0, 8).toString('latin1')).toBe('Art-Net\0');
  expect(d.readUInt16LE(8)).toBe(0x5000);
  expect(d.readUInt16BE(10)).toBe(14);
  expect(d.readUInt16BE(16)).toBe(512);
  expect(d.length).toBe(18 + 512);
  return [d[18], d[19]];
}

/** (intensity, pattern) of every packet received during the next `ms`. */
async function listen(ms: number) {
  packets = [];
  await new Promise(r => setTimeout(r, ms));
  return packets.map(dmx);
}

const armed = async () => (await studio.state()).armed as boolean;

test('the box is typed, listed and connected, dark while disarmed', async ({ page }) => {
  expect(port()).not.toBe(6454);
  await openUi(page, studio);
  await page.locator('#artnetPanel summary').evaluate(el => (el.parentElement as HTMLDetailsElement).open = true);
  await reveal(page, '#anTarget');
  // Off by default: nothing listed.
  await expect(page.locator(`#devList li[data-id^="artnet:"]`)).toHaveCount(0);
  // A broadcast is refused.
  await page.locator('#anTarget').fill('255.255.255.255');
  await page.locator('#anSave').click();
  await expect(page.locator('#anMsg')).toContainText('diffusion');
  await page.locator('#anTarget').fill(`127.0.0.1:${port()}`);
  await page.locator('#anSave').click();
  await expect(page.locator('#anMsg')).toContainText('Enregistré');
  const row = page.locator(`#devList li[data-id="${id()}"]`);
  await expect(row).toContainText('ShowNET SD');
  await row.locator('.dev-connect').click();
  await expect(page.locator('#output')).toHaveText(`Sortie : ShowNET carte SD (Art-Net 127.0.0.1:${port()})`);
  await expect(page.locator('#output')).toHaveAttribute('data-kind', 'artnet');
  await expect(page.locator('#anLive')).toContainText('intensité 0, figure 0');
  expect(await armed()).toBe(false);
  const got = await listen(500);
  expect(got.length).toBeGreaterThan(8); // ~30 a second, never silence
  expect(got.every(([i, p]) => i === 0 && p === 0)).toBe(true);
  // Settings are fixed while connected.
  await page.locator('#anSave').click();
  await expect(page.locator('#anMsg')).toContainText('Déconnectez');
});

test('armed on a bank cue it sends its number, Escape and cues without number go dark', async ({ page }) => {
  await openUi(page, studio);
  const cues = page.locator('#cues .cue');
  const [banked, unbanked] = [await cues.nth(0).getAttribute('data-id'), await cues.nth(1).getAttribute('data-id')];
  expect(await studio.post('/api/sdbank', { slots: [{ number: 7, cue_id: banked, seconds: 1 }] })).toBe(200);
  // A ShowNET output: the checklist (hardware stop included) and a Public zone.
  await studio.completeChecklist();
  await studio.declareAudience();
  await page.locator(`#cues .cue[data-id="${banked}"]`).click();
  await expect.poll(async () => (await studio.frame()).active_cue).toBe(banked);
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await expect.poll(async () => (await listen(150)).at(-1)?.[1]).toBe(7);
  const lit = await listen(300);
  expect(lit.every(([i, p]) => i > 0 && i <= 128 && p === 7)).toBe(true); // ≤ master × 50 % default cap
  await expect(page.locator('#sdPill')).toBeHidden();

  // A cue that has no bank number: dark, and a warning in the top bar.
  await page.locator(`#cues .cue[data-id="${unbanked}"]`).click();
  await expect.poll(async () => (await studio.frame()).active_cue).toBe(unbanked);
  await expect(page.locator('#sdPill')).toBeVisible();
  await expect(page.locator('#sdPill')).toHaveAttribute('title', /n'a pas de numéro/);
  await new Promise(r => setTimeout(r, 100));
  const dark = await listen(300);
  expect(dark.length).toBeGreaterThan(5);
  expect(dark.every(([i, p]) => i === 0 && p === 0)).toBe(true);

  // Back on the bank cue, then Escape: dark at once, still sending.
  await page.locator(`#cues .cue[data-id="${banked}"]`).click();
  await expect.poll(async () => (await listen(150)).at(-1)?.[1]).toBe(7);
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  await new Promise(r => setTimeout(r, 100));
  const after = await listen(300);
  expect(after.length).toBeGreaterThan(5);
  expect(after.every(([i, p]) => i === 0 && p === 0)).toBe(true);
  expect(await studio.post('/api/estop/reset', {})).toBe(200);
});

test('disconnecting sends dark packets, then nothing', async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  packets = [];
  await page.locator('#outDisconnect').click();
  await expect(page.locator('#output')).toHaveText('Aperçu seulement');
  await new Promise(r => setTimeout(r, 200));
  const last = packets.map(dmx);
  expect(last.length).toBeGreaterThan(3);
  expect(last.every(([i, p]) => i === 0 && p === 0)).toBe(true);
  expect(await listen(300)).toEqual([]);
});
