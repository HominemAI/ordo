import { defineConfig, devices } from '@playwright/test';

// The throwaway identity service (stack.sh) is Playwright's web server.
// Chromium only: the virtual passkey device (Chrome DevTools' WebAuthn
// domain) is Chromium's. The flows share one deployment's people, so they
// run in order in one worker.
//
// Run: `cargo build -p ordo-identity-service`, `npm install` here (the
// console e2e's node_modules, linked, works too), then `npx playwright test`.
// Set ORDO_PG_HOST (and _PORT, _USER) to use a running cluster; without it
// stack.sh starts and destroys its own.
// CI: .github/workflows/identity-e2e.yml.
const origin = `http://localhost:${process.env['E2E_IDENTITY_PORT'] ?? '4690'}`;
const ci = !!process.env['CI'];

export default defineConfig({
  testDir: 'tests',
  fullyParallel: false,
  workers: 1,
  forbidOnly: ci,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: [['list']],
  outputDir: 'test-results',
  use: {
    baseURL: origin,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'browser', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'bash stack.sh',
    url: `${origin}/health/ready`,
    reuseExistingServer: false,
    timeout: 120_000,
    stdout: 'pipe',
    stderr: 'pipe',
    gracefulShutdown: { signal: 'SIGTERM', timeout: 10_000 },
  },
});
