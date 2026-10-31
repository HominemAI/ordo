import { defineConfig, devices } from '@playwright/test';

// The throwaway stack (stack.sh) is Playwright's web server: it starts before
// the first test and is stopped after the last. Locally, a stack already
// running on the same ports is reused, so `bash stack.sh` in one terminal and
// `npx playwright test` in another works for a quick loop.
//
// Six projects:
// - `api`: the door at request level, no browser. Ways in, CORS, the mail
//   catcher, the stand-in provider, writes through /op.
// - `browser`: the page in Chromium, with a virtual passkey (CDP WebAuthn).
//   Runs tests/browser (passkey, other ways in) and tests/ui (notes flows).
// - `phone`: tests/phone in Chromium as a Pixel 7.
// - `firefox`, `webkit`: tests/ui only. No virtual passkey outside Chromium,
//   so people come in by emailed code (tests/people.ts).
//
// - `upgrade`: v2 over a live v1 (tests/upgrade). Last and alone, Chromium only.
//
// E2E_TARGET picks the target. `throwaway` (the default) is the stack above.
// `staging` is phase 2 and refused for now (tests/support.ts).
const page = `http://localhost:${process.env['E2E_PAGE_PORT'] ?? '5190'}`;
const ci = !!process.env['CI'];
// The screenshot projects run in their own CI step, with their own report and
// results, so they cannot overwrite or hide the main run's (docs/howto-e2e.md).
const reportDir = process.env['E2E_REPORT'] ?? 'report';
const outputDir = process.env['E2E_OUTPUT'] ?? 'test-results';

export default defineConfig({
  testDir: 'tests',
  // Each flow signs up its own people, so flows can run side by side.
  fullyParallel: true,
  workers: ci ? 3 : 4,
  forbidOnly: ci,
  // No retries: a flow that passes on the second try is a finding, not a pass.
  retries: 0,
  // A flow signs up one to three people, and each new person's tenant may take
  // up to the SDK's two-minute wait (tests/support.ts, SETTING_UP_MS). A sign-up
  // is quick when the door is not loaded; the bound is for when it is.
  timeout: 180_000,
  // Pictures for toHaveScreenshot. Steady by construction: no animation, no
  // caret, a small tolerance for the odd anti-aliased pixel. The pictures are
  // made in the CI runner only; locally the specs skip (tests/screenshots).
  expect: {
    timeout: 15_000,
    toHaveScreenshot: { animations: 'disabled', caret: 'hide', scale: 'css', maxDiffPixelRatio: 0.01 },
  },
  snapshotDir: 'screenshots',
  // One folder per project, with no platform in the name: one environment makes them.
  snapshotPathTemplate: '{snapshotDir}/{projectName}/{arg}{ext}',
  reporter: [
    ['list'],
    ['html', { outputFolder: `${reportDir}/html`, open: 'never' }],
    ['json', { outputFile: `${reportDir}/results.json` }],
  ],
  outputDir,
  use: {
    baseURL: page,
    // The stand-in provider's certificate is signed by a test CA.
    ignoreHTTPSErrors: true,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    { name: 'api', testMatch: /api\/.*\.spec\.ts/ },
    // Chromium: the passkey flows (browser/) and the notes flows (ui/).
    { name: 'browser', testMatch: /(browser|ui)\/.*\.spec\.ts/, use: { ...devices['Desktop Chrome'] } },
    // A Pixel 7 in Chromium: a narrow touch screen, and the virtual passkey still works.
    { name: 'phone', testMatch: /phone\/.*\.spec\.ts/, use: { ...devices['Pixel 7'] } },
    // Firefox and Safari (WebKit). The virtual authenticator is Chromium only,
    // so these run the notes flows (ui/) with an emailed code for sign-in.
    // Passkey specs (browser/) never run here.
    { name: 'firefox', testMatch: /ui\/.*\.spec\.ts/, use: { ...devices['Desktop Firefox'] } },
    { name: 'webkit', testMatch: /ui\/.*\.spec\.ts/, use: { ...devices['Desktop Safari'] } },
    // Pictures of key screens, report only. Chromium, a fixed desktop window,
    // one locale, one time zone, light, no motion. The specs skip unless
    // E2E_SCREENSHOTS is set.
    // The upgrade check: v2 applied over a live v1. It changes the program for
    // the whole stack, so it depends on every other test project and runs last,
    // alone. Locally, `--project=upgrade --no-deps` runs it by itself.
    {
      name: 'upgrade',
      testMatch: /upgrade\/.*\.spec\.ts/,
      dependencies: ['api', 'browser', 'phone', 'firefox', 'webkit'],
      use: { ...devices['Desktop Chrome'] },
    },
    // What notes v2 adds after the upgrade: search, password links. It needs
    // v2 served, so it runs after `upgrade`. Locally, `--project=v2 --no-deps`
    // applies v2 itself and runs alone.
    {
      name: 'v2',
      testMatch: /v2\/.*\.spec\.ts/,
      dependencies: ['upgrade'],
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'screenshots',
      testMatch: /screenshots\/screens\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1280, height: 720 },
        deviceScaleFactor: 1,
        locale: 'en-US',
        timezoneId: 'UTC',
        colorScheme: 'light',
        reducedMotion: 'reduce',
      },
    },
    {
      name: 'screenshots-phone',
      testMatch: /screenshots\/phone\.spec\.ts/,
      use: { ...devices['Pixel 7'], locale: 'en-US', timezoneId: 'UTC', colorScheme: 'light', reducedMotion: 'reduce' },
    },
  ],
  webServer:
    (process.env['E2E_TARGET'] ?? 'throwaway') === 'throwaway'
      ? {
          command: 'bash stack.sh',
          url: `${page}/`,
          reuseExistingServer: !ci,
          // A cold Postgres, the door's first boot and a Vite build.
          timeout: 180_000,
          stdout: 'pipe',
          stderr: 'pipe',
          gracefulShutdown: { signal: 'SIGTERM', timeout: 10_000 },
        }
      : undefined,
});
