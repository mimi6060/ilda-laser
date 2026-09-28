// MIDI helpers for e2e tests (T-209): drive the simulated APC40 mkII of a
// studio started with --midi-test. Bytes go through POST /api/midi/inject,
// so they take the same path as a real controller's (decoder, mapping
// engine, T-208 safety rules). Layout: Akai's public APC40 mkII protocol.
import type { Studio } from './studio';

export const TEST_PORT = 'Test APC40 mkII';
export const NOTE_STOP_ALL = 0x51;
export const NOTE_SHIFT = 0x62;
export const NOTE_SCENE_1 = 0x52;
/** The button our test profile maps (under Shift) to transport.arm. */
export const NOTE_ARM = 0x5b;
export const CC_CUE_LEVEL = 0x2f;
export const DEVICE_INQUIRY = [0xf0, 0x7e, 0x7f, 0x06, 0x01, 0xf7];

/** Clip-launch pad (row 0 at the top, like the studio's grid) → note. */
export function padNote(row: number, col: number) {
  if (row < 0 || row > 4 || col < 0 || col > 7) throw new Error(`pad (${row},${col}) is off the grid`);
  return (4 - row) * 8 + col;
}

export interface SimDevice { port: string; model: string; mode: number | null; sent: number[][]; pads: (number | null)[][] }

export class Apc {
  constructor(private readonly studio: Studio, readonly port = TEST_PORT) {}

  /** Raw bytes "played" on the device. Throws unless the studio accepts them. */
  async inject(bytes: number[]) {
    const status = await this.studio.post('/api/midi/inject', { port: this.port, bytes });
    if (status !== 200) throw new Error(`inject ${bytes}: HTTP ${status}`);
  }

  button(note: number, down = true) { return this.inject(down ? [0x90, note, 0x7f] : [0x80, note, 0]); }
  pressPad(row: number, col: number) { return this.button(padNote(row, col), true); }
  releasePad(row: number, col: number) { return this.button(padNote(row, col), false); }
  shift(down: boolean) { return this.button(NOTE_SHIFT, down); }

  /** Fader n: 0–7 = tracks 1–8 (CC 7 on channel n), 8 = master (CC 14). */
  moveFader(n: number, value: number) {
    const v = Math.max(0, Math.min(127, Math.round(value)));
    return this.inject(n === 8 ? [0xb0, 0x0e, v] : [0xb0 | n, 0x07, v]);
  }

  /** Endless encoder `cc` turned by `delta` steps (two's complement). */
  turnKnob(cc: number, delta: number) {
    const d = Math.max(-64, Math.min(63, Math.round(delta)));
    return this.inject([0xb0, cc, d & 0x7f]);
  }

  async device(): Promise<SimDevice> {
    const sent = await this.studio.get<{ devices: SimDevice[] }>('/api/midi/sent');
    const dev = sent.devices.find(d => d.port === this.port);
    if (!dev) throw new Error(`no simulated device ${this.port}`);
    return dev;
  }

  /** Last LED value the studio sent to pad (row, col); null if never addressed. */
  async ledAt(row: number, col: number) { return (await this.device()).pads[row][col]; }
}

/**
 * A small profile in the spirit of T-204, written into the data dir before
 * the studio starts: 40 grid pads, Stop All → blackout, scene 1 → page 1
 * (Shift: page 2), track fader 1 → master size with pickup, master fader →
 * brightness, Cue Level encoder → position X, Shift + NOTE_ARM → arm.
 */
export function testProfileFiles(slug = 'e2e-apc') {
  const note = (number: number) => ({ kind: 'note', channel: 0, number });
  const mappings: object[] = [];
  for (let n = 0; n < 40; n++) mappings.push({ input: note(n), mode: 'grid', args: { slot: (4 - Math.floor(n / 8)) * 8 + (n % 8) } });
  mappings.push(
    { input: note(NOTE_STOP_ALL), target: 'transport.blackout', mode: 'trigger' },
    { input: note(NOTE_SCENE_1), target: 'page.1', mode: 'trigger' },
    { input: note(NOTE_SCENE_1), shift: true, target: 'page.2', mode: 'trigger' },
    { input: { kind: 'cc', channel: 0, number: 0x07 }, target: 'master.size', mode: 'absolute', pickup: true },
    { input: { kind: 'cc', channel: 0, number: 0x0e }, target: 'master.brightness', mode: 'absolute' },
    { input: { kind: 'cc', channel: 0, number: CC_CUE_LEVEL }, target: 'master.pos_x', mode: 'relative', step: 0.1 },
    { input: note(NOTE_ARM), shift: true, target: 'transport.arm', mode: 'trigger' },
  );
  const profile = { version: 1, name: 'APC40 mkII — e2e', driver: 'apc40mk2', match: { port_contains: [] }, host_mode: 0x41, shift_key: note(NOTE_SHIFT), mappings };
  const devices = { ports: { [TEST_PORT]: { profile: slug, enabled: true } } };
  return {
    [`midi/profiles/${slug}.json`]: JSON.stringify(profile, null, 2),
    'midi/devices.json': JSON.stringify(devices, null, 2),
  };
}
