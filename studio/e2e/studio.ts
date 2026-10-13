// Test harness: starts a throw-away, preview-only Laser Studio.
//
// Safety (CLAUDE.md): the studio is never started with --device, always
// with --no-midi (never grab the user's controller) and --no-audio (never
// open the Mac's microphone, T-230), never on port 8080
// (the user's instance) and never with the user's studio-data/.
// Each spec file gets a fresh temporary --data-dir, deleted afterwards.
// MIDI specs add --midi-test (T-209): a simulated APC40 mkII fed by
// POST /api/midi/inject; still --no-midi, so no real port is ever opened.
// Always --test-devices (T-303): a DAC scan returns a fixed list and never
// touches the network (no IDN/Ether Dream discovery) or the USB bus.
// T-401: arming needs a connected output, so a spec starts with the fake
// one (--test-output, a log file, never a laser) unless it asks for none;
// --test-devices also makes every « Connecter » open a fake output only.

import { test as base, expect, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { appendFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';

export const REPO_ROOT = path.resolve(__dirname, '..', '..');
/**
 * T-482: `npm run soak` sets LASER_STUDIO_SOAK=1 and runs a release build,
 * where the engine's « no frame over 50 ms » criterion applies.
 */
export const SOAK_RUN = process.env.LASER_STUDIO_SOAK === '1';
export const STUDIO_BIN = process.env.LASER_STUDIO_BIN ?? path.join(REPO_ROOT, 'target', SOAK_RUN ? 'release' : 'debug', 'laser-studio');
const USER_PORT = 8080;
/** `onboarding.json` of a data dir whose guided tour was seen (T-437). */
export const TOUR_SEEN = '{"tour_done":true,"tour_version":1}';
/** The name of the fake output (`--test-output`): never a laser. */
export const FAKE_OUTPUT = 'Sortie de test';

/**
 * T-358: the file where this run's studios write their PIDs (one per line),
 * set by global-setup.ts; global-teardown.ts stops any still alive.
 */
export const RUN_PIDS_ENV = 'LASER_STUDIO_E2E_PIDS';

function recordStudio(pid: number | undefined) {
  const file = process.env[RUN_PIDS_ENV];
  if (file && pid) appendFileSync(file, `${pid}\n`);
}

/** A free TCP port on localhost, never the user's 8080. */
async function freePort(): Promise<number> {
  for (;;) {
    const port = await new Promise<number>((resolve, reject) => {
      const srv = createServer();
      srv.unref();
      srv.on('error', reject);
      srv.listen(0, '127.0.0.1', () => {
        const addr = srv.address();
        const p = typeof addr === 'object' && addr ? addr.port : 0;
        srv.close(() => resolve(p));
      });
    });
    if (port && port !== USER_PORT) return port;
  }
}

export type Point = [number, number, number, number, number];
export interface Frame { points: Point[]; armed: boolean; output: string | null; cue_page: number; active_cue: string | null; playlist: number | null; tempo: Tempo; live: Live }
export interface Tempo { bpm: number; beat: number; bar: number; beat_in_bar: number; phase: number; beats_per_bar: number; source: 'manual' | 'tap' | 'audio'; follow: 'off' | 'waiting' | 'locked' | 'coasting' | 'unlocked'; confidence: number; detected_bpm: number; guide_bpm: number | null;
  downbeat: { beat_in_bar_offset: number; confidence: number; phrase_start: number | null } | null; downbeat_suggested: boolean; phrase_bar: number | null; phrase_bars: number }
export interface Live { brightness: number; size: number; pos_x: number; pos_y: number; rot_angle: number[]; rot_speed: number[]; rot_sync: boolean; rot_reverse: boolean; speed: number; [k: string]: unknown }

export interface StudioOptions {
  /** Start with --midi-test: a simulated APC40 mkII and /api/midi/inject. */
  midiTest?: boolean;
  /** Start with --test-ltc-out: LTC out goes to the simulated audio output « Test LTC » (T-501). */
  ltcTest?: boolean;
  /** Files written into the fresh data dir before the first start (path → content). */
  files?: Record<string, string>;
  /**
   * T-437: a fresh data dir shows the guided tour. By default the harness
   * writes `onboarding.json` (tour seen) so it never gets in the way;
   * `tour: true` leaves it out (onboarding.spec.ts).
   */
  tour?: boolean;
  /** Pass the hidden --test-interlock flag: an interlock that is never satisfied, so arming is always refused (T-264). */
  testInterlock?: boolean;
  /** Pass the hidden --test-hooks flag (simulated engine stall, T-253; crashing decoder files, T-298). */
  testHooks?: boolean;
  /** With testHooks: the audio decoding time limit in ms (T-298; default 120 s). */
  decodeTimeoutMs?: number;
  /**
   * T-300: start with --ponk 127.0.0.1:<port>, the port of the spec's own
   * local UDP receiver (asked at start: it is bound in a beforeAll). Never
   * 5583 (a MadMapper on this Mac listens there), never multicast.
   */
  ponkPort?: () => number;
  /**
   * T-258: pass the hidden --test-checklist flag: the pre-show checklist
   * gates arming as with a real output, while the studio stays preview
   * only. Without it (and without ponkPort) there is no checklist interlock.
   */
  testChecklist?: boolean;
  /** T-303: what a DAC scan "finds" (--test-devices; default: nothing). */
  devices?: { kind: string; id: string; name: string; ip?: string }[];
  /**
   * T-401: arming needs a connected output. By default the studio starts
   * with the fake one (--test-output, a log file next to the data dir,
   * never a laser) connected, so specs can arm; 'none' starts in preview
   * only (arming refused until an output is connected). Ignored with PONK.
   */
  output?: 'test' | 'none';
}

export class Studio {
  /** Created on the first start (not at import: Playwright imports spec files more than once). */
  dataDir = '';
  port = 0;
  private proc: ChildProcess | null = null;
  private log = '';

  constructor(private readonly opts: StudioOptions = {}) {}

  get url() { return `http://127.0.0.1:${this.port}`; }
  /** The studio process's PID while it runs (the soak reads its memory and open files). */
  get pid() { return this.proc?.pid ?? null; }

  async start() {
    if (!this.dataDir) {
      this.dataDir = mkdtempSync(path.join(tmpdir(), 'laser-studio-e2e-'));
      const seeds = { ...(this.opts.tour ? {} : { 'onboarding.json': TOUR_SEEN }), ...(this.opts.files ?? {}) };
      for (const [rel, content] of Object.entries(seeds)) {
        const file = path.join(this.dataDir, rel);
        if (!file.startsWith(this.dataDir + path.sep)) throw new Error(`seed file outside the data dir: ${rel}`);
        mkdirSync(path.dirname(file), { recursive: true });
        writeFileSync(file, content);
      }
    }
    this.port = await freePort();
    // Deliberately no --device: preview only, no laser output. --no-midi so
    // a test never opens the user's MIDI controller (--midi-test only adds
    // a simulated one); --no-audio so it never opens an audio input.
    // --test-keep-awake: a fake sleep blocker, so arming or playing in a
    // test never holds a power assertion on this Mac (T-412).
    // --exit-with-parent (T-358): the studio stops by itself (cleanly) if
    // this worker dies or the data dir is deleted, so an interrupted run
    // leaves no studio behind.
    const args = ['--port', String(this.port), '--data-dir', this.dataDir, '--no-midi', '--no-audio', '--test-keep-awake', '--exit-with-parent', String(process.pid)];
    args.push('--test-devices', JSON.stringify(this.opts.devices ?? []));
    // T-428: a tablet remote turned on in a test listens on loopback only,
    // never on this Mac's network interfaces.
    args.push('--remote-bind', '127.0.0.1');
    // T-456: a main machine's sync listener, too, on loopback only.
    args.push('--standby-bind', '127.0.0.1');
    if (this.opts.midiTest) args.push('--midi-test');
    if (this.opts.ltcTest) args.push('--test-ltc-out');
    // The fake output (T-401), unless PONK is the output or the spec wants none.
    const fakeOutput = this.opts.ponkPort === undefined && this.opts.output !== 'none';
    if (fakeOutput) args.push('--test-output', this.outputLog);
    if (this.opts.testHooks) args.push('--test-hooks');
    if (this.opts.testChecklist) args.push('--test-checklist');
    if (this.opts.testInterlock) args.push('--test-interlock');
    if (this.opts.testHooks && this.opts.decodeTimeoutMs) args.push('--test-decode-timeout-ms', String(this.opts.decodeTimeoutMs));
    const ponkPort = this.opts.ponkPort?.();
    if (ponkPort !== undefined) {
      if (!ponkPort || ponkPort === 5583) throw new Error(`refusing PONK port ${ponkPort}`);
      args.push('--ponk', `127.0.0.1:${ponkPort}`);
    }
    if (args.includes('--device') || args[args.indexOf('--remote-bind') + 1] !== '127.0.0.1' || args[args.indexOf('--standby-bind') + 1] !== '127.0.0.1' || !args.includes('--test-devices') || !args.includes('--no-midi') || !args.includes('--no-audio') || this.port === USER_PORT) throw new Error('refusing to start an unsafe studio');
    this.proc = spawn(STUDIO_BIN, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    recordStudio(this.proc.pid);
    this.proc.stdout!.on('data', d => { this.log += d; });
    this.proc.stderr!.on('data', d => { this.log += d; });
    const deadline = Date.now() + 15_000;
    for (;;) {
      if (this.proc.exitCode !== null) throw new Error(`studio exited early:\n${this.log}`);
      try {
        const r = await fetch(`${this.url}/api/state`);
        if (r.ok) break;
      } catch { /* not listening yet */ }
      if (Date.now() > deadline) throw new Error(`studio did not start:\n${this.log}`);
      await new Promise(r => setTimeout(r, 50));
    }
    const st = await this.state();
    // Guard rails: a test studio drives no laser (the fake output, PONK to
    // the spec's own receiver, or nothing) and starts disarmed.
    const expectedOutput = ponkPort !== undefined ? `PONK → MadMapper (127.0.0.1:${ponkPort})` : fakeOutput ? FAKE_OUTPUT : null;
    // A remembered choice among the --test-devices may connect (a fake output) in the background.
    const fakeDevices = (this.opts.devices ?? []).map(d => d.name);
    if (st.output !== expectedOutput && !fakeDevices.includes(st.output)) throw new Error(`test studio has a laser output: ${st.output}`);
    if (st.armed !== false) throw new Error('test studio did not start disarmed');
    this.initialSettings ??= st.settings;
  }

  private initialSettings: unknown = null;

  /** Back to the start-up look and neutral live modifiers, disarmed. */
  async reset() {
    await this.post('/api/arm', { on: false });
    await this.post('/api/lock', { show_mode: false }); // T-283: edits below would get 423
    await this.post('/api/playlist/stop');
    await this.post('/api/settings', this.initialSettings);
    await this.post('/api/control', { id: 'master.reset' });
  }

  /** T-426: back to the empty « Défaut » operator (views and interface preferences). */
  async resetOperator() {
    try {
      await this.post('/api/operators/current', { name: 'Défaut' });
      await this.post('/api/operators/' + encodeURIComponent('Défaut'), { views: [], prefs: {} });
    } catch { /* studio not running (a spec that restarts it) */ }
  }

  /** Ctrl+C (the studio's clean shutdown), then SIGKILL if it hangs. */
  async stop() {
    const p = this.proc;
    this.proc = null;
    if (!p || p.exitCode !== null) return;
    const exited = new Promise<void>(resolve => p.once('exit', () => resolve()));
    p.kill('SIGINT');
    const timer = setTimeout(() => p.kill('SIGKILL'), 5_000);
    await exited;
    clearTimeout(timer);
  }

  /** SIGKILL, like a crash or a power cut: no clean shutdown (T-287). */
  async kill() {
    const p = this.proc;
    this.proc = null;
    if (!p || p.exitCode !== null) return;
    const exited = new Promise<void>(resolve => p.once('exit', () => resolve()));
    p.kill('SIGKILL');
    await exited;
  }

  /** Stop and start again on the same data directory (tests persistence). */
  async restart() {
    await this.stop();
    await this.start();
  }

  /** The fake output's call log (arm, disarm, lit, dark, blank, close). */
  get outputLog() { return this.dataDir + '.output.log'; }

  cleanup() {
    if (this.dataDir) rmSync(this.outputLog, { force: true });
    if (this.dataDir) rmSync(this.dataDir, { recursive: true, force: true });
    this.dataDir = '';
  }

  async get<T = any>(p: string): Promise<T> {
    const r = await fetch(this.url + p);
    if (!r.ok) throw new Error(`GET ${p}: ${r.status}`);
    return r.json() as Promise<T>;
  }
  async post(p: string, body: unknown = {}) {
    const r = await fetch(this.url + p, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    return r.status;
  }
  /**
   * T-258: fill in and complete the pre-show checklist through the API, as
   * an operator would (for specs whose output gates arming on it: PONK,
   * --test-checklist). Never arms.
   */
  async completeChecklist(operator = 'Test e2e') {
    const c = await this.get<{ complete: boolean; items: { id: string; visible: boolean; done: boolean; needs_value: boolean }[] }>('/api/checklist');
    if (c.complete) return;
    if (await this.post('/api/checklist/operator', { name: operator }) !== 200) throw new Error('checklist operator refused');
    for (const it of c.items.filter(i => i.visible && !i.done)) {
      if (await this.post('/api/checklist/check', { id: it.id, on: true, value: it.needs_value ? 'E2E' : null }) !== 200) throw new Error(`checklist item ${it.id} refused`);
    }
    if (await this.post('/api/checklist/complete') !== 200) throw new Error('checklist not completed');
  }
  /**
   * T-255: with a real output, arming also waits until the operator said
   * where the audience is. Adds a « Public » zone along the bottom edge
   * (blanked outside audience scanning), as the Sécurité panel would.
   */
  async declareAudience() {
    const { settings } = await this.get('/api/safety');
    if (settings.zones.some((z: { audience: boolean }) => z.audience)) return;
    settings.zones.push({ id: 0, name: 'Public', kind: 'blank', level: 0, points: [[-1, -1], [1, -1], [1, -0.95], [-1, -0.95]], audience: true });
    if (await this.post('/api/safety', settings) !== 200) throw new Error('Public zone refused');
  }
  state() { return this.get('/api/state'); }
  frame() { return this.get<Frame>('/api/frame'); }
  live() { return this.get<Live>('/api/live'); }
  controlValues() { return this.get<{ values: Record<string, number | boolean>; cue_page: number; active_cue: string | null }>('/api/control-values'); }
  presets() { return this.get<{ categories: string[]; presets: { id: string; name: string; category: string; tag?: string }[] }>('/api/presets'); }
}

/**
 * One studio per spec file: started before the file's first test,
 * stopped and deleted after its last one. Always disarms at the end.
 */
export function useStudio(opts: StudioOptions = {}): Studio {
  const studio = new Studio(opts);
  base.beforeAll(async () => { await studio.start(); });
  // T-426: each test opens a fresh browser, which takes the operator's
  // preferences from the studio; start every test from an empty profile.
  base.beforeEach(async () => { await studio.resetOperator(); });
  base.afterAll(async () => {
    try { await studio.post('/api/arm', { on: false }); } catch { /* already stopped */ }
    await studio.stop();
    studio.cleanup();
  });
  return studio;
}

/**
 * Every write this page sends to the studio, for « display only, nothing
 * sent » checks. Two kinds run on their own and are kept apart:
 * - the heartbeat (T-252), left out;
 * - the operator's interface preferences (T-426): a display change made on
 *   the page (2D/3D, a 3D layer switch…) is remembered and, 500 ms later,
 *   saved to `POST /api/operators/<nom>`. These go to `prefSaves`, with
 *   their body, so a spec checks they only carry `laserStudio.*`
 *   preferences (see `expectOnlyPrefs`), never a look, cue or output.
 */
export function watchWrites(page: Page) {
  const writes: string[] = [];
  const prefSaves: { path: string; body: { views?: unknown[]; prefs?: Record<string, unknown> } }[] = [];
  page.on('request', q => {
    if (q.method() === 'GET') return;
    const url = new URL(q.url());
    if (url.pathname === '/api/heartbeat') return;
    const op = /^\/api\/operators\/(.+)$/.exec(url.pathname);
    if (q.method() === 'POST' && op && !['current', 'delete'].includes(decodeURIComponent(op[1]))) {
      prefSaves.push({ path: url.pathname, body: q.postDataJSON() });
      return;
    }
    writes.push(q.method() + ' ' + q.url());
  });
  return { writes, prefSaves };
}

/** The operator saves since `from` only carry interface preferences of the « Défaut » operator, views untouched. */
export function expectOnlyPrefs(saves: ReturnType<typeof watchWrites>['prefSaves'], from = 0, views: unknown[] = []) {
  for (const s of saves.slice(from)) {
    expect(s.path).toBe('/api/operators/' + encodeURIComponent('Défaut'));
    expect(s.body.views).toEqual(views);
    expect(Object.keys(s.body.prefs ?? {}).every(k => k.startsWith('laserStudio.'))).toBe(true);
  }
}

export type Workspace = 'live' | 'timeline' | 'creation' | 'settings' | 'desk';

/** Click a workspace tab (T-295: LIVE, TIMELINE, CRÉATION, RÉGLAGES). */
export async function openWorkspace(page: Page, ws: Workspace) {
  await page.locator(`#wsTabs [data-ws-tab="${ws}"]`).click();
  await expect(page.locator(`#wsTabs [data-ws-tab="${ws}"]`)).toHaveClass(/active/);
}

/**
 * Show the workspace (and the LIVE panel tab) holding `selector`, by
 * clicking their tabs like a user would. Controls of a hidden workspace
 * can't be clicked or typed in.
 */
export async function reveal(page: Page, selector: string) {
  const where = await page.locator(selector).first().evaluate(el => ({
    ws: (el.closest('[data-ws]') as HTMLElement | null)?.dataset.ws ?? null,
    panel: (el.closest('[data-panel]') as HTMLElement | null)?.dataset.panel ?? null,
    cpanel: (el.closest('[data-cpanel]') as HTMLElement | null)?.dataset.cpanel ?? null,
  }));
  if (where.ws) await openWorkspace(page, where.ws as Workspace);
  if (where.panel) await page.locator(`#liveTabs [data-panel-tab="${where.panel}"]`).click();
  // CRÉATION sub-tabs (T-296): Look / Figures.
  if (where.cpanel) await page.locator(`#creationTabs [data-ctab="${where.cpanel}"]`).click();
  await expect(page.locator(selector).first()).toBeVisible();
}

/** Open the UI on the LIVE workspace and wait until it has loaded its state and cue grid. */
export async function openUi(page: Page, studio: Studio) {
  await page.goto(studio.url + '/');
  await openWorkspace(page, 'live');
  await expect(page.locator('#shapes button')).not.toHaveCount(0);
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await expect(page.locator('#stPoints')).not.toHaveText('0');
}

/**
 * T-503: the 3D settings (viewpoints, exposure, haze, safety overlays)
 * are a drawer over the view, closed by default; open it like the
 * operator does, with « Réglages 3D ». The view must already be in 3D.
 */
export async function open3dSettings(page: Page) {
  const btn = page.locator('#render3dBtn');
  await expect(btn).toBeVisible();
  if ((await btn.getAttribute('aria-expanded')) !== 'true') await btn.click();
  await expect(page.locator('#render3d')).toBeVisible();
}

/** T-506: the cue bar's rare settings sit behind « Plus… »; unfold them. */
export async function openCueMore(page: Page) {
  const btn = page.locator('#cueMoreBtn');
  if ((await btn.getAttribute('aria-expanded')) !== 'true') await btn.click();
  await expect(page.locator('#cueMore')).toBeVisible();
}

/** Move keyboard focus off any input, so the page's shortcuts apply. */
export async function focusPage(page: Page) {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.locator('h1').click();
}

/** Largest |x| or |y| among lit points: how big the drawing is. */
export function extent(points: Point[]) {
  return Math.max(0, ...points.filter(isLit).map(p => Math.max(Math.abs(p[0]), Math.abs(p[1]))));
}
export function isLit(p: Point) { return p[2] + p[3] + p[4] > 0; }
export function centroid(points: Point[]) {
  const lit = points.filter(isLit);
  const n = lit.length || 1;
  return { x: lit.reduce((a, p) => a + p[0], 0) / n, y: lit.reduce((a, p) => a + p[1], 0) / n };
}

export { expect };
/**
 * T-433: the page asks before closing or reloading with unsaved work (a
 * `beforeunload` dialog). Specs reload pages with a modified project, so
 * that question is accepted by default. Other dialogs keep Playwright's
 * default (dismissed) unless the spec listens for them itself.
 */
export const test = base.extend({
  page: async ({ page }, use) => {
    page.on('dialog', d => {
      if (d.type() === 'beforeunload') d.accept().catch(() => {});
      else if (page.listenerCount('dialog') === 1) d.dismiss().catch(() => {});
    });
    await use(page);
  },
});
