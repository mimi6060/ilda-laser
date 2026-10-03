// PONK output (T-300): the studio streams to a local UDP receiver of this
// spec (127.0.0.1, a port the OS picks: never 5583, never multicast, never
// a real MadMapper). Disarmed it keeps sending empty frames; armed, the
// look; Escape goes back to empty frames at once.
import { test, expect, useStudio, openUi } from '../studio';
import { createSocket, type Socket } from 'node:dgram';

let rx: Socket;
let packets: Buffer[] = [];

// Registered before useStudio: the receiver is bound before the studio starts.
test.beforeAll(async () => {
  rx = createSocket('udp4');
  rx.on('message', m => { packets.push(m); });
  await new Promise<void>(resolve => rx.bind(0, '127.0.0.1', () => resolve()));
});
test.afterAll(() => { rx?.close(); });

const studio = useStudio({ ponkPort: () => rx.address().port });

/** Lit points in one PONK datagram (see github.com/madmappersoftware/Ponk). */
function litPoints(d: Buffer): number {
  expect(d.subarray(0, 8).toString('latin1')).toBe('PONK-UDP');
  expect(d[8]).toBe(0); // protocol version
  expect(d.subarray(13, 45).toString('utf8').replace(/\0+$/, '')).toBe('Laser Studio');
  // An empty frame always fits one datagram: a split one has points.
  if (d[46] !== 1) return Infinity;
  let off = 52, n = 0;
  while (off < d.length) {
    expect(d[off]).toBe(1); // XY_F32_RGB_U8
    off += 2 + d[off + 1] * 12; // format, metadata count, metadata
    const count = d.readUInt16LE(off);
    off += 2 + count * 11;
    n += count;
  }
  expect(off).toBe(d.length);
  return n;
}

/** Lit points of each datagram received during the next `ms`. */
async function listen(ms: number) {
  packets = [];
  await new Promise(r => setTimeout(r, ms));
  return packets.map(litPoints);
}

const armed = async () => (await studio.state()).armed as boolean;

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
  await openUi(page, studio);
});

test('the header shows the PONK output', async ({ page }) => {
  await expect(page.locator('#output')).toHaveText(/^Sortie : PONK → MadMapper \(127\.0\.0\.1:\d+\)$/);
  await expect(page.locator('#output')).toHaveAttribute('data-kind', 'ponk');
  await expect(page.locator('#output')).toHaveAttribute('title', /MadMapper/);
});

test('disarmed, it keeps sending empty frames', async () => {
  expect(await armed()).toBe(false);
  const lit = await listen(500);
  expect(lit.length).toBeGreaterThan(10); // ~60 a second, never silence
  expect(lit.every(n => n === 0)).toBe(true);
});

test('armed it sends the look, Escape blanks it at once', async ({ page }) => {
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await expect.poll(async () => Math.max(0, ...await listen(200))).toBeGreaterThan(0);
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  // Frames still flow, all empty.
  await new Promise(r => setTimeout(r, 100));
  const lit = await listen(300);
  expect(lit.length).toBeGreaterThan(5);
  expect(lit.every(n => n === 0)).toBe(true);
});
