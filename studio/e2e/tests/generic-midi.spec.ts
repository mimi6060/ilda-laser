// Generic MIDI controllers (T-211): an unknown device, plugged next to the
// simulated APC40 mkII of --midi-test through POST /api/midi/plug, is
// listed with its activity light, learned, given LED feedback, and its
// profile exported then imported back from the « Contrôleur » section.
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test, expect, useStudio, openUi, reveal } from '../studio';
import { TEST_PORT } from '../midi';

const studio = useStudio({ midiTest: true });
const PAD = 'Test Pad générique';

interface Mapping { port: string; profile: string; index: number; message: string; channel: number | null; target: string; mode: string; kind: string; encoding: string; led: { off: number; on: number; blink?: number } | null }
interface Device { name: string; connected: boolean; model: string; profile: string; activity_ms: number | null }
const midi = () => studio.get('/api/midi');
const pad = async () => ((await midi()).devices as Device[]).find(d => d.name === PAD);
const padMaps = async () => ((await midi()).mappings as Mapping[]).filter(m => m.port === PAD);
const inject = async (bytes: number[]) => expect(await studio.post('/api/midi/inject', { port: PAD, bytes })).toBe(200);
const sentToPad = async () => ((await studio.get('/api/midi/sent')).devices as { port: string; sent: number[][] }[]).find(d => d.port === PAD)!.sent;

async function openPanel(page: import('@playwright/test').Page) {
  await reveal(page, '#midiPanel summary');
  if (!await page.locator('#midiPanel').evaluate(e => (e as HTMLDetailsElement).open)) await page.locator('#midiPanel summary').click();
}

test.beforeAll(async () => {
  expect(await studio.post('/api/midi/plug', { port: PAD })).toBe(200);
  await expect.poll(async () => { const d = await pad(); return d && [d.connected, d.model, d.profile]; }, { timeout: 8000 }).toEqual([true, 'Unknown', 'generic']);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/midi/learn/cancel');
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await openUi(page, studio);
});

test('an unknown device is listed with its activity light and shows up in the monitor', async ({ page }) => {
  await openPanel(page);
  const row = page.locator('#midiDevices .mdev', { hasText: PAD });
  await expect(row).toContainText('Modèle détecté : inconnu');
  await expect(row.locator('select[data-mprof]')).toHaveValue('generic');
  await expect(page.locator('#midiDevices .mdev', { hasText: TEST_PORT })).toBeVisible(); // the APC is still there
  await inject([0x95, 61, 90]);
  await expect(row.locator('.mact')).toHaveClass(/on/);
  await expect(page.locator('#midiMonitor')).toContainText(`${PAD} · Note On 61 · vél. 90 · can. 6`);
  await expect(row.locator('.mact')).not.toHaveClass(/on/, { timeout: 4000 });
  // Templates are offered in the profile menu.
  await expect(row.locator('select[data-mprof] optgroup[label="Modèles de départ"] option')).toHaveCount(4);
});

test('learn on any channel, LED feedback, encoder encoding, export then import', async ({ page }) => {
  // Learned without any device-specific code: a CC on channel 5, a note on channel 3.
  expect(await studio.post('/api/midi/learn', { target: 'master.pos_x' })).toBe(200);
  await inject([0xb4, 21, 65, 0xb4, 21, 65]); // an encoder step, twice
  expect(await studio.post('/api/midi/learn', { target: 'cue.multi' })).toBe(200);
  await inject([0x92, 60, 100]);
  await expect.poll(async () => (await padMaps()).map(m => [m.message, m.channel, m.mode])).toEqual([['CC 21', 4, 'relative'], ['Note 60', 2, 'toggle']]);
  expect((await pad())!.profile).toBe('generic-perso');

  await openPanel(page);
  const rows = page.locator('#midiMappings .mmap');
  // The encoder's encoding is chosen from the list.
  const enc = rows.locator(`select[data-menc][data-port="${PAD}"]`).first();
  await expect(enc).toHaveValue('relative:offset64');
  await enc.selectOption('relative:sign_bit');
  await expect.poll(async () => (await padMaps())[0].encoding).toBe('sign_bit');

  // LED: « 0 127 » on the note; the studio lights it when multi is on.
  const led = rows.locator(`input[data-mledv][data-port="${PAD}"]`).nth(1);
  await led.fill('0 127');
  await led.press('Enter');
  await expect.poll(async () => (await padMaps())[1].led).toEqual({ off: 0, on: 127 });
  await expect.poll(async () => (await sentToPad()).filter(m => m[0] === 0x92 && m[1] === 60).pop()).toEqual([0x92, 60, 0]);
  await inject([0x92, 60, 100, 0x82, 60, 0]);
  await expect.poll(async () => (await sentToPad()).filter(m => m[0] === 0x92 && m[1] === 60).pop()).toEqual([0x92, 60, 127]);
  await inject([0x92, 60, 100, 0x82, 60, 0]);
  await expect.poll(async () => (await sentToPad()).filter(m => m[0] === 0x92 && m[1] === 60).pop()).toEqual([0x92, 60, 0]);
  expect((await sentToPad()).every(m => m[0] !== 0xf0 || m.length === 6)).toBe(true); // no APC SysEx, only the inquiry

  // Export, then import: a new profile with exactly the same mappings.
  const before = await padMaps();
  const row = page.locator('#midiDevices .mdev', { hasText: PAD });
  const [download] = await Promise.all([page.waitForEvent('download'), row.locator('[data-mexport]').click()]);
  expect(download.suggestedFilename()).toBe('generic-perso.json');
  const dir = mkdtempSync(path.join(tmpdir(), 'laser-studio-midi-export-'));
  const file = path.join(dir, download.suggestedFilename());
  await download.saveAs(file);
  const exported = JSON.parse(readFileSync(file, 'utf8'));
  expect(exported.mappings).toHaveLength(2);
  const [chooser] = await Promise.all([page.waitForEvent('filechooser'), row.locator('[data-mimport]').click()]);
  await chooser.setFiles(file);
  rmSync(dir, { recursive: true, force: true });
  await expect(page.locator('#learnBanner')).toContainText('Profil importé : generic-perso-2');
  await expect.poll(async () => (await pad())!.profile).toBe('generic-perso-2');
  const after = await padMaps();
  const strip = (m: Mapping[]) => m.map(({ profile, ...rest }) => rest);
  expect(strip(after)).toEqual(strip(before));
  await expect(row.locator('select[data-mprof]')).toHaveValue('generic-perso-2');
  expect((await studio.state()).armed).toBe(false);
});
