import { defineConfig, devices } from '@playwright/test';

// T-482: the long soak (`npm run soak`, soak/) is a project of its own,
// only defined when LASER_STUDIO_SOAK=1, so `npm test` never runs it. Its
// 2-minute version, tests/soak.spec.ts, is part of the normal suite.
const soak = process.env.LASER_STUDIO_SOAK === '1';

// Every spec file starts its own preview-only studio (see studio.ts), so
// files can run in parallel; tests inside a file share that instance and
// run in order.
export default defineConfig({
  testDir: './tests',
  globalSetup: './global-setup.ts',
  globalTeardown: './global-teardown.ts',
  fullyParallel: false,
  workers: process.env.CI ? 2 : 4,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 5_000 },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
  outputDir: 'test-results',
  use: {
    ...devices['Desktop Chrome'],
    headless: true,
    viewport: { width: 1400, height: 1000 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: soak
    ? [{ name: 'soak', testDir: './soak', timeout: 0, use: { browserName: 'chromium', trace: 'off' } }]
    : [{ name: 'chromium', use: { browserName: 'chromium' } }],
});
