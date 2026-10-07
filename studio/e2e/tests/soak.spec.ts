// The 2-minute interface soak (T-482): the long one (`npm run soak`,
// soak/soak.spec.ts) in short, so the scenario itself keeps working.
// Same checks; the memory bounds are looser, and in this debug build the
// engine's frames are only bounded against a stall (250 ms): the 50 ms
// criterion is the release soak's. Preview only (studio.ts).
import { writeFileSync } from 'node:fs';
import { test, expect, useStudio } from '../studio';
import { runSoak, soakReport } from '../soak/operator';

const studio = useStudio();

test('two minutes of scripted operation: no error, no leak, never armed but on request, e-stop always', async ({ page }) => {
  test.setTimeout(5 * 60_000);
  const r = await runSoak(page, studio, {
    minutes: 2,
    warmupMinutes: 0.5,
    sampleEverySec: 15,
    seed: +(process.env.SOAK_SEED ?? 482),
    studioMemTolerance: 0.10,
    studioMemSlackKb: 4096,
    pageHeapTolerance: 0.5,
    frameLimitMs: 250,
    stateLimitMs: 2_000,
    longTaskLimitMs: 2_000,
    verbose: false,
  });
  const report = soakReport(r, new Date().toISOString().slice(0, 10));
  writeFileSync(test.info().outputPath('soak.md'), report);
  await test.info().attach('soak.md', { body: report, contentType: 'text/markdown' });
  expect(r.failures, report).toEqual([]);
  expect(r.explicitArms, 'the soak arms (in preview) at least once').toBeGreaterThan(0);
  for (const step of ['workspace', 'cues', 'drag', 'dialog']) expect(r.steps[step] ?? 0, step).toBeGreaterThan(0);
});
