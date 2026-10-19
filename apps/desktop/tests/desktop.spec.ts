import { test, expect } from "@playwright/test";
import type { Request } from "../src/protocol";

test("an uncertain Enable retries its original identity and payload", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const calls: unknown[] = [];
    Object.assign(window, { previewCalls: calls });
    window.addEventListener("quad9-preview-request", (event) =>
      calls.push((event as CustomEvent).detail),
    );
  });
  await page.goto("/?preview=lost-response");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("checkbox", { name: "IPv4", exact: true }).check();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page.getByRole("button", { name: "Enable Quad9", exact: true }).click();
  await expect(
    page.getByText("The result is uncertain.", { exact: false }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Retry last request", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Quad9 configured" }),
  ).toBeVisible();
  const calls: Request[] = await page.evaluate(
    () => (window as unknown as { previewCalls: Request[] }).previewCalls,
  );
  const mutations = calls.filter((c) => c.command.type === "enable_quad9");
  expect(mutations).toHaveLength(2);
  expect(mutations[0]).toEqual(mutations[1]);
});

test("production browser content cannot activate sample service data", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:1421/?preview=configured");
  await expect(
    page.getByRole("heading", { name: "Service unavailable" }),
  ).toBeVisible();
  await expect(
    page.getByText("INTERACTIVE PREVIEW", { exact: false }),
  ).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Enable Quad9" })).toHaveCount(
    0,
  );
});

test("explicit scope, enable, diagnostic, and restore flow", async ({
  page,
}) => {
  await page.goto("/?preview=off");
  await expect(
    page.getByRole("heading", { name: "Ready when you are" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Enable Quad9" }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("checkbox", { name: "IPv4", exact: true }).check();
  await expect(
    page.getByRole("checkbox", { name: "IPv6", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page.getByRole("button", { name: "Enable Quad9" }).click();
  await expect(
    page.getByRole("heading", { name: "Quad9 configured" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(
    page.getByText("DNS over HTTPS observed", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page
    .getByRole("button", { name: "Turn off and restore DNS", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Ready when you are" }),
  ).toBeVisible();
});

test("service failure never displays off or enables mutation", async ({
  page,
}) => {
  await page.goto("/?preview=unavailable");
  await expect(
    page.getByRole("heading", { name: "Service unavailable" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Reconnect to service" }),
  ).toBeEnabled();
  await expect(page.getByRole("button", { name: "Enable Quad9" })).toHaveCount(
    0,
  );
});

test("recovery remains visible and allows restoration", async ({ page }) => {
  await page.goto("/?preview=recovery");
  await expect(
    page.getByRole("heading", { name: "Recovery required" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Turn off and restore DNS", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Ready when you are" }),
  ).toBeVisible();
});

test("theme persists and excluded adapters explain their state", async ({
  page,
}) => {
  await page.goto("/?preview=off");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: "Dark", exact: true }).check();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByText("Virtual adapter is excluded")).toBeVisible();
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
});

test("keyboard, high contrast, reduced motion, and narrow layout", async ({
  page,
}) => {
  await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
  await page.setViewportSize({ width: 320, height: 640 });
  await page.goto("/?preview=off");
  await expect(
    page.getByRole("heading", { name: "Ready when you are" }),
  ).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "Connection", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Tab");
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("heading", { name: "Diagnostics", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("capture review screenshots of the running UI", async ({ page }) => {
  await page.goto("/?preview=off");
  await expect(
    page.getByRole("heading", { name: "Ready when you are" }),
  ).toBeVisible();
  await page.screenshot({
    path: "../../output/playwright/connection-light.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("checkbox", { name: "IPv4", exact: true }).check();
  await page.getByRole("radio", { name: "Dark", exact: true }).check();
  await page.screenshot({
    path: "../../output/playwright/settings-dark.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page.getByRole("button", { name: "Enable Quad9" }).click();
  await expect(
    page.getByRole("heading", { name: "Quad9 configured" }),
  ).toBeVisible();
  await page.screenshot({
    path: "../../output/playwright/configured-dark.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(
    page.getByText("DNS over HTTPS observed", { exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "../../output/playwright/diagnostics-dark.png",
    fullPage: true,
  });
});
