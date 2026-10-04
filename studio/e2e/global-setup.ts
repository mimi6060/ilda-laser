import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { REPO_ROOT, RUN_PIDS_ENV, STUDIO_BIN } from './studio';

// Build the studio once before any test runs, so every spec file tests the
// current code. Set LASER_STUDIO_SKIP_BUILD=1 to reuse an existing binary.
export default function globalSetup() {
  if (process.env.LASER_STUDIO_SKIP_BUILD !== '1') {
    const rustup = '/opt/homebrew/opt/rustup/bin';
    const PATH = existsSync(rustup) ? `${rustup}:${process.env.PATH}` : process.env.PATH;
    execFileSync('cargo', ['build', '-p', 'laser-studio'], { cwd: REPO_ROOT, stdio: 'inherit', env: { ...process.env, PATH } });
  }
  if (!existsSync(STUDIO_BIN)) throw new Error(`studio binary not found: ${STUDIO_BIN}`);
  // T-358: this run's studios write their PIDs here (the workers inherit
  // the variable); global-teardown.ts stops any still alive.
  const runDir = mkdtempSync(path.join(tmpdir(), 'laser-studio-e2e-run-'));
  const pids = path.join(runDir, 'pids');
  writeFileSync(pids, '');
  process.env[RUN_PIDS_ENV] = pids;
}
