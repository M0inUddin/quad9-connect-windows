import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

test("direct HTTPS cannot turn an unverified system resolver into protected DNS", async ({
  page,
}, testInfo) => {
  await page.goto("/?preview=off");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(
    page.getByText("9.9.9.9: HTTPS response · HTTP 405", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("2620:fe::fe: No HTTPS response", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Could not verify DoH · other transport observed", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByText("DNS over HTTPS observed", { exact: true }),
  ).toHaveCount(0);
  expect(
    (
      await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
        .analyze()
    ).violations,
  ).toEqual([]);
  await page.setViewportSize({ width: 480, height: 1040 });
  await page.screenshot({
    path: testInfo.outputPath("upstream-light.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: "Dark", exact: true }).check();
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page.screenshot({
    path: testInfo.outputPath("upstream-dark.png"),
    fullPage: true,
  });
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await expect(
    page.getByText("9.9.9.9: HTTPS response · HTTP 405", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByText("Not checked", { exact: true })).toBeVisible();
});

test("direct timeouts preserve an independent system DoH observation", async ({
  page,
}) => {
  await page.goto("/?preview=upstream-unavailable");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(
    page.getByText("DNS over HTTPS observed", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("9.9.9.9: Timed out", { exact: true }),
  ).toBeVisible();
});

test("an HTTP error stays visible and is never described as a DNS answer", async ({
  page,
}) => {
  await page.goto("/?preview=upstream-http-error");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(
    page.getByText("9.9.9.9: HTTPS response · HTTP 503", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("does not verify a DNS answer", { exact: false }),
  ).toBeVisible();
});

test("an older service reports a missing direct check without inventing a failure", async ({
  page,
}) => {
  await page.goto("/?preview=legacy-probe");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(
    page.getByText("Not provided by this service version", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("No HTTPS response", { exact: true }),
  ).toHaveCount(0);
});
