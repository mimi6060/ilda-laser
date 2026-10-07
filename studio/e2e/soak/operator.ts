// Endurance soak of the interface (T-482, the browser half of T-449).
//
// A scripted operator, driven by a seeded generator, clicks through the
// real UI for a configurable time: every workspace and panel, cue pages
// and cues, slider drags (latest wins, T-484), tap tempo and BPM, the
// timeline examples, dialogs (opened and closed, sometimes with Échap),
// project save-as / open, a second tab coming and going, and explicit
// arm → e-stop → reset cycles on the fake output (never a laser).
//
// Meanwhile the harness watches:
// - the page: no uncaught error, no crash, its JS heap and DOM counters
//   after a forced garbage collection (CDP), its longest long task;
// - the studio: `/api/state` latency (it stays responsive), `armed` only
//   inside an explicit arm window, the fake output's log (never more arms
//   than asked, never lit while disarmed), the engine's frame times
//   (`/api/perf`, T-448), its memory (`footprint`) and open files (`lsof`).
//
// Preview only: the studio comes from studio.ts (no --device, free port,
// temp data dir, fake output). Nothing here talks to anything but it.

import { execFile } from 'node:child_process';
import { loadavg } from 'node:os';
import { promisify } from 'node:util';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { test, type CDPSession, type Page } from '@playwright/test';
import { expect, focusPage, openUi, openWorkspace, reveal, STUDIO_BIN, type Studio, type Workspace } from '../studio';

export interface SoakOptions {
  /** Total length of the run, warm-up included. */
  minutes: number;
  /** The first memory sample counted as the reference is taken after this. */
  warmupMinutes: number;
  /** Memory, heap and file samples. */
  sampleEverySec: number;
  seed: number;
  /** Allowed growth of the studio's memory between reference and end (0.05 = 5 %). */
  studioMemTolerance: number;
  /** Plus this much (kB): the allocator's pages and the stores filling up to their caps. */
  studioMemSlackKb: number;
  /** Allowed growth of the page's JS heap (after GC) between reference and end. */
  pageHeapTolerance: number;
  /** The engine's worst frame must stay under this (ms); null = report only. */
  frameLimitMs: number | null;
  /** `/api/state` must answer within this (ms). */
  stateLimitMs: number;
  /** One UI task longer than this counts as a freeze (ms). */
  longTaskLimitMs: number;
  /** Print a line per sample. */
  verbose: boolean;
}

export interface MemSample {
  atS: number;
  studioKb: number;
  files: number;
  heapKb: number;
  nodes: number;
  listeners: number;
  worstFrameMs: number;
  lateFrames: number;
  /** The Mac's load average (1 min): other work on the machine. */
  load: number;
}

export interface SoakResult {
  opts: SoakOptions;
  release: boolean;
  durationS: number;
  steps: Record<string, number>;
  slowestStep: { name: string; ms: number };
  samples: MemSample[];
  /** Floors (lowest values) of the first third of the samples after the warm-up, and of the last third. */
  reference: MemSample | null;
  last: MemSample | null;
  /** Index of the first sample after the warm-up. */
  refIndex: number;
  /** The elements (by id) whose node count grew most from the first sample after the warm-up to the last. */
  domGrowth: { id: string; from: number; to: number }[];
  pageErrors: string[];
  stateRequests: number;
  worstStateMs: number;
  worstFrameMs: number;
  worstFrameAfterWarmupMs: number;
  lateFramesAfterWarmup: number;
  framesAfterWarmup: number;
  /** Each time the late-frame count went up: when, the stages' worst times, and the operator's step then. */
  lateEvents: { atS: number; late: number; frameMs: number; renderMs: number; safetyMs: number; outputMs: number; step: string }[];
  longestTaskMs: number;
  longTasks: number;
  /** The page's long tasks over 200 ms: when, how long, and the operator's step then. */
  longTaskEvents: { atS: number; ms: number; step: string }[];
  /** Slowest answer of the page to a probe (an animation frame), ms. */
  worstPageMs: number;
  explicitArms: number;
  outputArms: number;
  armedOutsideWindow: number;
  litWhileDisarmed: number;
  estops: number;
  worstEstopMs: number;
  /** Seconds the harness stood still (the Mac asleep): such a run is not valid. */
  sleptS: number;
  failures: string[];
}

/** xorshift32: small, seeded, reproducible. */
export class Rng {
  private s: number;
  constructor(seed: number) { this.s = (seed >>> 0) || 0x9e3779b9; }
  next() {
    let x = this.s;
    x ^= x << 13; x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5; x >>>= 0;
    this.s = x;
    return x / 0x100000000;
  }
  int(n: number) { return Math.floor(this.next() * n); }
  pick<T>(a: readonly T[]): T { return a[this.int(a.length)]; }
  between(lo: number, hi: number) { return lo + this.next() * (hi - lo); }
}

/** An error's first line and its locator, without terminal colours. */
function brief(e: unknown) {
  const lines = String((e as Error)?.message ?? e).replace(/\u001b\[[0-9;]*m/g, '').split('\n').map(l => l.trim()).filter(Boolean);
  return [lines[0], ...lines.filter(l => /^(Locator|Expected|Received):/.test(l))].join(' · ');
}

const run = promisify(execFile);
/** `p`, or an error if it takes over 60 s (a page that no longer answers is a hang too). */
function within<T>(p: Promise<T>, what: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  const late = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`${what}: no answer in 60 s`)), 60_000); });
  return Promise.race([p, late]).finally(() => clearTimeout(timer));
}
const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

/**
 * Runs a measuring tool without blocking the event loop (the watcher keeps
 * polling meanwhile), killed after 30 s: `footprint` once hung for 15 min
 * on a loaded Mac, and a synchronous call froze the whole harness.
 */
async function tool(cmd: string, args: string[]): Promise<string | null> {
  try {
    const { stdout } = await run(cmd, args, { encoding: 'utf8', timeout: 30_000, maxBuffer: 16 << 20 });
    return stdout;
  } catch {
    return null;
  }
}

/** macOS `footprint` (what Activity Monitor shows), else the RSS, in kB; -1 if neither answers. */
export async function processKb(pid: number): Promise<number> {
  const text = await tool('footprint', ['-f', 'bytes', '--noCategories', String(pid)]);
  const m = text && /Footprint:\s*(\d+)\s*B\b/.exec(text);
  if (m) return Math.round(+m[1] / 1024);
  const rss = await tool('ps', ['-o', 'rss=', '-p', String(pid)]);
  return rss ? parseInt(rss.trim(), 10) || -1 : -1;
}

/** Open file descriptors of a process (`lsof`), -1 if it can't be read. */
export async function openFiles(pid: number): Promise<number> {
  const out = await tool('lsof', ['-n', '-P', '-p', String(pid)]);
  return out ? Math.max(0, out.trim().split('\n').length - 1) : -1;
}

/**
 * The fake output's log: how many times it was armed, and how many times
 * it went lit while disarmed (must be 0).
 */
export function readOutputLog(file: string) {
  if (!existsSync(file)) return { arms: 0, litWhileDisarmed: 0 };
  let armed = false, arms = 0, litWhileDisarmed = 0;
  for (const line of readFileSync(file, 'utf8').split('\n')) {
    if (line === 'arm') { armed = true; arms++; }
    else if (line === 'disarm' || line === 'close') armed = false;
    else if (line === 'lit' && !armed) litWhileDisarmed++;
  }
  return { arms, litWhileDisarmed };
}

const WORKSPACES: Workspace[] = ['live', 'timeline', 'creation', 'settings', 'desk'];
const LIVE_PANELS = ['direct', 'fx', 'lfo', 'music', 'running', 'scenes', 'pools', 'macros', 'cuelist'];
const CREATION_TABS = ['look', 'figures', 'abstract', 'program', 'media', 'macros'];
/** Sliders a show operator drags: masters, speed, layer dimmers, the look. */
const SLIDERS = ['#topMaster', '#mSize', '#mPosX', '#mPosY', '#mSpeed', '#mBright', '#speedRate', '#layerDim1', '#layerDim2', '#layerDim3', '#layerDim4', '#scale', '#rot', '#bright'];

export async function runSoak(page: Page, studio: Studio, opts: SoakOptions): Promise<SoakResult> {
  const rng = new Rng(opts.seed);
  const pid = studio.pid;
  if (!pid) throw new Error('the studio is not running');
  const t0 = Date.now();
  const end = t0 + opts.minutes * 60_000;
  const warmEnd = t0 + opts.warmupMinutes * 60_000;
  const atS = () => Math.round((Date.now() - t0) / 100) / 10;
  const r: SoakResult = {
    opts, release: STUDIO_BIN.split(path.sep).includes('release'),
    durationS: 0, steps: {}, slowestStep: { name: '', ms: 0 }, samples: [], reference: null, last: null, refIndex: -1, domGrowth: [], pageErrors: [],
    stateRequests: 0, worstStateMs: 0, worstFrameMs: 0, worstFrameAfterWarmupMs: 0, lateFramesAfterWarmup: 0, framesAfterWarmup: 0,
    lateEvents: [], longTaskEvents: [], longestTaskMs: 0, longTasks: 0, worstPageMs: 0, explicitArms: 0, outputArms: 0, armedOutsideWindow: 0, litWhileDisarmed: 0, estops: 0, worstEstopMs: 0, sleptS: 0, failures: [],
  };
  const fail = (msg: string) => { if (r.failures.length < 50) r.failures.push(`${atS()} s: ${msg}`); };

  // ---- the page ----
  page.on('pageerror', e => r.pageErrors.push(`${atS()} s: ${e.message}`));
  page.on('crash', () => r.pageErrors.push(`${atS()} s: the page crashed`));
  // Long tasks (the UI frozen for that long), from the page's own observer.
  await page.addInitScript(() => {
    const w = window as unknown as { __soakLongest: number; __soakLong: number; __soakLongList: [number, number][] };
    w.__soakLongest = 0;
    w.__soakLong = 0;
    w.__soakLongList = [];
    try {
      new PerformanceObserver(list => {
        for (const e of list.getEntries()) {
          w.__soakLongest = Math.max(w.__soakLongest, e.duration);
          w.__soakLong++;
          if (e.duration > 200 && w.__soakLongList.length < 200) w.__soakLongList.push([performance.timeOrigin + e.startTime, e.duration]);
        }
      }).observe({ type: 'longtask', buffered: true });
    } catch { /* no long task API */ }
  });
  // A step that waits longer than this is a hang: fail it, don't wait out the test.
  page.setDefaultTimeout(15_000);
  await openUi(page, studio);
  // Loading the page itself is not the operator's freeze.
  await page.evaluate(() => { const w = window as unknown as { __soakLongest: number; __soakLong: number }; w.__soakLongest = 0; w.__soakLong = 0; (w as unknown as { __soakLongList: unknown[] }).__soakLongList = []; });
  const cdp: CDPSession = await page.context().newCDPSession(page);
  await cdp.send('HeapProfiler.enable');

  // ---- what the operator is doing (for the late-frame log) ----
  let currentStep = 'start';
  /** When each step started (epoch ms), to tell which one a long task fell in. */
  const stepLog: [number, string][] = [[Date.now(), 'tour']];
  // ---- explicit arm window: the only time `armed` may be true ----
  let armWindow = false;

  // ---- the watcher: state every 250 ms, perf every 500 ms, a page probe every 2 s ----
  let watching = true;
  let perfStart: { late: number; frames: number } | null = null;
  let lastPerf = { late: 0, frames: 0, worst: 0 };
  const watcher = (async () => {
    let n = 0;
    // One pass takes about 250 ms (state, perf: 10 s at most). A pass
    // much longer means everything stood still, the studio's engine too:
    // the Mac went to sleep (a laptop left alone, even on AC). Such a run
    // measures nothing.
    let last = Date.now();
    while (watching) {
      const gap = Date.now() - last;
      if (gap > 30_000) {
        r.sleptS += Math.round(gap / 1000);
        fail(`everything stood still for ${Math.round(gap / 1000)} s (the Mac asleep?): the run is not valid`);
      }
      last = Date.now();
      const allowed = armWindow;
      const s0 = performance.now();
      try {
        const res = await fetch(`${studio.url}/api/state`, { signal: AbortSignal.timeout(10_000) });
        const st = await res.json() as { armed: boolean };
        const ms = performance.now() - s0;
        r.stateRequests++;
        r.worstStateMs = Math.max(r.worstStateMs, ms);
        if (ms > opts.stateLimitMs) fail(`/api/state took ${ms.toFixed(0)} ms`);
        if (st.armed && !allowed && !armWindow) { r.armedOutsideWindow++; fail('armed outside an explicit arm window'); }
      } catch (e) {
        fail(`/api/state failed: ${(e as Error).message}`);
      }
      if (n % 8 === 4) {
        // The page answers: one animation frame, measured from here.
        const p0 = performance.now();
        try {
          await page.evaluate(() => new Promise(res => requestAnimationFrame(() => res(0))));
          r.worstPageMs = Math.max(r.worstPageMs, performance.now() - p0);
        } catch { /* the page is navigating: the next probe */ }
      }
      if (n++ % 2 === 0) {
        try {
          type Stage = { worst_ms: number };
          const p = await studio.get<{ frame: Stage; render: Stage; safety: Stage; output: Stage; late_frames: number; frames: number }>('/api/perf');
          if (p.late_frames > lastPerf.late && r.lateEvents.length < 200) {
            r.lateEvents.push({ atS: atS(), late: p.late_frames - lastPerf.late, frameMs: p.frame.worst_ms, renderMs: p.render.worst_ms, safetyMs: p.safety.worst_ms, outputMs: p.output.worst_ms, step: currentStep });
          }
          lastPerf = { late: p.late_frames, frames: p.frames, worst: p.frame.worst_ms };
          r.worstFrameMs = Math.max(r.worstFrameMs, p.frame.worst_ms);
          if (Date.now() >= warmEnd) {
            // The window is 10 s: once it holds only post-warm-up frames, count them.
            if (!perfStart) perfStart = { late: p.late_frames, frames: p.frames };
            else r.worstFrameAfterWarmupMs = Math.max(r.worstFrameAfterWarmupMs, p.frame.worst_ms);
          }
        } catch (e) {
          fail(`/api/perf failed: ${(e as Error).message}`);
        }
      }
      await sleep(250);
    }
  })();

  // ---- memory sample, taken between two operator steps ----
  let refIndex = -1;
  let domRef: Record<string, number> | null = null, domLast: Record<string, number> = {};
  async function sample() {
    await within(cdp.send('HeapProfiler.collectGarbage'), 'garbage collection');
    const heap = await within(cdp.send('Runtime.getHeapUsage'), 'heap usage');
    const dom = await within(cdp.send('Memory.getDOMCounters'), 'DOM counters');
    const s: MemSample = {
      atS: atS(), studioKb: await processKb(pid!), files: await openFiles(pid!), heapKb: Math.round(heap.usedSize / 1024),
      nodes: dom.nodes, listeners: dom.jsEventListeners, worstFrameMs: lastPerf.worst, lateFrames: lastPerf.late, load: Math.round(loadavg()[0] * 10) / 10,
    };
    r.samples.push(s);
    if (refIndex < 0 && Date.now() >= warmEnd) refIndex = r.samples.length - 1;
    // Where the nodes are, by element id (innermost id counts): what grows, if anything does.
    const byId = await within(page.evaluate(() => {
      const out: Record<string, number> = {};
      for (const el of Array.from(document.querySelectorAll('[id]'))) {
        const n = el.getElementsByTagName('*').length;
        if (n >= 20) out[el.id] = n;
      }
      return out;
    }), 'DOM census');
    if (refIndex === r.samples.length - 1) domRef = byId;
    domLast = byId;
    if (opts.verbose) {
      const ref = refIndex >= 0 ? r.samples[refIndex] : null;
      const pct = ref ? ` (${((s.studioKb / ref.studioKb - 1) * 100).toFixed(1)} %)` : ' (échauffement)';
      console.log(`${(s.atS / 60).toFixed(1)} min · studio ${(s.studioKb / 1024).toFixed(1)} Mo${pct} · tas page ${(s.heapKb / 1024).toFixed(1)} Mo · ${s.nodes} nœuds · ${s.listeners} écouteurs · ${s.files} fichiers · pire image ${s.worstFrameMs.toFixed(1)} ms · ${s.lateFrames} en retard`);
    }
  }

  // ---- the operator's steps ----
  const projectFile = (name: string) => path.join(studio.dataDir, 'projects', `${name}.lsproj`);
  const visible = async (sel: string) => page.locator(sel).first().isVisible().catch(() => false);
  /** Close whatever an earlier step may have left open, without Échap. */
  async function tidy() {
    for (const sel of ['#keysClose', '#aboutClose', '#tlExClose', '#projCancel', '#askCancel', '#helpClose', '#newProjCancel', '#whatsNewClose']) {
      if (await visible(sel)) await page.locator(sel).first().click();
    }
    if (await visible('#estopBanner')) {
      resetAt = Date.now();
      await page.locator('#estopReset').click();
    }
  }
  let escapeAt = 0, resetAt = 0;

  const steps: Record<string, (which?: number) => Promise<void>> = {
    async workspace() {
      const ws = rng.pick(WORKSPACES);
      await openWorkspace(page, ws);
      if (ws === 'live') {
        await page.locator(`#liveTabs [data-panel-tab="${rng.pick(LIVE_PANELS)}"]`).click();
      } else if (ws === 'creation') {
        const tab = page.locator(`#creationTabs [data-ctab="${rng.pick(CREATION_TABS)}"]`);
        if (await tab.isVisible()) await tab.click();
      } else if (ws === 'settings') {
        // Open then close a few RÉGLAGES sections.
        const sections = page.locator('[data-ws="settings"] details > summary:visible');
        const n = await sections.count();
        for (let i = 0; i < Math.min(2, n); i++) {
          const s = sections.nth(rng.int(n));
          await s.click();
          await s.click();
        }
      }
    },
    async cues() {
      await openWorkspace(page, 'live');
      const tabs = page.locator('#cueTabs button:visible');
      const n = await tabs.count();
      if (n) await tabs.nth(rng.int(Math.min(n, 12))).click();
      const cues = page.locator('#cues .cue[data-id]:visible');
      const c = await cues.count();
      for (let i = 0; i < 1 + rng.int(3) && c; i++) {
        await cues.nth(rng.int(c)).click();
        await sleep(rng.between(100, 600));
      }
      if (rng.next() < 0.3) {
        await reveal(page, '#runAll');
        await page.locator('#runAll').click();
      }
    },
    async drag() {
      const sel = rng.pick(SLIDERS);
      if (await page.locator(sel).count() === 0) return;
      await reveal(page, sel);
      const box = await page.locator(sel).boundingBox();
      if (!box) return;
      const y = box.y + box.height / 2;
      let x = box.x + box.width * rng.between(0.1, 0.9);
      await page.mouse.move(x, y);
      await page.mouse.down();
      // A drag of 20–60 moves, as a hand gives, ending anywhere.
      for (let i = 0, n = 20 + rng.int(40); i < n; i++) {
        x = Math.min(box.x + box.width - 1, Math.max(box.x + 1, x + rng.between(-0.08, 0.08) * box.width));
        await page.mouse.move(x, y);
        await sleep(8);
      }
      await page.mouse.up();
    },
    async tempo() {
      await openWorkspace(page, 'live');
      if (rng.next() < 0.5) {
        await focusPage(page);
        const gap = rng.between(350, 600);
        for (let i = 0; i < 4; i++) { await page.keyboard.press('Enter'); await sleep(gap); }
      } else {
        const bpm = page.locator('#bpm');
        await bpm.fill(String(Math.round(rng.between(80, 175))));
        await bpm.press('Enter');
      }
    },
    async timeline() {
      await openWorkspace(page, 'timeline');
      const opener = (await visible('#tlEmptyEx')) ? '#tlEmptyEx' : (await visible('#tlExamples')) ? '#tlExamples' : null;
      if (opener && rng.next() < 0.5) {
        await page.locator(opener).click();
        const dlg = page.locator('#tlExDialog');
        await expect(dlg).toBeVisible();
        const ex = dlg.locator('.tl-ex');
        await expect(ex.first()).toBeVisible();
        await ex.nth(rng.int(await ex.count())).click();
        await expect(dlg).toBeHidden();
      }
      if (await page.locator('#tlPlay').isVisible()) {
        await page.locator('#tlPlay').click();
        await sleep(rng.between(500, 2500));
        await page.locator(rng.next() < 0.5 ? '#tlPause' : '#tlStop').click();
      }
    },
    async dialog(forced?: number) {
      // Échap closes the dialog AND latches the e-stop: it must work over any dialog.
      const viaEscape = rng.next() < 0.25;
      const which = forced ?? rng.int(5);
      if (which === 0) {
        await page.locator('#helpBtn').click();
        await page.locator('#helpMenuKeys').click();
        await expect(page.locator('#keysHelp')).toBeVisible();
        if (!viaEscape) await page.locator('#keysClose').click();
      } else if (which === 1) {
        await page.locator('#projBtn').click();
        await page.locator('#projMenu [data-proj="about"]').click();
        await expect(page.locator('#aboutDialog')).toBeVisible();
        if (rng.next() < 0.5) await page.locator('#aboutTabNotes').click();
        if (!viaEscape) await page.locator('#aboutClose').click();
      } else if (which === 2) {
        await focusPage(page);
        await page.keyboard.press('Control+k');
        await expect(page.locator('#searchBox')).toBeVisible();
        await page.keyboard.type(rng.pick(['tunnel', 'cercle', 'vague', 'tempo', 'zone', 'faisceau']), { delay: 30 });
        if (!viaEscape) await page.locator('h1').click(); // a click outside closes it
      } else {
        // The bell or the history: Échap must close them too (T-482).
        const [btn, panel] = which === 3 ? ['#noticeBtn', '#noticePanel'] : ['#historyBtn', '#historyPanel'];
        await page.locator(btn).click();
        await expect(page.locator(panel)).toBeVisible();
        if (!viaEscape) await page.locator(btn).click();
      }
      if (viaEscape) await estop(false);
      await tidy();
    },
    async project() {
      const cur = (await studio.get<{ name: string }>('/api/project')).name;
      const target = cur === 'Soak A' ? 'Soak B' : 'Soak A';
      await page.locator('#projBtn').click();
      if (!existsSync(projectFile(target))) {
        await page.locator('[data-proj="save-as"]').click();
        await page.locator('#projPath').fill(target);
        await page.locator('#projOk').click();
      } else {
        await page.locator('[data-proj="open"]').click();
        // Unsaved changes are asked about first, then the file is chosen.
        const ask = page.locator('#projAsk'), dlg = page.locator('#projDialog');
        await expect(page.locator('#projAsk:visible, #projDialog:visible').first()).toBeVisible();
        if (await ask.isVisible()) {
          // « Enregistrer » refuses (an alert) when the unsaved show is a
          // timeline example: it needs a name first, by design. Then the
          // operator opens again and drops the changes.
          let alerted = false;
          const onDialog = (d: import('@playwright/test').Dialog) => {
            if (d.type() === 'alert') alerted = true;
            if (d.type() !== 'beforeunload') d.dismiss().catch(() => {});
          };
          page.on('dialog', onDialog);
          try {
            await page.locator(rng.next() < 0.5 ? '#askSave' : '#askDiscard').click();
            await expect.poll(async () => alerted || await dlg.isVisible()).toBe(true);
          } finally {
            page.off('dialog', onDialog);
          }
          if (alerted) {
            await page.locator('#projBtn').click();
            await page.locator('[data-proj="open"]').click();
            await expect(ask).toBeVisible();
            await page.locator('#askDiscard').click();
          }
        }
        await expect(dlg).toBeVisible();
        await page.locator('#projPath').fill(target);
        await page.locator('#projOk').click();
      }
      await expect(page.locator('#projDialog')).toBeHidden({ timeout: 15_000 });
      await expect(page.locator('#projName')).toHaveText(new RegExp(`^${target}( •)?$`), { timeout: 15_000 });
    },
    async secondTab() {
      // Another page comes and goes (a tablet, a second screen): the
      // studio's streams and heartbeats must not pile up.
      const other = await page.context().newPage();
      other.on('pageerror', e => r.pageErrors.push(`${atS()} s (2e onglet): ${e.message}`));
      await other.goto(studio.url + '/');
      // It opens on the operator's last workspace (T-426), maybe on an
      // empty frame: loaded = cues built and the project's name in.
      await expect(other.locator('#cues .cue').first()).toBeAttached({ timeout: 15_000 });
      await expect(other.locator('#projName')).toHaveText(/\S/, { timeout: 15_000 });
      await sleep(rng.between(300, 1500));
      await other.close();
    },
    async arm() {
      // Arming in preview, on the fake output only, and only here.
      await tidy();
      const st = await studio.get<{ output: string | null; armed: boolean }>('/api/state');
      if (st.output !== 'Sortie de test') throw new Error(`refusing to arm: output ${st.output}`);
      armWindow = true;
      r.explicitArms++;
      await page.locator('#armBtn').click();
      await expect.poll(async () => (await studio.state()).armed, { timeout: 5_000 }).toBe(true);
      await sleep(rng.between(200, 1500));
      await estop(true);
    },
  };

  /** Échap: always the e-stop, latched; disarmed at once; then reset by the button. */
  async function estop(wasArmed: boolean) {
    const s0 = performance.now();
    escapeAt = Date.now();
    await page.keyboard.press('Escape');
    // Done when the studio has latched the stop, not merely disarmed: a
    // disarm can come from elsewhere (the presence watchdog, on a loaded
    // Mac), and a reset sent before the stop arrives would be undone by it.
    let ms = 0;
    for (;;) {
      const st = await studio.get<{ armed: boolean; estop: unknown }>('/api/arm');
      ms = performance.now() - s0;
      if (!st.armed && st.estop) break;
      if (ms > 2_000) { fail(`e-stop: ${st.armed ? 'still armed' : 'not latched'} after ${ms.toFixed(0)} ms`); break; }
    }
    armWindow = false;
    r.estops++;
    if (wasArmed) r.worstEstopMs = Math.max(r.worstEstopMs, ms);
    await expect(page.locator('#estopBanner')).toBeVisible();
    // What Échap closes is closed; a modal window stays open by design
    // (T-452) and covers the banner: the operator closes it, then resets.
    await expect(page.locator('#keysHelp')).toBeHidden();
    await expect(page.locator('#searchBox')).toBeHidden();
    await expect(page.locator('#noticePanel')).toBeHidden();
    await expect(page.locator('#historyPanel')).toBeHidden();
    await tidy();
    try {
      await expect(page.locator('#estopBanner')).toBeHidden();
    } catch (e) {
      const api = await studio.get('/api/arm');
      const ui = await page.evaluate('JSON.stringify({ estop: arm && arm.estop, armed, seqMin: LINK.armSeqMin, seq: LINK.seq, version: LINK.version, serverVersion: LINK.serverVersion, up: LINK.up, leader: LINK.leader, leaderUp: LINK.leaderUp, push: LINK.push, waitFor: LINK.waitFor })').catch(err => String(err));
      const log = await studio.get<{ events: unknown[] }>('/api/safety/log').then(l => l.events.slice(-8)).catch(err => String(err));
      throw new Error(`banner still shown; Échap at ${escapeAt}, reset clicked at ${resetAt}; /api/arm ${JSON.stringify(api).slice(0, 300)}; page ${String(ui).slice(0, 400)}; log ${JSON.stringify(log).slice(0, 2000)}`);
    }
    if ((await studio.state()).armed) fail('the e-stop reset re-armed');
  }

  const weighted: [string, number][] = [['workspace', 5], ['cues', 5], ['drag', 6], ['tempo', 2], ['timeline', 2], ['dialog', 3], ['project', 1], ['secondTab', 1], ['arm', 1]];
  const total = weighted.reduce((a, [, w]) => a + w, 0);
  const pickStep = () => {
    let x = rng.next() * total;
    for (const [name, w] of weighted) { if ((x -= w) < 0) return name; }
    return weighted[0][0];
  };

  /**
   * The warm-up starts with a tour of everything once: panels and
   * dialogs build their content on first use, which is no leak.
   */
  async function tour() {
    for (const ws of WORKSPACES) await openWorkspace(page, ws);
    await openWorkspace(page, 'live');
    for (const p of LIVE_PANELS) await page.locator(`#liveTabs [data-panel-tab="${p}"]`).click();
    await openWorkspace(page, 'creation');
    for (const c of CREATION_TABS) {
      const tab = page.locator(`#creationTabs [data-ctab="${c}"]`);
      if (await tab.isVisible()) await tab.click();
    }
    await openWorkspace(page, 'settings');
    // The sections shown now (opening one shows the ones inside it).
    const n = await page.locator('[data-ws="settings"] details > summary:visible').evaluateAll(els => { els.forEach((el, i) => el.setAttribute('data-soak', String(i))); return els.length; });
    for (let i = 0; i < n; i++) {
      const sum = page.locator(`[data-soak="${i}"]`);
      if (!(await sum.isVisible())) continue;
      await sum.click();
      await sum.click();
    }
    for (let which = 0; which < 5; which++) await steps.dialog(which);
    for (const name of ['cues', 'drag', 'tempo', 'timeline', 'project', 'project', 'secondTab', 'arm']) {
      currentStep = `tour:${name}`;
      stepLog.push([Date.now(), currentStep]);
      await steps[name]();
      r.steps[name] = (r.steps[name] ?? 0) + 1;
    }
  }

  let nextSample = Date.now();
  let stopped = false;
  try {
    currentStep = 'tour';
    try {
      await tour();
    } catch (e) {
      fail(`tour failed (${currentStep}): ${brief(e)}`);
      stopped = true;
      await page.screenshot({ path: test.info().outputPath('step-failed.png') }).catch(() => {});
    }
    while (Date.now() < end && !stopped) {
      if (Date.now() >= nextSample) {
        try {
          await sample();
        } catch (e) {
          fail(`sample failed: ${brief(e)}`);
          stopped = true;
          break;
        }
        nextSample += opts.sampleEverySec * 1000;
      }
      const name = pickStep();
      currentStep = name;
      stepLog.push([Date.now(), name]);
      const s0 = Date.now();
      try {
        await steps[name]();
      } catch (e) {
        // The run stops there, with its report: what hung, and when.
        fail(`step « ${name} » failed: ${brief(e)}`);
        r.steps[name] = (r.steps[name] ?? 0) + 1;
        await page.screenshot({ path: test.info().outputPath('step-failed.png') }).catch(() => {});
        stopped = true;
        break;
      }
      const ms = Date.now() - s0;
      r.steps[name] = (r.steps[name] ?? 0) + 1;
      if (ms > r.slowestStep.ms) r.slowestStep = { name, ms };
      await sleep(rng.between(50, 400));
    }
    await tidy().catch(e => fail(`closing up: ${brief(e)}`));
    await sample().catch(e => fail(`last sample failed: ${brief(e)}`));
  } finally {
    watching = false;
    await watcher;
  }

  // ---- end of run ----
  r.durationS = Math.round((Date.now() - t0) / 1000);
  // Floors: the lowest value of the first third after the warm-up, and
  // of the last third. One sample swings with what is loaded (a show, the
  // search index, a figure list); a leak lifts the floor.
  r.refIndex = refIndex;
  if (domRef) {
    const ref = domRef;
    r.domGrowth = Object.entries(domLast).map(([id, to]) => ({ id, from: ref[id] ?? 0, to }))
      .filter(g => g.to - g.from >= 20).sort((a, b) => (b.to - b.from) - (a.to - a.from)).slice(0, 8);
  }
  const after = refIndex >= 0 ? r.samples.slice(refIndex) : [];
  if (after.length >= 2) {
    const k = Math.max(1, Math.floor(after.length / 3));
    r.reference = floorSample(after.slice(0, k));
    r.last = floorSample(after.slice(-k));
  }
  if (perfStart) {
    r.lateFramesAfterWarmup = lastPerf.late - perfStart.late;
    r.framesAfterWarmup = lastPerf.frames - perfStart.frames;
  }
  const [longest, count, list] = await page.evaluate(() => {
    const w = window as unknown as { __soakLongest: number; __soakLong: number; __soakLongList: [number, number][] };
    return [w.__soakLongest, w.__soakLong, w.__soakLongList] as const;
  });
  r.longestTaskMs = longest;
  r.longTasks = count;
  r.longTaskEvents = list.map(([at, ms]) => ({ atS: Math.round((at - t0) / 100) / 10, ms: Math.round(ms), step: [...stepLog].reverse().find(([t]) => t <= at)?.[1] ?? 'start' }));
  const log = readOutputLog(studio.outputLog);
  r.outputArms = log.arms;
  r.litWhileDisarmed = log.litWhileDisarmed;
  if (log.arms > r.explicitArms) fail(`the output was armed ${log.arms} times for ${r.explicitArms} explicit arms`);
  if (log.litWhileDisarmed) fail(`the output went lit ${log.litWhileDisarmed} times while disarmed`);
  if ((await studio.state()).armed) fail('armed at the end of the run');
  if (r.longestTaskMs > opts.longTaskLimitMs) fail(`the page froze for ${r.longestTaskMs.toFixed(0)} ms`);
  if (r.worstPageMs > opts.longTaskLimitMs) fail(`the page took ${r.worstPageMs.toFixed(0)} ms to answer`);
  if (opts.frameLimitMs !== null && r.worstFrameAfterWarmupMs > opts.frameLimitMs) fail(`engine frame of ${r.worstFrameAfterWarmupMs.toFixed(1)} ms after the warm-up (limit ${opts.frameLimitMs} ms)`);
  const ref = r.reference, last = r.last;
  if (ref && last) {
    if (last.studioKb > ref.studioKb * (1 + opts.studioMemTolerance) + opts.studioMemSlackKb) fail(`studio memory ${ref.studioKb} → ${last.studioKb} kB (> +${opts.studioMemTolerance * 100} % + ${opts.studioMemSlackKb} kB)`);
    // Even its floor moves with what the page holds (a show, the search
    // index, figure lists): 4 MB of slack on the page heap.
    if (last.heapKb > ref.heapKb * (1 + opts.pageHeapTolerance) + 4096) fail(`page heap ${ref.heapKb} → ${last.heapKb} kB`);
    if (last.listeners > ref.listeners * 1.1 + 50) fail(`page listeners ${ref.listeners} → ${last.listeners}`);
    if (last.nodes > ref.nodes * 1.1 + 500) fail(`page DOM nodes ${ref.nodes} → ${last.nodes}`);
    if (ref.files >= 0 && last.files > ref.files + 20) fail(`studio open files ${ref.files} → ${last.files}`);
  } else {
    fail('no memory sample after the warm-up');
  }
  if (r.pageErrors.length) fail(`${r.pageErrors.length} page error(s): ${r.pageErrors.slice(0, 3).join(' | ')}`);
  return r;
}

function floorSample(xs: MemSample[]): MemSample {
  // -1: the tool did not answer that time.
  const min = (f: (s: MemSample) => number) => Math.min(...xs.map(f).filter(v => v >= 0), Infinity);
  return {
    atS: xs[xs.length - 1].atS, studioKb: min(s => s.studioKb), files: min(s => s.files), heapKb: min(s => s.heapKb),
    nodes: min(s => s.nodes), listeners: min(s => s.listeners), worstFrameMs: Math.max(...xs.map(s => s.worstFrameMs)), lateFrames: xs[xs.length - 1].lateFrames, load: Math.max(...xs.map(s => s.load)),
  };
}

const mb = (kb: number) => (kb / 1024).toFixed(1).replace('.', ',');
const pct = (a: number, b: number) => `${b >= a ? '+' : ''}${((b / a - 1) * 100).toFixed(1).replace('.', ',')} %`;

/** The report, in French (docs/qa/soak-<date>.md). */
export function soakReport(r: SoakResult, date: string): string {
  const ref = r.reference, last = r.last;
  const ok = r.failures.length === 0;
  const lines = [
    `# Endurance de l'interface (soak Playwright) — ${date}`,
    '',
    `Tâche T-482 (reste de T-449). Aperçu uniquement : studio de test (sortie factice, port libre, dossier temporaire), jamais \`--device\`.`,
    '',
    `- **Verdict : ${ok ? 'VERT' : 'ROUGE'}**`,
    `- Durée : ${(r.durationS / 60).toFixed(1).replace('.', ',')} min (échauffement ${r.opts.warmupMinutes} min), graine \`SOAK_SEED=${r.opts.seed}\``,
    `- Binaire : ${r.release ? 'release' : 'debug'} ; charge du Mac (moyenne 1 min) : ${r.samples.length ? `${Math.min(...r.samples.map(s => s.load))} à ${Math.max(...r.samples.map(s => s.load))}`.replace(/\./g, ',') : '?'} (d'autres travaux tournaient en même temps)`,
    ...(r.sleptS ? [`- **Tout s'est arrêté ${r.sleptS} s pendant la course (Mac en veille) : mesures non valables.** \`npm run soak\` lance \`caffeinate -dims\` ; un portable laissé seul peut s'endormir quand même.`] : []),
    `- Étapes de l'opérateur : ${Object.entries(r.steps).map(([k, v]) => `${k} ×${v}`).join(', ')} ; la plus lente : ${r.slowestStep.name} (${r.slowestStep.ms} ms)`,
    '',
    '## Critères',
    '',
    '| Critère | Mesure | Seuil |',
    '|---|---|---|',
    `| Erreurs de la page | ${r.pageErrors.length} | 0 |`,
    `| Armé hors fenêtre d'armement explicite | ${r.armedOutsideWindow} | 0 |`,
    `| Armements de la sortie / demandés | ${r.outputArms} / ${r.explicitArms} | ≤ demandés |`,
    `| Allumée en étant désarmée (journal de la sortie) | ${r.litWhileDisarmed} | 0 |`,
    `| Arrêts d'urgence (Échap) / pire délai armé → désarmé | ${r.estops} / ${r.worstEstopMs.toFixed(1).replace('.', ',')} ms | toujours |`,
    `| Pire image moteur après échauffement | ${r.worstFrameAfterWarmupMs.toFixed(1).replace('.', ',')} ms (toute la course : ${r.worstFrameMs.toFixed(1).replace('.', ',')} ms) | ${r.opts.frameLimitMs === null ? 'relevé seulement (debug)' : `< ${r.opts.frameLimitMs} ms`} |`,
    `| Images en retard (> 16,7 ms) après échauffement | ${r.lateFramesAfterWarmup} sur ${r.framesAfterWarmup} | relevé |`,
    `| Pire réponse de \`/api/state\` (${r.stateRequests} requêtes) | ${r.worstStateMs.toFixed(0)} ms | < ${r.opts.stateLimitMs} ms |`,
    `| Plus longue tâche de la page (tâches > 50 ms) | ${r.longestTaskMs.toFixed(0)} ms (${r.longTasks}) | < ${r.opts.longTaskLimitMs} ms |`,
    `| Pire réponse de la page (une image d'animation) | ${r.worstPageMs.toFixed(0)} ms | < ${r.opts.longTaskLimitMs} ms |`,
    ...(ref && last ? [
      `| Mémoire du studio (footprint) | ${mb(ref.studioKb)} → ${mb(last.studioKb)} Mo (${pct(ref.studioKb, last.studioKb)}) | ≤ +${r.opts.studioMemTolerance * 100} % + ${mb(r.opts.studioMemSlackKb)} Mo |`,
      `| Tas JS de la page après GC | ${mb(ref.heapKb)} → ${mb(last.heapKb)} Mo (${pct(ref.heapKb, last.heapKb)}) | ≤ +${r.opts.pageHeapTolerance * 100} % + 4 Mo |`,
      `| Nœuds DOM / écouteurs | ${ref.nodes} → ${last.nodes} / ${ref.listeners} → ${last.listeners} | ≤ +10 % |`,
      `| Fichiers ouverts du studio | ${ref.files} → ${last.files} | ≤ +20 |`,
    ] : ['| Mémoire | pas de mesure après l\'échauffement | |']),
    '',
    '## Images en retard',
    '',
    ...(r.lateEvents.length ? [
      'Chaque hausse du compteur, avec les pires durées par étape de la fenêtre de 10 s et l\'étape de l\'opérateur en cours (ou juste finie).',
      '',
      '| s | nouvelles | image (ms) | rendu | sécurité | sortie | étape |',
      '|---|---|---|---|---|---|---|',
      ...r.lateEvents.map(e => `| ${e.atS} | ${e.late} | ${e.frameMs.toFixed(1)} | ${e.renderMs.toFixed(1)} | ${e.safetyMs.toFixed(1)} | ${e.outputMs.toFixed(1)} | ${e.step} |`),
    ] : ['Aucune.']),
    '',
    '## Tâches longues de la page (> 200 ms)',
    '',
    ...(r.longTaskEvents.length ? [
      '| s | durée (ms) | étape |',
      '|---|---|---|',
      ...r.longTaskEvents.map(e => `| ${e.atS} | ${e.ms} | ${e.step} |`),
    ] : ['Aucune.']),
    '',
    '## Nœuds DOM : plus fortes hausses par élément',
    '',
    ...(r.domGrowth.length ? [
      'Descendants de chaque élément à identifiant, de la première mesure après l\'échauffement à la dernière (un parent compte aussi ce que gagnent ses enfants).',
      '',
      '| élément | avant | après |',
      '|---|---|---|',
      ...r.domGrowth.map(g => `| \`#${g.id}\` | ${g.from} | ${g.to} |`),
    ] : ['Aucune hausse de 20 nœuds ou plus.']),
    '',
    '## Courbe',
    '',
    'Mémoire comparée par planchers : la plus basse valeur du premier tiers des mesures après l\'échauffement, puis du dernier tiers (une mesure isolée varie avec ce qui est chargé). Le tas de la page est mesuré après un ramasse-miettes forcé (CDP).',
    '',
    '| min | studio (Mo) | tas page (Mo) | nœuds | écouteurs | fichiers | pire image (ms) | en retard | charge |',
    '|---|---|---|---|---|---|---|---|---|',
    ...r.samples.map((s, i) => `| ${(s.atS / 60).toFixed(1).replace('.', ',')}${i === r.refIndex ? ' (début réf.)' : ''} | ${mb(s.studioKb)} | ${mb(s.heapKb)} | ${s.nodes} | ${s.listeners} | ${s.files} | ${s.worstFrameMs.toFixed(1).replace('.', ',')} | ${s.lateFrames} | ${String(s.load).replace('.', ',')} |`),
    '',
    '## Échecs',
    '',
    ...(ok ? ['Aucun.'] : r.failures.map(f => `- ${f}`)),
    '',
    `Rejouer : \`SOAK_MINUTES=${r.opts.minutes} SOAK_SEED=${r.opts.seed} npm --prefix studio/e2e run soak\``,
    '',
  ];
  return lines.join('\n');
}
