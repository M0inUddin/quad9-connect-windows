import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  workers: 2,
  use: {
    channel: process.env.PLAYWRIGHT_CHANNEL,
    baseURL: "http://127.0.0.1:1420",
    viewport: { width: 480, height: 720 },
    colorScheme: "light",
    reducedMotion: "reduce",
  },
  webServer: [
    {
      command: "npm run dev",
      url: "http://127.0.0.1:1420",
      reuseExistingServer: !process.env.CI,
    },
    {
      command:
        "npm exec vite -- preview --host 127.0.0.1 --port 1421 --strictPort",
      url: "http://127.0.0.1:1421",
      reuseExistingServer: !process.env.CI,
    },
  ],
  reporter: "list",
  outputDir: "../../output/playwright/results",
});
