// Test harness: starts a throw-away, preview-only Laser Studio.
//
// Safety (CLAUDE.md): the studio is never started with --device, always
// with --no-midi (never grab the user's controller) and --no-audio (never
// open the Mac's microphone, T-230), never on port 8080
// (the user's instance) and never with the user's studio-data/.
// Each spec file gets a fresh temporary --data-dir, deleted afterwards.
// MIDI specs add --midi-test (T-209): a simulated APC40 mkII fed by
// POST /api/midi/inject; still --no-midi, so no real port is ever opened.

import { test as base, expect, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';

export const REPO_ROOT = path.resolve(__dirname, '..', '..');
export const STUDIO_BIN = process.env.LASER_STUDIO_BIN ?? path.join(REPO_ROOT, 'target', 'debug', 'laser-studio');
const USER_PORT = 8080;

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
export interface Tempo { bpm: number; beat: number; bar: number; beat_in_bar: number; phase: number; beats_per_bar: number; source: 'manual' | 'tap' | 'audio'; follow: 'off' | 'waiting' | 'locked' | 'coasting' | 'unlocked'; confidence: number; detected_bpm: number; guide_bpm: number | null }
export interface Live { brightness: number; size: number; pos_x: number; pos_y: number; rot_angle: number[]; rot_speed: number[]; rot_sync: boolean; rot_reverse: boolean; speed: number; [k: string]: unknown }

export interface StudioOptions {
  /** Start with --midi-test: a simulated APC40 mkII and /api/midi/inject. */
  midiTest?: boolean;
  /** Files written into the fresh data dir before the first start (path → content). */
  files?: Record<string, string>;
  /** Pass the hidden --test-hooks flag (simulated engine stall, T-253; crashing decoder files, T-298). */
  testHooks?: boolean;
  /** With testHooks: the audio decoding time limit in ms (T-298; default 120 s). */
  decodeTimeoutMs?: number;
}

export class Studio {
  /** Created on the first start (not at import: Playwright imports spec files more than once). */
  dataDir = '';
  port = 0;
  private proc: ChildProcess | null = null;
  private log = '';

  constructor(private readonly opts: StudioOptions = {}) {}

  get url() { return `http://127.0.0.1:${this.port}`; }

  async start() {
    if (!this.dataDir) {
      this.dataDir = mkdtempSync(path.join(tmpdir(), 'laser-studio-e2e-'));
      for (const [rel, content] of Object.entries(this.opts.files ?? {})) {
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
    const args = ['--port', String(this.port), '--data-dir', this.dataDir, '--no-midi', '--no-audio'];
    if (this.opts.midiTest) args.push('--midi-test');
    if (this.opts.testHooks) args.push('--test-hooks');
    if (this.opts.testHooks && this.opts.decodeTimeoutMs) args.push('--test-decode-timeout-ms', String(this.opts.decodeTimeoutMs));
    if (args.includes('--device') || !args.includes('--no-midi') || !args.includes('--no-audio') || this.port === USER_PORT) throw new Error('refusing to start an unsafe studio');
    this.proc = spawn(STUDIO_BIN, args, { stdio: ['ignore', 'pipe', 'pipe'] });
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
    // Guard rails: a test studio is preview-only and starts disarmed.
    if (st.output !== null) throw new Error(`test studio has a laser output: ${st.output}`);
    if (st.armed !== false) throw new Error('test studio did not start disarmed');
    this.initialSettings ??= st.settings;
  }

  private initialSettings: unknown = null;

  /** Back to the start-up look and neutral live modifiers, disarmed. */
  async reset() {
    await this.post('/api/arm', { on: false });
    await this.post('/api/playlist/stop');
    await this.post('/api/settings', this.initialSettings);
    await this.post('/api/control', { id: 'master.reset' });
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

  /** Stop and start again on the same data directory (tests persistence). */
  async restart() {
    await this.stop();
    await this.start();
  }

  cleanup() {
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
  state() { return this.get('/api/state'); }
  frame() { return this.get<Frame>('/api/frame'); }
  live() { return this.get<Live>('/api/live'); }
  controlValues() { return this.get<{ values: Record<string, number | boolean>; cue_page: number; active_cue: string | null }>('/api/control-values'); }
  presets() { return this.get<{ categories: string[]; presets: { id: string; name: string; category: string }[] }>('/api/presets'); }
}

/**
 * One studio per spec file: started before the file's first test,
 * stopped and deleted after its last one. Always disarms at the end.
 */
export function useStudio(opts: StudioOptions = {}): Studio {
  const studio = new Studio(opts);
  base.beforeAll(async () => { await studio.start(); });
  base.afterAll(async () => {
    try { await studio.post('/api/arm', { on: false }); } catch { /* already stopped */ }
    await studio.stop();
    studio.cleanup();
  });
  return studio;
}

export type Workspace = 'live' | 'timeline' | 'creation' | 'settings';

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
export const test = base;
