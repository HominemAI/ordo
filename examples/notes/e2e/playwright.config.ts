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
    // `ordo evolve` from v2 to v3 through the door, as a developer signed in
    // with `ordo login` (plan-versions.md, increment 8). It stores v3 beside
    // v2 and runs the step on the deployment's own database, so it runs
    // after `v2` and before `v3`. Locally, `--project=evolve --no-deps`.
    {
      name: 'evolve',
      testMatch: /evolve\/v2-to-v3\.spec\.ts/,
      dependencies: ['v2'],
      use: { ...devices['Desktop Chrome'] },
    },
    // Notes v3 over the running notes v2, with the page open (plan-versions.md,
    // increment 7a). It moves the stack to v3, so it runs after `evolve`.
    // Locally, `--project=v3 --no-deps` applies v2 itself and runs alone.
    {
      name: 'v3',
      testMatch: /v3\/.*\.spec\.ts/,
      dependencies: ['evolve'],
      use: { ...devices['Desktop Chrome'] },
    },
    // The pre-checks of `ordo evolve` over bad data in a tenant (increment 9),
    // from v3 to 0.3.1 (fixtures/notes-v3-1). It needs v3 serving, so it runs
    // after `v3`, last. Locally, `--project=evolve-checks --no-deps`.
    // Restore across versions (plan-versions.md increment 17): a tenant's
    // database restored from a snapshot taken on v2 while v3 serves; its pin
    // follows the data, then it moves again. It needs v3 active, so it runs
    // after `v3`. Locally, `--project=restore --no-deps`.
    {
      name: 'restore',
      testMatch: /restore\/.*\.spec\.ts/,
      dependencies: ['v3'],
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'evolve-checks',
      testMatch: /evolve\/prechecks\.spec\.ts/,
      dependencies: ['restore'],
      use: { ...devices['Desktop Chrome'] },
    },
    // attachments.size, orphaned by v3: exported, erased, then pruned
    // (plan-versions.md, increment 14). It prunes a column, so it runs last.
    {
      name: 'orphans',
      testMatch: /orphans\/.*\.spec\.ts/,
      dependencies: ['evolve-checks'],
      use: { ...devices['Desktop Chrome'] },
    },
    // A staging clone of a production tenant (plan-versions.md increment 12),
    // into the stack's second deployment. It puts staging in production's
    // chain while it runs, and `ordo apply` goes to a chain's first step, so
    // it runs after `orphans`, last. Locally, `--project=clone --no-deps`.
    {
      name: 'clone',
      testMatch: /clone\/.*\.spec\.ts/,
      dependencies: ['orphans'],
      use: { ...devices['Desktop Chrome'] },
    },
    // notes.body sealed on live data (plan-versions.md increment 13), from
    // v3 to 0.3.4 (versions written by the spec). It seals a field for the whole
    // stack, so it runs after `clone`. Locally, `--project=seal --no-deps`.
    {
      name: 'seal',
      testMatch: /seal\/.*\.spec\.ts/,
      dependencies: ['clone'],
      use: { ...devices['Desktop Chrome'] },
    },
    // The signed rehearsal (plan-versions.md increment 12b): staging
    // rehearses 0.3.4 to 0.3.5 on a clone and signs it; production refuses
    // the step until it accepts that. Version numbers only rise, so it runs
    // after `seal`, last. Locally, `--project=rehearsal --no-deps`.
    {
      name: 'rehearsal',
      testMatch: /rehearsal\/.*\.spec\.ts/,
      dependencies: ['seal'],
      use: { ...devices['Desktop Chrome'] },
    },
    // A rename on one tenant under live writes (plan-versions.md, increment
    // 11a): the move with dual writes, the soak, a revert and the close, with
    // the page open. It stores 0.3.6, above
    // `rehearsal`'s 0.3.5 and below `branch`'s 0.5.x, and version numbers only
    // rise, so it runs between them. Locally, `--project=moves --no-deps`.
    {
      name: 'moves',
      testMatch: /evolve\/dual-writes\.spec\.ts/,
      dependencies: ['rehearsal'],
      use: { ...devices['Desktop Chrome'] },
    },
    // A rename on 3 tenants moved by a rollout, and a rollback in the soak
    // (plan-versions.md, increment 11b). It stores 0.3.7, above `moves`'
    // 0.3.6 and below `branch`'s 0.5.x, so it runs between them. Locally,
    // `--project=rollout-moves --no-deps`.
    {
      name: 'rollout-moves',
      testMatch: /evolve\/every-tenant\.spec\.ts/,
      dependencies: ['moves'],
      use: { ...devices['Desktop Chrome'] },
    },
    // A `--fill` backfill on one tenant under live writes (plan-versions.md
    // increment 10b): the person types while the fill runs, and every row
    // ends filled. It stores 0.3.8, above `rollout-moves`' 0.3.7 and below
    // `branch`'s 0.5.x, so it runs between them. Locally,
    // `--project=fill --no-deps`.
    {
      name: 'fill',
      testMatch: /evolve\/fill\.spec\.ts/,
      dependencies: ['rollout-moves'],
      use: { ...devices['Desktop Chrome'] },
    },
    // Notes v3 splits a field (plan-versions.md, increment 15b): the
    // mapping asked for, the move, a revert in the soak and the close. It
    // stores 0.3.9, above `fill`'s 0.3.8 and below `branch`'s 0.5.x, so it
    // runs between them. Locally, `--project=split --no-deps`.
    {
      name: 'split',
      testMatch: /evolve\/split\.spec\.ts/,
      dependencies: ['fill'],
      use: { ...devices['Desktop Chrome'] },
    },
    // Dev branches as tenants on their own version (plan-versions.md
    // increment 5): 2 branches served side by side while the stack serves
    // its own. It applies programs, so it runs after `split`.
    // Locally, `--project=branch --no-deps`.
    {
      name: 'branch',
      testMatch: /branch\/.*\.spec\.ts/,
      dependencies: ['split'],
      use: { ...devices['Desktop Chrome'] },
    },
    // Tenant builds (plan-versions.md increment 16): a beta tenant serves the
    // branch while the rest serve main. It stores 0.6.0 and activates 0.6.1,
    // above `branch`'s 0.5.x, and version numbers only rise, so it runs after
    // `branch`, last.
    // Locally, `--project=tenant-build --no-deps`.
    {
      name: 'tenant-build',
      testMatch: /tenant-build\/.*\.spec\.ts/,
      dependencies: ['branch'],
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
