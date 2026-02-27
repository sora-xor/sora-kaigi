import { defineConfig, devices } from '@playwright/test';

const port = 4317;

export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  fullyParallel: false,
  outputDir: '../../output/playwright/web',
  reporter: 'line',
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure'
  },
  webServer: {
    command: `npm run dev -- --host 127.0.0.1 --port ${port} --strictPort`,
    port,
    timeout: 120_000,
    reuseExistingServer: false
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] }
    },
    {
      name: 'webkit',
      use: { ...devices['Desktop Safari'] }
    },
    {
      name: 'firefox',
      use: { ...devices['Desktop Firefox'] }
    }
  ]
});
