import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './tests/browser',
  workers: 1,
  timeout: 30_000,
  use: {
    baseURL: 'http://127.0.0.1:18787',
    headless: true,
    launchOptions: { executablePath: process.env.CHROME_BIN || undefined },
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'node scripts/e2e-server.mjs',
    url: 'http://127.0.0.1:18787/health/ready',
    timeout: 30_000,
    reuseExistingServer: false,
  },
});
