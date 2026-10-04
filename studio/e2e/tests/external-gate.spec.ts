// The external inputs gate (T-408): closed at start, opened only from the
// UI (top-bar button or Alt+X), closed again by an emergency stop. OSC is
// sent from a loopback socket to a port the system picked (0): nothing
// goes to another host, and the studio never sends anything. Preview only.
import dgram from 'node:dgram';
import { test, expect, useStudio, openUi, openWorkspace, type Frame } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Source { source: string; groups: string[]; sets: string[]; require_access: boolean; access: boolean; refused: number }
interface External { open: boolean; estop: boolean; sources: Source[] }
const external = () => studio.get<External>('/api/external');
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

let port = 0;
async function send(address: string, args: (number | string)[] = []) {
  const sock = dgram.createSocket('udp4');
  await new Promise<void>((resolve, reject) => sock.send(osc(address, args), port, '127.0.0.1', e => (e ? reject(e) : resolve())));
  sock.close();
}

/** OSC on, on loopback with a system-picked port. */
async function oscOn() {
  await studio.post('/api/netin', { osc: { enabled: true, port: 0 } });
  await expect.poll(async () => (await studio.get<{ osc: { status: { local: string | null } } }>('/api/netin')).osc.status.local).not.toBeNull();
  port = Number((await studio.get<{ osc: { status: { local: string } } }>('/api/netin')).osc.status.local.split(':').pop());
}

let marker = 0;
/** Sends a message, then a numbered ping: once that ping shows in the
 * monitor, the message before it was handled (the queue is in order). */
async function sizeAfter(address: string, args: number[]) {
  await send(address, args);
  const n = ++marker;
  await send('/ls/ping', [n]);
  await expect.poll(async () => (await studio.get<{ osc: { recent: { address: string; args: unknown[] }[] } }>('/api/netin')).osc.recent[0]?.args[0]).toBe(n);
  return (await frame()).live.size;
}

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/external/gate', { on: false });
  await studio.post('/api/external/scopes', { scopes: [{ source: 'osc', groups: ['master', 'look', 'layer'], require_access: true }] });
  await studio.reset();
  await oscOn();
});

async function open(page: Page) {
  await openUi(page, studio);
  await expect(page.locator('#extGate')).toHaveAttribute('aria-pressed', 'false');
}

test('closed: OSC changes nothing; opened from the top bar with access raised, it does', async ({ page }) => {
  await open(page);
  expect((await external()).open).toBe(false);
  await send('/ls/access', [1]);
  expect(await sizeAfter('/ls/norm/master.size', [0.25])).toBe(1);
  expect((await external()).sources.find(s => s.source === 'osc')!.refused).toBeGreaterThan(0);

  await page.locator('#extGate').click();
  await expect(page.locator('#extGate')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#extGate')).toHaveClass(/open/);
  await send('/ls/access', [1]);
  await expect.poll(async () => sizeAfter('/ls/norm/master.size', [0.25])).toBeCloseTo(0.5, 5);
  // Access lowered: ignored again.
  await send('/ls/access', [0]);
  expect(await sizeAfter('/ls/norm/master.size', [1])).toBeCloseTo(0.5, 5);
  expect((await studio.state()).armed).toBe(false);
});

test('a masters-only scope cannot start a cue; ticking Cues in RÉGLAGES allows it', async ({ page }) => {
  await open(page);
  await page.keyboard.press('Alt+KeyX');
  await expect(page.locator('#extGate')).toHaveAttribute('aria-pressed', 'true');
  const cue = (await studio.presets()).presets[2].id;
  await send('/ls/access', [1]);
  await send('/ls/cue/play', [cue]);
  await sizeAfter('/ls/ping', []);
  expect((await frame()).active_cue).toBeNull();

  await openWorkspace(page, 'settings');
  await page.locator('#extPanel summary').click();
  const row = page.locator('#extTable tr[data-src="osc"]');
  await expect(row.locator('[data-set="masters"]')).toBeChecked();
  await expect(row.locator('[data-set="cues"]')).not.toBeChecked();
  await expect(row.locator('.ref')).not.toHaveText('0');
  await row.locator('[data-set="cues"]').check();
  await expect.poll(async () => (await external()).sources.find(s => s.source === 'osc')!.sets).toContain('cues');
  await send('/ls/access', [1]);
  await send('/ls/cue/play', [cue]);
  await expect.poll(async () => (await frame()).active_cue).toBe(cue);
  await expect(page.locator('#extPanel')).toContainText('Rien d\'externe ne peut armer le laser');
});

test('blackout from OSC with the gate closed latches the stop; the stop closes the gate and a reset does not reopen it', async ({ page }) => {
  await open(page);
  await send('/ls/blackout');
  await expect.poll(async () => (await studio.get<{ estop: { source: string } | null }>('/api/arm')).estop?.source).toBe('osc');
  // The engine has handled the message too (it trips the same latch).
  await expect.poll(async () => (await studio.get<{ osc: { recent: { address: string }[] } }>('/api/netin')).osc.recent[0]?.address).toBe('/ls/blackout');

  await studio.post('/api/estop/reset', {});
  await page.locator('#extGate').click();
  await expect(page.locator('#extGate')).toHaveAttribute('aria-pressed', 'true');
  await page.locator('#estopBtn').click();
  await expect(page.locator('#extGate')).toHaveAttribute('aria-pressed', 'false');
  // Opening is refused while the stop is latched.
  expect(await studio.post('/api/external/gate', { on: true })).toBe(409);
  await studio.post('/api/estop/reset', {});
  await page.waitForTimeout(1200);
  expect((await external()).open).toBe(false);
  await expect(page.locator('#extGate')).toHaveAttribute('aria-pressed', 'false');
  expect((await studio.state()).armed).toBe(false);
});

test('only the local UI opens it, and a restart closes it', async () => {
  expect(await studio.post('/api/external/gate', { on: true, source: 'osc' })).toBe(403);
  expect(await studio.post('/api/external/gate', { on: true, source: 'dmx' })).toBe(403);
  expect((await external()).open).toBe(false);
  expect(await studio.post('/api/external/gate', { on: true })).toBe(200);
  expect((await external()).open).toBe(true);
  await studio.restart();
  expect((await external()).open).toBe(false);
  // The scopes are kept (external.json), the gate is not.
  expect((await external()).sources.find(s => s.source === 'osc')!.require_access).toBe(true);
});
