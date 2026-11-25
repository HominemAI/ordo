import { defineConfig, devices } from '@playwright/test';
import base from './playwright.config';

// The notes e2e case with 2 images (docs/plan-versions.md §4, increment 6).
// Its own stack and its own run: the stack starts the previous Ordo release's
// `ordo-host` and swaps this build's in under it when the test asks
// (stack.sh, E2E_PREVIOUS_HOST). Every other project runs one Ordo, so this
// one is never part of playwright.config.ts's projects.
//
//     eval "$(bash scripts/previous-ordo.sh)"      # builds the previous release
//     cd examples/notes/e2e
//     npx playwright test -c playwright.ordo-upgrade.config.ts
//
// Locally a stack already running is not reused: a stack that was started
// without the previous Ordo would pass for the wrong reason.
export default defineConfig({
  ...base,
  fullyParallel: false,
  workers: 1,
  reporter: [
    ['list'],
    ['html', { outputFolder: 'report-ordo-upgrade/html', open: 'never' }],
    ['json', { outputFile: 'report-ordo-upgrade/results.json' }],
  ],
  outputDir: 'test-results-ordo-upgrade',
  projects: [
    {
      name: 'ordo-upgrade',
      testMatch: /ordo-upgrade\/.*\.spec\.ts/,
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: base.webServer && {
    ...(base.webServer as object),
    reuseExistingServer: false,
  },
});
