// DMX output (T-453): a few named channels for a haze machine and simple
// lights, set in RÉGLAGES › Sorties et projecteurs › Sortie DMX, played by
// the timeline's DMX tracks and the LIVE « Brouillard » button. Off by
// default; the output universe can't be one the studio listens to (409);
// the e-stop puts every channel at 0, a disarm (« Noir ») every channel
// back to rest and the haze off, until a new explicit action. The "node" is this spec's own UDP
// receiver on 127.0.0.1 and a port the OS picks (never 6454, never a real
// node; the test studio refuses any address but the loopback anyway).
// Preview only: the laser is never armed.
import { test, expect, useStudio, openUi, openWorkspace, type Page } from '../studio';
import { createSocket, type Socket } from 'node:dgram';

let rx: Socket;
let packets: Buffer[] = [];

test.beforeAll(async () => {
  rx = createSocket('udp4');
  rx.on('message', m => { packets.push(m); });
  await new Promise<void>(resolve => rx.bind(0, '127.0.0.1', () => resolve()));
});
test.afterAll(() => { rx?.close(); });

const studio = useStudio();
const port = () => rx.address().port;

interface DmxOut { enabled: boolean; sending: boolean; values: Record<string, number>; estop: boolean; held: string | null; haze: { available: boolean; held: boolean; pump_on: boolean } }
const dmxOut = async () => (await studio.state()).dmx_out as DmxOut;
interface Ev { id: number; start: number; len: number; source: { kind: string; channel?: string; value?: number; id?: string } }
interface Show { name: string; tracks: { name: string; kind: string; events: Ev[] }[] }
const serverShow = async () => (await studio.get<{ show: Show }>('/api/timeline')).show;
const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

/** The 512 channels of every ArtDmx packet received during the next `ms`. */
async function listen(ms: number) {
  packets = [];
  await sleep(ms);
  return packets.map(d => {
    expect(d.subarray(0, 8).toString('latin1')).toBe('Art-Net\0');
    expect(d.readUInt16LE(8)).toBe(0x5000);
    expect(d.readUInt16LE(14)).toBe(1);
    expect(d.length).toBe(18 + 512);
    return [...d.subarray(18)];
  });
}

async function openDmxPanel(page: Page) {
  await openWorkspace(page, 'settings');
  await page.locator('#dmxOutPanel summary').evaluate(el => (el.parentElement as HTMLDetailsElement).open = true);
  await expect(page.locator('#doAddCh')).toBeVisible();
}

/** Page point of show time `u` in the event row of track `ti`. */
async function rowAt(page: Page, ti: number, u: number) {
  const top = await page.evaluate(i => (window as any).tlDebug.laneY(i), ti) as number;
  const view = await page.evaluate(() => (window as any).tlDebug.view());
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  return { x: box.x + (u - view.t0) * view.pps, y: box.y + top + view.lane / 2 };
}

test.beforeEach(async () => {
  await studio.post('/api/timeline/stop');
  await studio.post('/api/estop/reset');
  await studio.reset();
});

test.afterEach(async () => {
  await studio.post('/api/timeline/stop');
  expect((await studio.state()).armed, 'the DMX output never arms').toBe(false);
});

test('off by default, set in RÉGLAGES, self-drive refused, a DMX event plays, the e-stop zeroes all', async ({ page }) => {
  expect(port()).not.toBe(6454);
  // Off by default: nothing sent, no socket.
  const first = await studio.get('/api/dmx-out');
  expect(first.config.enabled).toBe(false);
  expect(first.status.to).toBeNull();
  expect(await listen(300)).toHaveLength(0);

  await openUi(page, studio);
  await openDmxPanel(page);
  await expect(page.locator('#doLive')).toHaveText('Arrêté');
  // Two named channels: the pump on 1, a light on 10 resting at 30.
  await page.locator('#doAddCh').click();
  await page.locator('#doAddCh').click();
  await page.getByLabel('Nom du canal 1').fill('Brouillard');
  await page.getByLabel('Nom du canal 2').fill('Face');
  await page.getByLabel('Adresse du canal 2').fill('10');
  await page.getByLabel('Valeur de repos du canal 2').fill('30');
  await page.locator('#doPump').selectOption('Brouillard');
  await page.locator('#doInterface').selectOption('127.0.0.1');
  await page.locator('#doTarget').fill(`127.0.0.1:${port()}`);
  await page.locator('#doSave').click();
  await expect(page.locator('#doMsg')).toContainText('Enregistré');
  const cfg = (await studio.get('/api/dmx-out')).config;
  expect(cfg.channels).toEqual([{ name: 'Brouillard', address: 1, rest: 0 }, { name: 'Face', address: 10, rest: 30 }]);
  expect([cfg.enabled, cfg.haze.pump]).toEqual([false, 'Brouillard']);

  // The studio listens to Art-Net universe 1: enabling the output there is refused.
  expect(await studio.post('/api/netin', { artnet: { enabled: true, universe: 1, port: 0 } })).toBe(200);
  await page.locator('#doEnabled').check();
  await expect(page.locator('#doMsg')).toContainText("L'univers de sortie doit être différent des univers d'entrée");
  await expect(page.locator('#doEnabled')).not.toBeChecked();
  expect((await studio.get('/api/dmx-out')).config.enabled).toBe(false);
  expect(await studio.post('/api/netin', { artnet: { enabled: false } })).toBe(200);

  await page.locator('#doEnabled').check();
  await expect(page.locator('#doMsg')).toContainText('sortie DMX activée');
  await expect(page.locator('#doLive')).toHaveText(/^Envoi : 30 trames\/s/);
  // At rest: the pump at 0, the light at 30, every other channel at 0.
  await expect.poll(async () => (await listen(200)).at(-1)?.slice(0, 12)).toEqual([0, 0, 0, 0, 0, 0, 0, 0, 0, 30, 0, 0]);

  // TIMELINE: a new show, « + Piste ▾ › Piste DMX », a double-click, then the event's properties.
  expect(await studio.post('/api/timeline/new', { name: 'DMX e2e', time_base: 'seconds' })).toBe(200);
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe('DMX e2e');
  await page.evaluate(() => (window as any).tlDebug.setView(0, 40));
  await page.locator('#tlAddTrack').click();
  await page.locator('#tlAddDmx').click();
  await expect.poll(async () => (await serverShow()).tracks.map(t => t.kind)).toEqual(['cues', 'cues', 'dmx']);
  const r = await rowAt(page, 2, 0.2);
  await page.mouse.dblclick(r.x, r.y);
  await expect.poll(async () => (await serverShow()).tracks[2].events.length).toBe(1);
  expect((await serverShow()).tracks[2].events[0].source).toEqual({ kind: 'dmx', channel: 'Brouillard', value: 255 });
  await expect(page.locator('#tlDmxRow')).toBeVisible();
  await page.locator('#tlDmxCh').selectOption('Face');
  await expect.poll(async () => (await serverShow()).tracks[2].events[0].source.channel).toBe('Face');
  await page.locator('#tlDmxVal').fill('200');
  await page.locator('#tlDmxVal').press('Enter');
  await expect.poll(async () => (await serverShow()).tracks[2].events[0].source.value).toBe(200);

  // A disarm (the reset before this test) holds the DMX events at rest
  // until the show is started armed: arm the (fake) output, then Lecture.
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await sleep(300);
  expect((await dmxOut()).values.Face).toBe(30);
  expect(await studio.post('/api/timeline/stop')).toBe(200);
  expect(await studio.post('/api/arm', { on: true, source: 'ui' })).toBe(200);
  // Playing: the light at 200 for the event, in /api/state and on the wire.
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await expect.poll(async () => (await dmxOut()).values.Face).toBe(200);
  await expect.poll(async () => (await listen(200)).at(-1)?.[9]).toBe(200);
  // Stopped: back to rest.
  expect(await studio.post('/api/timeline/stop')).toBe(200);
  await expect.poll(async () => (await dmxOut()).values.Face).toBe(30);

  // « Noir » (the top bar's button, a disarm) while playing: the rest value
  // at once on the wire, and held: starting again disarmed brings nothing back.
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await expect.poll(async () => (await dmxOut()).values.Face).toBe(200);
  await page.locator('#blackoutBtn').click();
  await expect.poll(async () => (await dmxOut()).values.Face).toBe(30);
  await expect.poll(async () => (await listen(200)).at(-1)?.[9]).toBe(30);
  expect((await dmxOut()).held).toContain('Laser désarmé');
  await expect(page.locator('#doLive')).toContainText('Laser désarmé');
  expect(await studio.post('/api/timeline/stop')).toBe(200);
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await sleep(300);
  expect((await dmxOut()).values.Face).toBe(30);
  expect((await listen(200)).every(d => d[9] === 30)).toBe(true);

  // E-stop: every channel at 0, in the state and on the wire.
  expect(await studio.post('/api/estop')).toBe(200);
  await expect.poll(async () => Object.values((await dmxOut()).values)).toEqual([0, 0]);
  await expect.poll(async () => (await listen(200)).every(d => d.every(v => v === 0))).toBe(true);
  expect((await dmxOut()).estop).toBe(true);
  // Reset: the rest values again (the timeline was halted, it doesn't restart by itself).
  expect(await studio.post('/api/estop/reset')).toBe(200);
  await expect.poll(async () => (await dmxOut()).values.Face).toBe(30);
});

test('LIVE: the Brouillard button and Alt+B hold the pump; disabling sends zeros then nothing', async ({ page }) => {
  // The settings of the first test, set over the API this time.
  expect(await studio.post('/api/dmx-out', {
    enabled: true, interface: '127.0.0.1', target: `127.0.0.1:${port()}`,
    channels: [{ name: 'Brouillard', address: 1, rest: 0 }, { name: 'Ventilo', address: 2, rest: 0 }],
    haze: { pump: 'Brouillard', fan: 'Ventilo', pump_level: 180, max_burst_s: 10 },
  })).toBe(200);
  await openUi(page, studio);
  await page.locator('#liveTabs [data-panel-tab="direct"]').click();
  await expect(page.locator('#hazeBox')).toBeVisible();
  await expect(page.locator('#hazeFan')).toBeVisible();
  // Held: the pump at its level; let go: 0.
  await page.locator('#hazeBtn').scrollIntoViewIfNeeded();
  const btn = (await page.locator('#hazeBtn').boundingBox())!;
  await page.mouse.move(btn.x + btn.width / 2, btn.y + btn.height / 2);
  await page.mouse.down();
  await expect.poll(async () => (await dmxOut()).values.Brouillard).toBe(180);
  await expect(page.locator('#hazeBtn')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#hazeState')).toHaveText('Pompe en marche');
  await page.mouse.up();
  await expect.poll(async () => (await dmxOut()).values.Brouillard).toBe(0);
  // Alt+B held (LIVE, outside a text field).
  await page.locator('#hazeBtn').blur();
  await page.keyboard.down('Alt');
  await page.keyboard.down('KeyB');
  await expect.poll(async () => (await dmxOut()).haze.held).toBe(true);
  await page.keyboard.up('KeyB');
  await page.keyboard.up('Alt');
  await expect.poll(async () => (await dmxOut()).haze.held).toBe(false);
  // The fan slider.
  await page.locator('#hazeFan').fill('50');
  await expect.poll(async () => (await dmxOut()).values.Ventilo).toBe(128);
  await expect.poll(async () => (await listen(200)).at(-1)?.[1]).toBe(128);
  // A disarm (any): the fan to 0 at once and kept there; the haze button
  // lets go. Only a new move of the fan (or a new press) brings it back.
  await page.mouse.move(btn.x + btn.width / 2, btn.y + btn.height / 2);
  await page.mouse.down();
  await expect.poll(async () => (await dmxOut()).values.Brouillard).toBe(180);
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
  await expect.poll(async () => [(await dmxOut()).values.Brouillard, (await dmxOut()).values.Ventilo]).toEqual([0, 0]);
  await expect.poll(async () => (await listen(200)).at(-1)?.slice(0, 2)).toEqual([0, 0]);
  await sleep(300);
  expect((await listen(300)).every(d => d[0] === 0 && d[1] === 0)).toBe(true);
  await page.mouse.up();
  await page.locator('#hazeFan').fill('50');
  await expect.poll(async () => (await dmxOut()).values.Ventilo).toBe(128);
  // Disabled: frames of zeros, then silence.
  packets = [];
  expect(await studio.post('/api/dmx-out', { enabled: false })).toBe(200);
  await sleep(300);
  const tail = packets.map(d => [...d.subarray(18)]);
  expect(tail.length).toBeGreaterThan(0);
  expect(tail.at(-1)!.every(v => v === 0)).toBe(true);
  expect(await listen(300)).toHaveLength(0);
  // Without a pump channel the box is hidden.
  expect(await studio.post('/api/dmx-out', { haze: { pump: null, fan: null } })).toBe(200);
  await expect(page.locator('#hazeBox')).toBeHidden();
});
