import { defineConfig, devices } from '@playwright/test';

// The hidden suite. The runner starts the stack first (stacks/<stack>/up.sh)
// and points the suite at it:
//
//   BENCH_STACK       ordo or supabase: which adapter (adapters/index.ts)
//   BENCH_STACK_JSON  the stack.json up.sh wrote: the page, backend and mail
//   BENCH_REPORT_DIR  where results.json and the failure traces go
//
// One worker, in file order: every test shares one running stack, and the
// API probes share their people (tests/api.spec.ts).
const report = process.env['BENCH_REPORT_DIR'] ?? 'report';

export default defineConfig({
  testDir: 'tests',
  fullyParallel: false,
  workers: 1,
  // No retries: a test that passes on a second try is a finding, not a pass.
  retries: 0,
  // A new person's team can take a while on a cold stack.
  timeout: 120_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['json', { outputFile: `${report}/results.json` }]],
  outputDir: `${report}/artifacts`,
  use: {
    ...devices['Desktop Chrome'],
    // A missing label or button fails in 15 s, not at the test's timeout.
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
});
