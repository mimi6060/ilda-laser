import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { REPO_ROOT, STUDIO_BIN } from './studio';

// Build the studio once before any test runs, so every spec file tests the
// current code. Set LASER_STUDIO_SKIP_BUILD=1 to reuse an existing binary.
export default function globalSetup() {
  if (process.env.LASER_STUDIO_SKIP_BUILD !== '1') {
    const rustup = '/opt/homebrew/opt/rustup/bin';
    const PATH = existsSync(rustup) ? `${rustup}:${process.env.PATH}` : process.env.PATH;
    execFileSync('cargo', ['build', '-p', 'laser-studio'], { cwd: REPO_ROOT, stdio: 'inherit', env: { ...process.env, PATH } });
  }
  if (!existsSync(STUDIO_BIN)) throw new Error(`studio binary not found: ${STUDIO_BIN}`);
}
