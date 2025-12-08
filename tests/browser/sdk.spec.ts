import { test, expect } from "@playwright/test";

test("GreenticGUI SDK loads and attaches worker", async ({ page, baseURL }) => {
  const targetUrl = `${baseURL}/tests/sdk-harness`;
  await page.goto(targetUrl);

  // SDK global should be present.
  const hasSdk = await page.evaluate(() => typeof (window as any).GreenticGUI !== "undefined");
  expect(hasSdk).toBeTruthy();

  const slot = page.locator("#worker-slot");
  await expect(slot).toBeVisible();

  // Dataset should reflect attachment.
  await expect(slot).toHaveAttribute("data-greentic-worker", "worker.test");
});
