// DMX in (T-406): sACN and Art-Net into the free patch, the monitor, the
// loss policy and Apprendre DMX. The studio listens on 127.0.0.1 with
// ports the system picks (0); the test sends unicast packets from a
// loopback socket. Never multicast, never another host, and the studio
// never sends anything. Preview only.
import dgram from 'node:dgram';
import { test, expect, useStudio, openUi, openWorkspace, type Frame } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Listen { local: string | null }
interface NetIn { artnet: { status: Listen }; sacn: { status: Listen }; config: { dmx: { mappings: { universe: number; channel: number; control: string }[] } } }
const netin = () => studio.get<NetIn>('/api/netin');
const frame = () => studio.get<Frame>('/api/frame');

/** An E1.31 data packet (ANSI E1.31, our own encoder for the test). */
function sacn(universe: number, slots: number[], { name = 'Console e2e', priority = 100, seq = 0, cid = 7, terminated = false } = {}): Buffer {
  const len = 126 + slots.length;
  const p = Buffer.alloc(len);
  const flags = (n: number) => 0x7000 | n;
  p.writeUInt16BE(0x0010, 0);
  p.write('ASC-E1.17\0\0\0', 4, 'latin1');
  p.writeUInt16BE(flags(len - 16), 16);
  p.writeUInt32BE(4, 18);
  p.fill(cid, 22, 38);
  p.writeUInt16BE(flags(len - 38), 38);
  p.writeUInt32BE(2, 40);
  p.write(name, 44, 'utf8');
  p.writeUInt8(priority, 108);
  p.writeUInt8(seq & 0xff, 111);
  p.writeUInt8(terminated ? 0x40 : 0, 112);
  p.writeUInt16BE(universe, 113);
  p.writeUInt16BE(flags(len - 115), 115);
  Buffer.from([0x02, 0xa1, 0, 0, 0, 1]).copy(p, 117);
  p.writeUInt16BE(slots.length + 1, 123);
  p.writeUInt8(0, 125);
  Buffer.from(slots).copy(p, 126);
  return p;
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

let seq = 0;
async function sendSacn(universe: number, slots: number[], opts: { name?: string; priority?: number; cid?: number; terminated?: boolean } = {}) {
  await send(sacn(universe, slots, { ...opts, seq: ++seq }), (await netin()).sacn.status.local);
}

async function openPanel(page: Page) {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#netinPanel summary').click();
}

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/dmx/config', {
    artnet: { enabled: false, port: 0, universe: 0, address: 1 },
    sacn: { enabled: false, port: 0, universes: [1], multicast: false },
    dmx: { mappings: [{ universe: 1, channel: 1, control: 'master.size', pickup: false }], access: null },
    loss: { policy: 'hold', timeout_ms: 2500, fade_ms: 2000 },
  });
  await studio.post('/api/external/scopes', { scopes: [{ source: 'dmx', groups: ['master', 'look', 'layer'], require_access: false }] });
  await studio.post('/api/external/gate', { on: true });
});

test('sACN on loopback drives a patched control, shown in the monitor', async ({ page }) => {
  await openPanel(page);
  await expect(page.locator('#sacnBind')).toHaveValue('127.0.0.1');
  await page.locator('#sacnEnabled').check();
  await expect(page.locator('#sacnState')).toContainText('Écoute sur 127.0.0.1:');
  await expect.poll(async () => (await netin()).sacn.status.local).not.toBeNull();

  await sendSacn(1, [64, 0, 0, 200]);
  await expect.poll(async () => (await frame()).live.size).toBeCloseTo(2 * 64 / 255, 2);
  await expect(page.locator('#dmxMonGrid span').nth(3)).toHaveText('200');
  await expect(page.locator('#dmxMonState')).toContainText('Console e2e');
  await expect(page.locator('#dmxSources')).toContainText('priorité 100');
  await expect(page.locator('#dmxPill')).toHaveClass(/ok/);

  // A second source with a higher priority takes over; the lower one is ignored.
  await sendSacn(1, [255], { name: 'Secours', priority: 150, cid: 9 });
  await sendSacn(1, [10], { priority: 100 });
  await expect.poll(async () => (await frame()).live.size).toBeCloseTo(2, 2);
  await expect(page.locator('#dmxSources')).toContainText('Secours');
  // The backup announces it stops: the first console leads again.
  await sendSacn(1, [255], { name: 'Secours', priority: 150, cid: 9, terminated: true });
  await sendSacn(1, [20], { priority: 100 });
  await expect.poll(async () => (await frame()).live.size).toBeCloseTo(2 * 20 / 255, 2);
  expect((await studio.state()).armed).toBe(false);
});

test('Art-Net and sACN drive the same control', async () => {
  await studio.post('/api/dmx/config', { artnet: { enabled: true }, sacn: { enabled: true } });
  await expect.poll(async () => { const n = await netin(); return !!(n.sacn.status.local && n.artnet.status.local); }).toBe(true);
  await send(artDmx(1, [200, 0]), (await netin()).artnet.status.local);
  await expect.poll(async () => (await frame()).live.size).toBeCloseTo(2 * 200 / 255, 2);
  await sendSacn(1, [0]);
  await expect.poll(async () => (await frame()).live.size).toBeCloseTo(0, 2);
});

test('signal loss: banner with the source name, dark output, armed state untouched', async ({ page }) => {
  await studio.post('/api/dmx/config', { sacn: { enabled: true }, loss: { policy: 'dark', timeout_ms: 500 } });
  await openUi(page, studio);
  await expect.poll(async () => (await netin()).sacn.status.local).not.toBeNull();
  await sendSacn(1, [128]);
  await expect.poll(async () => (await frame()).live.size).toBeCloseTo(2 * 128 / 255, 2);
  await expect(page.locator('#dmxLost')).toBeHidden();
  // Nothing more: lost after 0.5 s.
  await expect(page.locator('#dmxLost')).toBeVisible({ timeout: 5000 });
  await expect(page.locator('#dmxLost')).toContainText('Signal DMX perdu — Console e2e (univers 1)');
  await expect(page.locator('#dmxPill')).toHaveClass(/lost/);
  await expect.poll(async () => (await frame()).points.every(p => p[2] === 0 && p[3] === 0 && p[4] === 0)).toBe(true);
  expect((await studio.state()).armed).toBe(false);
  expect((await studio.state()).estop).toBe(false);
  // The console is back (a stream again): the banner goes, the light
  // comes back, still disarmed.
  const stream = setInterval(() => { sendSacn(1, [128]).catch(() => {}); }, 100);
  try {
    await expect(page.locator('#dmxLost')).toBeHidden();
    await expect.poll(async () => (await frame()).points.some(p => p[2] + p[3] + p[4] > 0)).toBe(true);
    expect((await studio.state()).armed).toBe(false);
  } finally {
    clearInterval(stream);
  }
});

// T-511: a second edit made before the first is answered builds on it;
// both reach the studio, in order.
test('two quick mapping edits are both saved', async ({ page }) => {
  await openPanel(page);
  const row = page.locator('#dmxMaps tr[data-i="0"]');
  await expect(row.locator('[data-k=channel]')).toHaveValue('1');
  // Every save answered late: the second edit is made while the first waits.
  const posted: string[] = [];
  await page.route('**/api/netin', async route => {
    if (route.request().method() !== 'POST') return route.continue();
    posted.push(route.request().postData() ?? '');
    await new Promise(r => setTimeout(r, 400));
    await route.continue();
  });
  await row.locator('[data-k=channel]').fill('7');
  await row.locator('[data-k=channel]').press('Tab');
  await expect.poll(() => posted.length).toBe(1);
  await row.locator('[data-k=invert]').check();
  await expect.poll(async () => (await studio.get('/api/netin')).config.dmx.mappings[0]).toMatchObject({ universe: 1, channel: 7, control: 'master.size', invert: true });
  expect(posted).toHaveLength(2);
  await page.unrouteAll({ behavior: 'wait' });
  await expect(row.locator('[data-k=channel]')).toHaveValue('7');
  await expect(row.locator('[data-k=invert]')).toBeChecked();
});

test('Apprendre DMX maps the channel that moves', async ({ page }) => {
  await studio.post('/api/dmx/config', { sacn: { enabled: true }, dmx: { mappings: [] } });
  await openUi(page, studio);
  await expect.poll(async () => (await netin()).sacn.status.local).not.toBeNull();
  await page.locator('#topMaster').click({ button: 'right' });
  await page.locator('#midiMenuDmx').click();
  await expect(page.locator('#learnBanner')).toContainText('Apprendre DMX');
  // Where the faders are when learning starts, then channel 5 moves.
  await sendSacn(1, [0, 0, 0, 0, 0, 0]);
  await expect.poll(async () => (await studio.get<{ values: number[] | null }>('/api/dmx/monitor?universe=1')).values).not.toBeNull();
  await sendSacn(1, [0, 0, 0, 0, 90, 0]);
  await expect.poll(async () => (await netin()).config.dmx.mappings).toEqual([expect.objectContaining({ universe: 1, channel: 5, control: 'master.brightness' })]);
  await expect(page.locator('#learnBanner')).toContainText('canal 5');
});
