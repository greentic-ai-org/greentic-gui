import { defineConfig } from "@playwright/test";

const baseURL = process.env.BASE_URL || "http://localhost:8080";

export default defineConfig({
  testDir: "tests/browser",
  use: {
    baseURL,
    headless: true,
  },
  reporter: [["list"]],
});
