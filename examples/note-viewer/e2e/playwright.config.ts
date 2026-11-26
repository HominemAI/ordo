import { defineConfig, devices } from '@playwright/test';

// The two-deployment stack (stack.sh) is Playwright's web server: the
// identity service, notes v2 and note_viewer, each with its own database
// role and schema. note_viewer reads notes v2 through notes v2's door.
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
    // note_viewer's consumer key is registered at the identity service, and
    // its apply through its door records its use at notes v2: what a
    // deployment owner does once (plan-federation-grants.md §7). Every
    // project after it reads through notes v2's door.
    {
      name: 'door',
      testMatch: /door\/.*\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: { args: [`--ignore-certificate-errors-spki-list=${FIXTURE_LEAF_SPKI}`] },
      },
    },
    {
      name: 'api',
      testMatch: /api\/.*\.spec\.ts/,
      dependencies: ['door'],
    },
    {
      name: 'browser',
      testMatch: /browser\/.*\.spec\.ts/,
      dependencies: ['door'],
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: { args: [`--ignore-certificate-errors-spki-list=${FIXTURE_LEAF_SPKI}`] },
      },
    },
    // Notes v3 applied over the running notes v2 (plan-versions.md, increment
    // 7a). It changes notes' program for the whole stack, so it runs after.
    {
      name: 'v3',
      testMatch: /v3\/.*\.spec\.ts/,
      dependencies: ['api', 'browser'],
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: { args: [`--ignore-certificate-errors-spki-list=${FIXTURE_LEAF_SPKI}`] },
      },
    },
    // notes renames a field the viewer reads (plan-versions.md, increment
    // 7b). It changes notes' program again, so it runs after v3.
    {
      name: 'rename',
      testMatch: /rename\/.*\.spec\.ts/,
      dependencies: ['v3'],
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: { args: [`--ignore-certificate-errors-spki-list=${FIXTURE_LEAF_SPKI}`] },
      },
    },
    // notes stops declaring a field the viewer reads (increment 14). Its
    // fixture keeps the rename, so it runs after it.
    {
      name: 'orphans',
      testMatch: /orphans\/.*\.spec\.ts/,
      dependencies: ['rename'],
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: { args: [`--ignore-certificate-errors-spki-list=${FIXTURE_LEAF_SPKI}`] },
      },
    },
    // notes seals a field the viewer reads; the viewer shows it, opened by
    // notes' door (plan-federation-grants.md §6).
    {
      name: 'seal',
      testMatch: /seal\/.*\.spec\.ts/,
      dependencies: ['orphans'],
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: { args: [`--ignore-certificate-errors-spki-list=${FIXTURE_LEAF_SPKI}`] },
      },
    },
    // notes takes an op the viewer calls off its grant. The viewer's page
    // names the refusal. It narrows the grant for the whole stack, so it
    // runs last.
    {
      name: 'narrowed',
      testMatch: /narrowed\/.*\.spec\.ts/,
      dependencies: ['seal'],
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
