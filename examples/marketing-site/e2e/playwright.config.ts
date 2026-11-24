import { defineConfig, devices } from '@playwright/test';
const page = `http://localhost:${process.env['E2E_PAGE_PORT'] ?? '5391'}`;
export default defineConfig({
  testDir: 'tests', workers: 1, retries: 0, forbidOnly: !!process.env['CI'],
  timeout: 180_000, expect: { timeout: 20_000 }, reporter: [['list']],
  use: { baseURL: page, trace: 'retain-on-failure', screenshot: 'only-on-failure' },
  projects: [
    { name: 'api', testMatch: /api\/.*\.spec\.ts/ },
    { name: 'browser', testMatch: /browser\/.*\.spec\.ts/, use: { ...devices['Desktop Chrome'] } },
  ],
  webServer: {
    command: 'bash stack.sh', url: page, reuseExistingServer: false, timeout: 300_000,
    stdout: 'pipe', stderr: 'pipe', gracefulShutdown: { signal: 'SIGTERM', timeout: 15_000 },
  },
});
