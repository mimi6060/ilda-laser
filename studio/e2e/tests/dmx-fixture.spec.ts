// Appareils DMX (T-407): one fixture instance per projection zone, with our
// published personality. The studio listens to sACN on 127.0.0.1 with a
// port the system picks; the test sends unicast packets from a loopback
// socket. Never multicast, never another host, and the studio never sends
// anything. Preview only: nothing arms.
import dgram from 'node:dgram';
import fs from 'node:fs';
import { test, expect, useStudio, openUi, openWorkspace, isLit, type Frame } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Fixture { fixture: { zone: number; address: number; mode: string }; last: number; signal: string; access: boolean; applied: boolean }
interface NetIn { sacn: { status: { local: string | null } }; config: { dmx: { fixtures: { zone: number; address: number }[] } }; dmx: { fixtures: { list: Fixture[] } } }
const netin = () => studio.get<NetIn>('/api/netin');
const frame = () => studio.get<Frame>('/api/frame');

/** An E1.31 data packet (ANSI E1.31, our own encoder for the test). */
function sacn(universe: number, slots: number[], seq: number): Buffer {
  const len = 126 + slots.length;
  const p = Buffer.alloc(len);
  const flags = (n: number) => 0x7000 | n;
  p.writeUInt16BE(0x0010, 0);
  p.write('ASC-E1.17\0\0\0', 4, 'latin1');
  p.writeUInt16BE(flags(len - 16), 16);
  p.writeUInt32BE(4, 18);
  p.fill(7, 22, 38);
  p.writeUInt16BE(flags(len - 38), 38);
  p.writeUInt32BE(2, 40);
  p.write('Console e2e', 44, 'utf8');
  p.writeUInt8(100, 108);
  p.writeUInt8(seq & 0xff, 111);
  p.writeUInt16BE(universe, 113);
  p.writeUInt16BE(flags(len - 115), 115);
  Buffer.from([0x02, 0xa1, 0, 0, 0, 1]).copy(p, 117);
  p.writeUInt16BE(slots.length + 1, 123);
  p.writeUInt8(0, 125);
  Buffer.from(slots).copy(p, 126);
  return p;
}

let seq = 0;
async function sendSacn(slots: number[]) {
  const local = (await netin()).sacn.status.local;
  const port = Number(local?.split(':').pop());
  expect(port).toBeGreaterThan(0);
  const sock = dgram.createSocket('udp4');
  await new Promise<void>((resolve, reject) => sock.send(sacn(1, slots, ++seq), port, '127.0.0.1', e => (e ? reject(e) : resolve())));
  sock.close();
}

/** Compact at its defaults (neutral), with access raised and a position X and size. */
function compact(posX: number, size = 64): number[] {
  // intensity, size, X, Y, rotation, rot. speed, anim. speed, colour mode,
  // hue, strobe, page, cue, size X, size Y, function, access
  return [255, size, posX, 128, 128, 128, 64, 0, 0, 0, 0, 0, 128, 128, 0, 255];
}

async function openPanel(page: Page) {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#netinPanel summary').click();
}

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/projection', {
    zones: [{ id: 1, name: 'Gauche' }, { id: 2, name: 'Droite' }],
    default_zones: [1, 2],
  });
  await studio.post('/api/dmx/config', {
    sacn: { enabled: true, port: 0, universes: [1], multicast: false },
    dmx: { mappings: [], access: null, fixtures: [] },
    loss: { policy: 'hold', timeout_ms: 2500, fade_ms: 2000 },
  });
  await studio.post('/api/external/scopes', { scopes: [{ source: 'dmx', groups: ['master', 'look', 'layer', 'grid', 'cue', 'page'], require_access: true }] });
  await studio.post('/api/external/gate', { on: true });
});

test('two fixtures drive two zones independently, framed in the monitor', async ({ page }) => {
  await openPanel(page);
  await page.locator('#dmxFixAdd').click();
  await expect(page.locator('#dmxFixtures tr[data-i]')).toHaveCount(1);
  await page.locator('#dmxFixAdd').click();
  await expect(page.locator('#dmxFixtures tr[data-i]')).toHaveCount(2);
  await expect.poll(async () => (await netin()).config.dmx.fixtures).toEqual([
    expect.objectContaining({ zone: 1, address: 1 }),
    expect.objectContaining({ zone: 2, address: 17 }),
  ]);
  await expect.poll(async () => (await netin()).sacn.status.local).not.toBeNull();

  // Zone « Gauche » to the left, zone « Droite » to the right, both at 50 %.
  await sendSacn([...compact(0), ...compact(255)]);
  await expect.poll(async () => (await netin()).dmx.fixtures.list.map(f => f.applied)).toEqual([true, true]);
  await expect.poll(async () => {
    const lit = (await frame()).points.filter(isLit);
    return lit.length > 0 && lit.some(p => p[0] < -0.6) && lit.some(p => p[0] > 0.6) && lit.every(p => Math.abs(p[0]) > 0.4);
  }).toBe(true);
  await expect(page.locator('#dmxFixtures tr[data-i="0"] .st')).toContainText('appliqué');

  // Both to the left: the right half goes dark.
  await sendSacn([...compact(0), ...compact(0)]);
  await expect.poll(async () => {
    const lit = (await frame()).points.filter(isLit);
    return lit.length > 0 && lit.every(p => p[0] < -0.4);
  }).toBe(true);

  // Access down on « Droite »: back to neutral (centred, full size).
  const down = compact(0);
  down[15] = 0;
  await sendSacn([...compact(0), ...down]);
  await expect.poll(async () => (await netin()).dmx.fixtures.list.map(f => f.applied)).toEqual([true, false]);

  // The monitor frames and names each instance's channels.
  await expect(page.locator('#dmxMonFix')).toContainText('Gauche — Compact — 16 canaux, canaux 1–16');
  await expect(page.locator('#dmxMonFix')).toContainText('Droite — Compact — 16 canaux, canaux 17–32');
  await expect(page.locator('#dmxMonGrid span').nth(16)).toHaveClass(/fx-start/);
  await expect(page.locator('#dmxMonGrid span').nth(31)).toHaveAttribute('title', /Accès/);

  expect((await studio.state()).armed).toBe(false);
});

test('the channel chart, the fixture file and the overlap refusal', async ({ page }) => {
  await openPanel(page);
  await page.locator('#dmxFixChart').click();
  await expect(page.locator('#dmxFixChartBox')).toBeVisible();
  await expect(page.locator('#dmxFixChartBox')).toContainText('Compact — 16 canaux');
  await expect(page.locator('#dmxFixChartBox')).toContainText('Complet — 32 canaux');
  await expect(page.locator('#dmxFixChartBox tr')).toHaveCount(2 + 16 + 32);

  const [download] = await Promise.all([page.waitForEvent('download'), page.locator('#dmxFixGdtf').click()]);
  expect(download.suggestedFilename()).toBe('Laser_Studio@Laser_Studio_DMX@v1.gdtf');
  // The venue file (T-500): a non-empty zip.
  await expect(page.locator('#dmxFixMvr')).toHaveText('Exporter MVR…');
  const [mvr] = await Promise.all([page.waitForEvent('download'), page.locator('#dmxFixMvr').click()]);
  expect(mvr.suggestedFilename()).toBe('Laser_Studio.mvr');
  const bytes = fs.readFileSync(await mvr.path());
  expect(bytes.length).toBeGreaterThan(1000);
  expect(bytes.subarray(0, 2).toString()).toBe('PK');

  await page.locator('#dmxFixAdd').click();
  await page.locator('#dmxFixAdd').click();
  await expect(page.locator('#dmxFixtures tr[data-i]')).toHaveCount(2);
  // « Complet » on the second one, then an address inside the first one.
  await page.locator('#dmxFixtures tr[data-i="1"] [data-k=mode]').selectOption('full');
  await expect.poll(async () => (await netin()).dmx.fixtures.list[1]?.last).toBe(48);
  await page.locator('#dmxFixtures tr[data-i="1"] [data-k=address]').fill('10');
  await page.locator('#dmxFixtures tr[data-i="1"] [data-k=address]').press('Tab');
  await expect(page.locator('#netinError')).toHaveText('Les adresses 10–41 chevauchent l\'appareil de la zone « Gauche ».');
  expect((await netin()).config.dmx.fixtures[1].address).toBe(17);
  expect((await studio.state()).armed).toBe(false);
});
