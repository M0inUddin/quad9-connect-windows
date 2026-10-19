import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

for (const theme of ["Light", "Dark"] as const) {
  for (const state of ["off", "configured", "recovery", "unavailable"]) {
    test(`${theme} ${state} screens meet automated WCAG checks`, async ({
      page,
    }) => {
      await page.goto(`/?preview=${state}`);
      await page.getByRole("button", { name: "Settings", exact: true }).click();
      await page.getByRole("radio", { name: theme, exact: true }).check();
      for (const view of ["Connection", "Diagnostics", "Settings"]) {
        await page.getByRole("button", { name: view, exact: true }).click();
        await expect(page.locator(".primary-button:disabled")).toHaveCount(
          (view === "Connection" && state === "off") ||
            (view === "Diagnostics" && state === "unavailable")
            ? 1
            : 0,
        );
        const audit = await new AxeBuilder({ page })
          .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
          .analyze();
        expect(
          audit.violations,
          JSON.stringify(audit.violations, null, 2),
        ).toEqual([]);
      }
    });
  }
}

test("adapter controls expose their group and diagnostic results are announced", async ({
  page,
}) => {
  await page.goto("/?preview=off");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const ethernet = page.getByRole("group", {
    name: "Ethernet Available",
    exact: true,
  });
  await ethernet.getByRole("checkbox", { name: "IPv4", exact: true }).focus();
  await page.keyboard.press("Space");
  await expect(
    ethernet.getByRole("checkbox", { name: "IPv4", exact: true }),
  ).toBeChecked();
  await expect(
    ethernet.getByRole("checkbox", { name: "IPv6", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page.getByRole("button", { name: "Enable Quad9", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("status")).toHaveText("Quad9 configured");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(page.getByRole("status")).toHaveText(
    "Diagnostic complete. DNS over HTTPS observed. Direct Quad9 HTTPS: 1 of 2 endpoints replied.",
  );
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page
    .getByRole("button", { name: "Turn off and restore DNS", exact: true })
    .focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("status")).toHaveText("Ready when you are");
});

test("200 percent content zoom reflows every screen without clipped controls", async ({
  page,
}) => {
  await page.setViewportSize({ width: 480, height: 680 });
  await page.goto("/?preview=off");
  await page.evaluate(() => {
    document.documentElement.style.zoom = "2";
  });
  for (const view of ["Connection", "Diagnostics", "Settings"]) {
    await page.getByRole("button", { name: view, exact: true }).click();
    const clipped = await page
      .locator(
        "button,input,h1,h2,h3,legend:not(.sr-only),.profile-details strong",
      )
      .evaluateAll((elements) =>
        elements
          .filter((el) => {
            const r = el.getBoundingClientRect();
            return (
              r.width > 0 &&
              (r.left < -1 ||
                r.right > innerWidth + 1 ||
                el.scrollWidth > el.clientWidth + 1)
            );
          })
          .map((el) => el.outerHTML),
      );
    expect(clipped).toEqual([]);
  }
  await page.screenshot({
    path: "../../output/playwright/settings-200-percent.png",
    fullPage: true,
  });
});

test("slow diagnostics leave navigation and theme controls responsive", async ({
  page,
}) => {
  await page.goto("/?preview=slow-probe");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(page.getByRole("status")).toHaveText(
    "Checking DNS and Quad9 reachability",
  );
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: "Dark", exact: true }).check();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByRole("status")).toHaveText(
    "Diagnostic complete. DNS over HTTPS observed. Direct Quad9 HTTPS: 1 of 2 endpoints replied.",
  );
});

test("a failed diagnostic cannot announce an earlier success", async ({
  page,
}) => {
  await page.goto("/?preview=failed-probe");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(page.getByRole("status")).toHaveText(
    "Diagnostic complete. DNS over HTTPS observed. Direct Quad9 HTTPS: 1 of 2 endpoints replied.",
  );
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(page.getByRole("alert")).toContainText(
    "The resolver check timed out.",
  );
  await expect(page.getByRole("status")).not.toContainText(
    "Diagnostic complete",
  );
  await expect(
    page.getByText("DNS over HTTPS observed", { exact: true }),
  ).toHaveCount(0);
});
