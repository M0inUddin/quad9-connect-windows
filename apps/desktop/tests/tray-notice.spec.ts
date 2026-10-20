import { test, expect, type Page } from "@playwright/test";
import type { Request } from "../src/protocol";

const noticeName = "Closing and quitting Quad9";

async function expectOnlyReads(page: Page) {
  const commands = await page.evaluate(() =>
    (window as unknown as { trayNoticeCalls: Request[] }).trayNoticeCalls.map(
      (request) => request.command.type,
    ),
  );
  expect(commands).toContain("get_status");
  expect(commands).toContain("list_adapters");
  expect(
    commands.filter(
      (command) => !["get_status", "list_adapters"].includes(command),
    ),
  ).toEqual([]);
}

test("first-use tray guidance keeps the primary action visible and preserves preferences", async ({
  page,
}) => {
  await page.setViewportSize({ width: 480, height: 680 });
  await page.addInitScript(() => {
    const calls: Request[] = [];
    Object.assign(window, { trayNoticeCalls: calls });
    window.addEventListener("quad9-preview-request", (event) =>
      calls.push((event as CustomEvent<Request>).detail),
    );
  });
  await page.goto("/?preview=off");
  const notice = page.getByRole("note", { name: noticeName });
  await expect(notice).toBeVisible();
  await expect(notice).toContainText("Quit interface leaves DNS unchanged");
  const primary = page.getByRole("button", {
    name: "Enable Quad9",
    exact: true,
  });
  await expect(primary).toBeVisible();
  const bounds = await primary.boundingBox();
  expect(bounds).not.toBeNull();
  await page.screenshot({
    path: "../../output/playwright/tray-notice-light.png",
    fullPage: true,
  });
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(680);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: "Dark", exact: true }).check();
  await page.getByRole("checkbox", { name: "IPv4", exact: true }).check();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page.screenshot({
    path: "../../output/playwright/tray-notice-dark.png",
    fullPage: true,
  });
  await notice.getByRole("button", { name: "Got it", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(notice).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Connection", exact: true }),
  ).toBeFocused();
  await expectOnlyReads(page);
  await page.reload();
  await expect(page.getByRole("status")).toHaveText("Ready when you are");
  await expect(notice).toHaveCount(0);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(primary).toBeEnabled();
  await expectOnlyReads(page);
});

test("blocked notice storage still permits dismissal for the current document", async ({
  page,
}) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "localStorage", {
      get() {
        throw new DOMException("Blocked", "SecurityError");
      },
    });
  });
  await page.goto("/?preview=off");
  const notice = page.getByRole("note", { name: noticeName });
  await expect(notice).toBeVisible();
  await notice.getByRole("button", { name: "Got it", exact: true }).click();
  await expect(notice).toHaveCount(0);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: "Dark", exact: true }).check();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(notice).toHaveCount(0);
  await page.reload();
  await expect(notice).toBeVisible();
  await expect(page.getByRole("status")).toHaveText("Ready when you are");
});
