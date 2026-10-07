// The long interface soak (T-482): `npm --prefix studio/e2e run soak`.
// A release build (global-setup.ts), SOAK_MINUTES long (default 120),
// SOAK_SEED (default 482) for the operator's choices. The report goes to
// SOAK_REPORT_DIR, else docs/qa/ when the checkout has a docs/ folder
// (private notes, never in git), else the test's output folder.
// Preview only (studio.ts).
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, REPO_ROOT } from '../studio';
import { runSoak, soakReport } from './operator';

const studio = useStudio();
const num = (v: string | undefined, d: number) => (v !== undefined && v !== '' && Number.isFinite(+v) ? +v : d);

test('the interface holds for SOAK_MINUTES of scripted operation', async ({ page }) => {
  const minutes = num(process.env.SOAK_MINUTES, 120);
  const opts = {
    minutes,
    warmupMinutes: num(process.env.SOAK_WARMUP_MINUTES, Math.min(5, minutes / 4)),
    sampleEverySec: num(process.env.SOAK_SAMPLE_SECONDS, 60),
    seed: num(process.env.SOAK_SEED, 482),
    studioMemTolerance: 0.05,
    studioMemSlackKb: 1024,
    pageHeapTolerance: 0.25,
    frameLimitMs: 50,
    stateLimitMs: 1_000,
    longTaskLimitMs: 1_000,
    verbose: true,
  };
  console.log(`soak: ${minutes} min, seed ${opts.seed}, ${studio.url}`);
  const r = await runSoak(page, studio, opts);
  const date = new Date().toISOString().slice(0, 10);
  const dir = process.env.SOAK_REPORT_DIR
    ?? (existsSync(path.join(REPO_ROOT, 'docs')) ? path.join(REPO_ROOT, 'docs', 'qa') : test.info().outputDir);
  mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `soak-${date}.md`);
  writeFileSync(file, soakReport(r, date));
  console.log(`soak report: ${file}`);
  expect(r.failures, r.failures.join('\n')).toEqual([]);
});
