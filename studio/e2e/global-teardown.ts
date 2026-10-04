import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { RUN_PIDS_ENV } from './studio';

// T-358: stop every studio of this run still alive (a spec whose afterAll
// never ran: a timeout, a crashed worker), and delete its data dir. Only
// PIDs this run recorded, and only if that PID is still a test studio
// (--exit-with-parent): never the user's instance nor another run's.

function command(pid: number): string | null {
  try {
    return execFileSync('ps', ['-p', String(pid), '-o', 'command='], { encoding: 'utf8' }).trim() || null;
  } catch {
    return null; // gone
  }
}

function isTestStudio(cmd: string | null): cmd is string {
  return !!cmd && cmd.includes('laser-studio') && cmd.includes('--exit-with-parent') && !cmd.includes('--device');
}

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

export default async function globalTeardown() {
  const file = process.env[RUN_PIDS_ENV];
  if (!file || !existsSync(file)) return;
  const pids = [...new Set(readFileSync(file, 'utf8').split('\n').map(Number).filter(p => p > 1))];
  const left = pids.map(pid => ({ pid, cmd: command(pid) })).filter((s): s is { pid: number; cmd: string } => isTestStudio(s.cmd));
  for (const { pid } of left) {
    try { process.kill(pid, 'SIGINT'); } catch { /* already gone */ } // the clean shutdown
  }
  const deadline = Date.now() + 5_000;
  while (left.some(s => isTestStudio(command(s.pid))) && Date.now() < deadline) await sleep(100);
  for (const { pid } of left) {
    if (isTestStudio(command(pid))) {
      try { process.kill(pid, 'SIGKILL'); } catch { /* already gone */ }
    }
  }
  for (const { pid, cmd } of left) {
    console.warn(`global teardown: stopped a leftover test studio (PID ${pid})`);
    // Its temporary data dir, only if it is one of ours.
    const dir = /--data-dir (\S+)/.exec(cmd)?.[1];
    if (dir && path.basename(dir).startsWith('laser-studio-e2e-') && path.dirname(dir) === path.resolve(tmpdir())) {
      rmSync(dir, { recursive: true, force: true });
      rmSync(dir + '.output.log', { force: true });
    }
  }
  rmSync(path.dirname(file), { recursive: true, force: true });
}
