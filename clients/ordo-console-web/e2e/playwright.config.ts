import { defineConfig, devices } from '@playwright/test';

// The throwaway console stack (stack.sh) is Playwright's web server. Locally a
// stack already running on the same port is reused.
//
// Two projects, both Chromium: the virtual passkey device (Chrome DevTools'
// WebAuthn domain) is Chromium only, and every console screen needs a passkey.
// - `browser`: the sign-in flows on a desktop window.
// - `runtime`: the supervisor's screens on a desktop window, after `browser`
//   (it comes in with `ordo console recover`). The stack's deployment is the
//   fake host, so a change, a refusal and a crash take seconds.
// - `phone`: the same flows as a Pixel 7. Each step checks the page does not
//   scroll sideways.
//
// The console holds one deployment and at most two owners, so the flows run
// in order in one worker. `runtime` and `phone` run after `browser`: they need
// the owner the first-run flow made, and come in with `ordo console recover`.
const consoleUrl = `http://localhost:${process.env['E2E_CONSOLE_PORT'] ?? '4680'}`;
const ci = !!process.env['CI'];
// The console on the identity service (stack-identity.sh).
const identityConsoleUrl = `http://localhost:${process.env['E2E_ID_CONSOLE_PORT'] ?? '4683'}`;

export default defineConfig({
  testDir: 'tests',
  fullyParallel: false,
  workers: 1,
  forbidOnly: ci,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: [
    ['list'],
    ['html', { outputFolder: 'report/html', open: 'never' }],
    ['json', { outputFile: 'report/results.json' }],
  ],
  outputDir: 'test-results',
  use: {
    baseURL: consoleUrl,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    { name: 'browser', testMatch: /console\.spec\.ts/, use: { ...devices['Desktop Chrome'] } },
    { name: 'runtime', testMatch: /runtime\.spec\.ts/, dependencies: ['browser'], use: { ...devices['Desktop Chrome'] } },
    { name: 'phone', testMatch: /phone\.spec\.ts/, dependencies: ['runtime'], use: { ...devices['Pixel 7'] } },
    // `door`: the link to the deployment (sign-in, the relayed touch, rotation,
    // unlink), against the stack's door, after `phone`.
    { name: 'door', testMatch: /door\.spec\.ts/, dependencies: ['phone'], use: { ...devices['Desktop Chrome'] } },
    // `tenants`: the Tenants screen (step 5), after `door`. It links the
    // stack's door afresh with its own passkey.
    { name: 'tenants', testMatch: /tenants\.spec\.ts/, dependencies: ['door'], use: { ...devices['Desktop Chrome'] } },
    // `settings`: the Sign-in and Every setting screens (step 6), after
    // `tenants`, whose link to the stack's door it reads through.
    { name: 'settings', testMatch: /settings\.spec\.ts/, dependencies: ['tenants'], use: { ...devices['Desktop Chrome'] } },
    // `usage`: the Usage screen (step 7), after `settings`, so two specs
    // never hold the stack's one door link at once. It links the
    // stack's door afresh with its own passkey.
    { name: 'usage', testMatch: /usage\.spec\.ts/, dependencies: ['settings'], use: { ...devices['Desktop Chrome'] } },
    // `people`: the People screen (step 8), after `usage`, so two specs never
    // hold the stack's one door link at once. It links the door afresh.
    { name: 'people', testMatch: /people\.spec\.ts/, dependencies: ['usage'], use: { ...devices['Desktop Chrome'] } },
    // `shares`: the Shares and security screen (step 9), after `people`, so two specs
    // never hold the one door link at once. It links the stack's door afresh with its own passkey.
    { name: 'shares', testMatch: /shares\.spec\.ts/, dependencies: ['people'], use: { ...devices['Desktop Chrome'] } },
    // `identity`: the console on the identity service (identity plan, step
    // 6), against its own stack (stack-identity.sh, the second web server).
    { name: 'identity', testMatch: /identity\.spec\.ts/, use: { ...devices['Desktop Chrome'] } },
  ],
  webServer: [
    {
      command: 'bash stack.sh',
      url: `${consoleUrl}/health/ready`,
      reuseExistingServer: !ci,
      timeout: 180_000,
      stdout: 'pipe',
      stderr: 'pipe',
      gracefulShutdown: { signal: 'SIGTERM', timeout: 10_000 },
    },
    {
      command: 'bash stack-identity.sh',
      url: `${identityConsoleUrl}/health/ready`,
      reuseExistingServer: !ci,
      timeout: 180_000,
      stdout: 'pipe',
      stderr: 'pipe',
      gracefulShutdown: { signal: 'SIGTERM', timeout: 10_000 },
    },
  ],
});
