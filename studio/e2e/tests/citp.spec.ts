// CITP (T-454): cue names and pictures for a lighting console, set in
// RÉGLAGES › Vignettes pour la console (CITP). Off by default (no socket);
// switched on it listens on 127.0.0.1 here, on a port the system picks.
// The "console" is this spec's own TCP client on the loopback: it says its
// name (PINF PNam) and must appear in « Consoles connectées ». The full
// MSEX exchange (libraries, elements, pictures, updates) is covered by the
// Rust tests in citp/server.rs. Preview only: the laser is never armed.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import { connect, type Socket } from 'node:net';

const studio = useStudio();

interface Citp { enabled: boolean; listening: boolean; local: string | null; peers: string[]; served_thumbs: number; ignored_messages: number }
const citp = async () => (await studio.state()).citp as Citp;

/** A CITP/PINF PNam message (public spec §3.1.1 and §4): header, then "PNam" and the name. */
function pnam(name: string) {
  const body = Buffer.concat([Buffer.from('PNam', 'latin1'), Buffer.from(name + '\0', 'latin1')]);
  const h = Buffer.alloc(20);
  h.write('CITP', 0, 'latin1');
  h.writeUInt8(1, 4); h.writeUInt8(0, 5);
  h.writeUInt16LE(0, 6);
  h.writeUInt32LE(20 + body.length, 8);
  h.writeUInt16LE(1, 12); h.writeUInt16LE(0, 14);
  h.write('PINF', 16, 'latin1');
  return Buffer.concat([h, body]);
}

test.afterEach(async () => {
  await studio.post('/api/citp', { enabled: false, interface: null });
  expect((await studio.state()).armed, 'CITP never arms').toBe(false);
});

test('off by default, switched on in RÉGLAGES, a loopback console is listed, switched off', async ({ page }) => {
  const first = await studio.get('/api/citp');
  expect(first.config.enabled).toBe(false);
  expect((await citp()).listening).toBe(false);

  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#citpPanel summary').evaluate(el => (el.parentElement as HTMLDetailsElement).open = true);
  await expect(page.locator('#citpEnabled')).toBeVisible();
  await expect(page.locator('#citpThumb')).toHaveValue('128');
  await page.locator('#citpEnabled').check();
  await expect.poll(async () => (await citp()).listening).toBe(true);
  await expect(page.locator('#citpState')).toContainText("À l'écoute sur 127.0.0.1:");

  const local = (await citp()).local!;
  const [host, port] = local.split(':');
  expect(host).toBe('127.0.0.1');
  const sock: Socket = connect(+port, host);
  const got: Buffer[] = [];
  sock.on('data', d => got.push(d));
  await new Promise<void>((resolve, reject) => { sock.once('connect', () => resolve()); sock.once('error', reject); });
  try {
    sock.write(pnam('Console de test'));
    // The server names itself right after the connection.
    await expect.poll(() => Buffer.concat(got).subarray(16, 24).toString('latin1')).toBe('PINFPNam');
    await expect.poll(async () => (await citp()).peers.join(' ')).toContain('Console de test');
    await expect(page.locator('#citpPeers')).toContainText('Console de test');
    // Something it doesn't know is ignored and counted, nothing else.
    const junk = pnam('x'); junk.write('ZZZZ', 16, 'latin1');
    sock.write(junk);
    await expect.poll(async () => (await citp()).ignored_messages).toBeGreaterThan(0);
    expect((await studio.state()).armed).toBe(false);
  } finally {
    sock.destroy();
  }

  await page.locator('#citpEnabled').uncheck();
  await expect.poll(async () => (await citp()).listening).toBe(false);
  await expect(page.locator('#citpState')).toHaveText('Arrêté');
  expect((await studio.get('/api/citp')).config.enabled).toBe(false);
});
