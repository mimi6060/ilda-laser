// OSC and Art-Net inputs (T-013), switched on from RÉGLAGES › Entrées
// réseau. The studio listens on 127.0.0.1 with a port the system picks
// (0), and the test sends its packets from a loopback socket: nothing ever
// goes to another host, nothing is broadcast, and the studio never sends.
import dgram from 'node:dgram';
import { test, expect, useStudio, openUi, openWorkspace, type Frame } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Listen { local: string | null; error: string | null; packets: number; rejected: number }
interface NetIn {
  osc: { status: Listen; recent: { address: string; error: string | null }[] };
  artnet: { status: Listen; signal: string; channels: { channel: number; value: number | null }[] };
}
const netin = () => studio.get<NetIn>('/api/netin');
const armed = async () => (await studio.state()).armed;
const frame = () => studio.get<Frame>('/api/frame');

function osc(address: string, args: (number | string)[] = []): Buffer {
  const pad = (b: Buffer) => Buffer.concat([b, Buffer.alloc(4 - (b.length % 4))]);
  const parts = [pad(Buffer.from(address)), pad(Buffer.from(',' + args.map(a => (typeof a === 'string' ? 's' : 'f')).join('')))];
  for (const a of args) {
    if (typeof a === 'string') parts.push(pad(Buffer.from(a)));
    else { const b = Buffer.alloc(4); b.writeFloatBE(a); parts.push(b); }
  }
  return Buffer.concat(parts);
}

function artDmx(universe: number, data: number[]): Buffer {
  const head = Buffer.alloc(18);
  head.write('Art-Net\0', 0, 'latin1');
  head.writeUInt16LE(0x5000, 8);
  head.writeUInt16BE(14, 10);
  head.writeUInt16LE(universe, 14);
  head.writeUInt16BE(data.length, 16);
  return Buffer.concat([head, Buffer.from(data)]);
}

async function send(packet: Buffer, local: string | null) {
  const port = Number(local?.split(':').pop());
  expect(port).toBeGreaterThan(0);
  const sock = dgram.createSocket('udp4');
  await new Promise<void>((resolve, reject) => sock.send(packet, port, '127.0.0.1', e => (e ? reject(e) : resolve())));
  sock.close();
}

async function openPanel(page: Page) {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#netinPanel summary').click();
}

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/netin', { osc: { enabled: false, port: 0 }, artnet: { enabled: false, port: 0, universe: 0, address: 1 }, loss: { policy: 'neutral', timeout_ms: 2500 } });
  // The external inputs gate (T-408): open, every group, no access needed.
  const groups = ['master', 'look', 'layer', 'grid', 'cue', 'page', 'tempo', 'timeline', 'audio'];
  await studio.post('/api/external/scopes', { scopes: ['dmx', 'osc'].map(source => ({ source, groups, require_access: false })) });
  await studio.post('/api/external/gate', { on: true });
});

test('off by default, on localhost', async () => {
  const fresh = await studio.get('/api/netin');
  expect(fresh.effective.osc.enabled).toBe(false);
  expect(fresh.effective.artnet.enabled).toBe(false);
  expect(fresh.config.osc.bind).toBe('127.0.0.1');
  expect(fresh.config.artnet.bind).toBe('127.0.0.1');
  expect(fresh.osc.status.local).toBeNull();
});

test('an OSC message plays a cue; arming over OSC is refused', async ({ page }) => {
  await openPanel(page);
  await page.locator('#oscEnabled').check();
  await expect(page.locator('#oscState')).toContainText('Écoute sur 127.0.0.1:');
  await expect.poll(async () => (await netin()).osc.status.local).not.toBeNull();
  const local = (await netin()).osc.status.local;
  const { presets } = await studio.presets();
  const cue = presets[3].id;

  await send(osc('/ls/cue/play', [cue]), local);
  await expect.poll(async () => (await frame()).active_cue).toBe(cue);
  await expect(page.locator('#oscMonitor')).toContainText('/ls/cue/play');

  await send(osc('/ls/arm'), local);
  await send(osc('/ls/control/transport.arm', [1]), local);
  await expect(page.locator('#oscMonitor')).toContainText("l'armement ne se fait jamais depuis le réseau");
  expect(await armed()).toBe(false);

  await send(osc('/ls/norm/master.size', [0.25]), local);
  await expect.poll(async () => (await frame()).live.size).toBeCloseTo(0.5, 5);

  // Off: the socket closes.
  await page.locator('#oscEnabled').uncheck();
  await expect(page.locator('#oscState')).toHaveText('Coupé');
  await expect.poll(async () => (await netin()).osc.status.local).toBeNull();
});

test('a DMX channel changes the brightness; signal loss goes neutral without arming', async ({ page }) => {
  await openPanel(page);
  await page.locator('#netinTimeout').fill('0.5');
  await page.locator('#netinTimeout').press('Tab');
  await expect.poll(async () => (await studio.get('/api/netin')).config.loss.timeout_ms).toBe(500);
  await page.locator('#artnetEnabled').check();
  await expect(page.locator('#artnetState')).toContainText('En attente de données');
  await expect.poll(async () => (await netin()).artnet.status.local).not.toBeNull();
  const local = (await netin()).artnet.status.local;

  // Intensité, page, cue, taille, X, Y, rotation, vitesse, mode, teinte, blackout.
  await send(artDmx(0, [51, 0, 0, 128, 128, 128, 128, 64, 0, 0, 0]), local);
  await expect.poll(async () => (await frame()).live.brightness).toBeCloseTo(0.2, 5);
  await expect(page.locator('#dmxChart tr').first()).toContainText('51');
  await send(artDmx(7, [255, 0, 0, 128, 128, 128, 128, 64, 0, 0, 0]), local); // another universe
  await expect.poll(async () => (await netin()).artnet.signal).toBe('lost'); // 0.5 s without our universe
  await expect(page.locator('#artnetState')).toContainText('Signal perdu');
  expect((await frame()).live.brightness).toBe(0);
  expect(await armed()).toBe(false);

  await send(artDmx(0, [255, 0, 0, 128, 128, 128, 128, 64, 0, 0, 0]), local);
  await expect.poll(async () => (await frame()).live.brightness).toBe(1);
  expect(await armed()).toBe(false);
});

test('the blackout channel latches the emergency stop', async ({ page }) => {
  await openPanel(page);
  await page.locator('#artnetEnabled').check();
  await expect(page.locator('#artnetState')).toContainText('En attente');
  await expect.poll(async () => (await netin()).artnet.status.local).not.toBeNull();
  const local = (await netin()).artnet.status.local;
  await send(artDmx(0, [255, 0, 0, 128, 128, 128, 128, 64, 0, 0, 255]), local);
  await expect.poll(async () => (await studio.get('/api/arm')).estop?.source).toBe('dmx');
  expect(await armed()).toBe(false);
});

test('a bad address is refused with a French message', async ({ page }) => {
  await openPanel(page);
  await page.locator('#oscBind').fill('console.local');
  await page.locator('#oscBind').press('Tab');
  await expect(page.locator('#netinError')).toContainText("adresse d'écoute invalide");
  await expect(page.locator('#oscBind')).toHaveValue('127.0.0.1');
});
