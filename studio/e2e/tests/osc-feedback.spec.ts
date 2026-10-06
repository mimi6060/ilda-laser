// OSC feedback, the address list and the monitor (T-416). The studio
// listens on 127.0.0.1 with a port the system picks, the test's client is
// a loopback socket, and the feedback goes only to that client: nothing
// ever goes to another host, nothing is broadcast. Preview only.
import dgram from 'node:dgram';
import AxeBuilder from '@axe-core/playwright';
import { test, expect, useStudio, openUi, openWorkspace, reveal } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface OscCfg { config: { enabled: boolean; targets: string[] }; subscribers: { addr: string; fixed: boolean }[] }
interface Address { address: string; control: string; kind: string; min: number | null; max: number | null }
interface Monitor { entries: { incoming: boolean; address: string; result: string }[] }
const oscCfg = () => studio.get<OscCfg>('/api/osc/config');
const monitor = () => studio.get<Monitor>('/api/osc/monitor');

function osc(address: string, args: number[] = []): Buffer {
  const pad = (b: Buffer) => Buffer.concat([b, Buffer.alloc(4 - (b.length % 4))]);
  const parts = [pad(Buffer.from(address)), pad(Buffer.from(',' + 'f'.repeat(args.length)))];
  for (const a of args) { const b = Buffer.alloc(4); b.writeFloatBE(a); parts.push(b); }
  return Buffer.concat(parts);
}

/** Address and first numeric argument of a message we received. */
function decode(buf: Buffer): { address: string; value: number | null } {
  const str = (at: number) => { const end = buf.indexOf(0, at); return { s: buf.toString('utf8', at, end), next: (end + 4) & ~3 }; };
  const a = str(0);
  const tags = str(a.next);
  const t = tags.s[1];
  const value = t === 'f' ? buf.readFloatBE(tags.next) : t === 'i' ? buf.readInt32BE(tags.next) : null;
  return { address: a.s, value };
}

/** A loopback client: what it received, and a way to send to the studio. */
async function client() {
  const sock = dgram.createSocket('udp4');
  const got: { address: string; value: number | null }[] = [];
  sock.on('message', m => got.push(decode(m)));
  await new Promise<void>(resolve => sock.bind(0, '127.0.0.1', resolve));
  const net = await studio.get<{ osc: { status: { local: string | null } } }>('/api/netin');
  const port = Number(net.osc.status.local?.split(':').pop());
  expect(port).toBeGreaterThan(0);
  const send = (b: Buffer) => new Promise<void>((resolve, reject) => sock.send(b, port, '127.0.0.1', e => (e ? reject(e) : resolve())));
  return { got, send, port: sock.address().port, close: () => sock.close() };
}

async function oscOn() {
  await studio.post('/api/netin', { osc: { enabled: true, port: 0 } });
  await expect.poll(async () => (await studio.get<{ osc: { status: { local: string | null } } }>('/api/netin')).osc.status.local).not.toBeNull();
}

async function openPanel(page: Page) {
  await openWorkspace(page, 'settings');
  const panel = page.locator('#oscFbPanel');
  if (!(await panel.evaluate(d => (d as HTMLDetailsElement).open))) await panel.locator('summary').click();
}

test.beforeEach(async () => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/netin', { osc: { enabled: false, port: 0 } });
  await studio.post('/api/osc/config', { enabled: false, targets: [], allow_subscribe: true, beat_messages: false, max_rate_hz: 30 });
  await studio.post('/api/osc/monitor/clear', {});
  const groups = ['master', 'look', 'layer', 'grid', 'cue', 'page', 'tempo', 'timeline', 'audio'];
  await studio.post('/api/external/scopes', { scopes: ['dmx', 'osc'].map(source => ({ source, groups, require_access: false })) });
  await studio.post('/api/external/gate', { on: true });
});

test('the address list comes from the registry, with no arming', async () => {
  const list = await studio.get<Address[]>('/api/osc/addresses');
  const size = list.find(a => a.control === 'master.size');
  expect(size).toMatchObject({ address: '/ls/master/size', kind: 'continuous', min: 0, max: 2 });
  expect(list.map(a => a.address)).toContain('/ls/tempo/bpm');
  expect(list.map(a => a.control)).not.toContain('transport.arm');
  expect(list.every(a => a.address.startsWith('/ls/') && !a.address.includes('arm'))).toBe(true);
  expect((await oscCfg()).config.enabled).toBe(false);
});

test('feedback: a subscriber sees the slider move; the monitor shows it; Copier l\'adresse OSC', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: studio.url });
  await oscOn();
  await openUi(page, studio);
  await openPanel(page);
  await expect(page.locator('#oscFbState')).toHaveText('Coupé');
  await page.locator('#oscFbEnabled').check();
  await expect.poll(async () => (await oscCfg()).config.enabled).toBe(true);
  await expect(page.locator('#oscFbState')).toContainText('Actif');

  const c = await client();
  try {
    await c.send(osc('/ls/subscribe', [c.port]));
    await expect.poll(async () => (await oscCfg()).subscribers.map(s => s.addr)).toEqual([`127.0.0.1:${c.port}`]);
    await expect(page.locator('#oscFbSubs')).toContainText(`127.0.0.1:${c.port}`);
    // The full state arrives on subscribing.
    await expect.poll(() => c.got.some(m => m.address === '/ls/master/size')).toBe(true);

    await reveal(page, '#mSize');
    await page.locator('#mSize').fill('150');
    await expect.poll(() => c.got.some(m => m.address === '/ls/master/size' && Math.abs((m.value ?? 0) - 1.5) < 1e-3)).toBe(true);

    await openPanel(page);
    await page.locator('#oscMonDir').selectOption('out');
    await expect(page.locator('#oscMonTable tbody tr', { hasText: '/ls/master/size' }).first()).toBeVisible();
    await expect(page.locator('#oscMonTable tbody tr', { hasText: '/ls/master/size' }).first()).toContainText('sortant');

    // The client's own value is not sent back to it.
    const before = c.got.filter(m => m.address === '/ls/master/size').length;
    await c.send(osc('/ls/master/size', [0.5]));
    await expect.poll(async () => (await studio.live()).size).toBeCloseTo(0.5, 3);
    await page.waitForTimeout(400);
    expect(c.got.filter(m => m.address === '/ls/master/size').length).toBe(before);
    await page.locator('#oscMonDir').selectOption('in');
    await expect(page.locator('#oscMonTable tbody tr', { hasText: '/ls/master/size' }).first()).toContainText('appliqué');
  } finally {
    c.close();
  }

  // The address list, searchable.
  await page.locator('#oscAddrBtn').click();
  await page.locator('#oscAddrSearch').fill('Taille maître');
  await expect(page.locator('#oscAddrTable tbody tr')).toHaveCount(1);
  await expect(page.locator('#oscAddrTable tbody tr')).toContainText('/ls/master/size');
  // The open panel (monitor, subscribers, address list) passes axe.
  const r = await new AxeBuilder({ page }).include('#oscFbPanel').withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = r.violations.filter(v => v.impact === 'serious' || v.impact === 'critical').map(v => `${v.id} ${v.nodes.slice(0, 3).map(n => n.target.join(' ')).join(', ')}`);
  expect(bad, bad.join('\n')).toEqual([]);

  // Right click on « Taille maître » › Copier l'adresse OSC.
  await reveal(page, '#mSize');
  await page.locator('#mSize').click({ button: 'right' });
  await page.locator('#midiMenuOsc').click();
  await expect(page.locator('#lockToast')).toContainText('Adresse copiée : /ls/master/size');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('/ls/master/size');
  expect((await studio.state()).armed).toBe(false);
});

test('off: nothing goes out, subscribing is refused; /ls/tempo/bpm sets the tempo', async ({ page }) => {
  await oscOn();
  const c = await client();
  try {
    await c.send(osc('/ls/subscribe', [c.port]));
    await expect.poll(async () => (await monitor()).entries.find(e => e.address === '/ls/subscribe')?.result).toBe('refusé');
    await studio.post('/api/control', { id: 'master.size', value: 1.7 });
    await c.send(osc('/ls/tempo/bpm', [128]));
    await expect.poll(async () => (await studio.get<{ tempo: { bpm: number } }>('/api/frame')).tempo.bpm).toBe(128);
    await page.waitForTimeout(500);
    expect(c.got).toEqual([]);
    expect((await oscCfg()).subscribers).toEqual([]);
    expect((await monitor()).entries.some(e => !e.incoming)).toBe(false);
  } finally {
    c.close();
  }
  // The settings refuse a broadcast destination.
  await openUi(page, studio);
  await openPanel(page);
  await page.locator('#oscFbTargets').fill('255.255.255.255:9000');
  await page.locator('#oscFbTargets').press('Enter');
  await page.locator('#oscFbRate').focus();
  await expect(page.locator('#oscFbError')).toContainText('destination invalide');
  expect((await oscCfg()).config.targets).toEqual([]);
  expect((await studio.state()).armed).toBe(false);
});
