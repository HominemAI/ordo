import { defineConfig, devices } from '@playwright/test';

// The two-deployment stack (stack.sh) is Playwright's web server: the
// identity service, notes v2 and note_viewer, on one Postgres with two roles.
// Chromium only: the virtual passkey device (Chrome DevTools' WebAuthn
// domain) is Chromium's. The flows share the stack's people, so they run in
// order in one worker.
//
// The identity service is reached over https with the test fixture's
// `localhost` certificate (tls-front.mjs). Chromium refuses a passkey on a
// page with a certificate error, so the browser is told to trust that one
// key, by its SPKI hash, and nothing else.
//
// Run: `cargo build -p ordo-cli -p ordo-host -p ordo-identity-service`,
// `npm ci` here, in examples/notes-v2/web and in examples/note-viewer/web,
// then `npx playwright test`. Set ORDO_PG_HOST (and _PORT, _USER,
// _PASSWORD) to use a running cluster; without it stack.sh starts and
// destroys its own. CI: .github/workflows/federation-e2e.yml.
const viewerPage = `http://localhost:${process.env['E2E_VIEWER_PAGE_PORT'] ?? '5291'}`;
const ci = !!process.env['CI'];

/** `openssl x509 -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | base64` of the fixture leaf. */
const FIXTURE_LEAF_SPKI = 'dmjOrASrxqt8nLqNAaxP7LuJUpdPzc/F+u3D3xlxBMs=';

export default defineConfig({
  testDir: 'tests',
  fullyParallel: false,
  workers: 1,
  forbidOnly: ci,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  reporter: [['list']],
  outputDir: 'test-results',
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'api', testMatch: /api\/.*\.spec\.ts/ },
    {
      name: 'browser',
      testMatch: /browser\/.*\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: { args: [`--ignore-certificate-errors-spki-list=${FIXTURE_LEAF_SPKI}`] },
      },
    },
  ],
  webServer: {
    command: 'bash stack.sh',
    url: viewerPage,
    reuseExistingServer: false,
    timeout: 300_000,
    stdout: 'pipe',
    stderr: 'pipe',
    gracefulShutdown: { signal: 'SIGTERM', timeout: 15_000 },
  },
});
