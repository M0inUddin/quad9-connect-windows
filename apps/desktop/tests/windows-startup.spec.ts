import { test, expect } from "@playwright/test";
import type { Request } from "../src/protocol";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    let next = 0;
    const bridge = {
      mode: "off",
      available: true,
      writes: [] as string[],
      reads: 0,
      failAfterWrite: false,
      hold: false,
      release: null as (() => void) | null,
      dnsCommands: [] as string[],
    };
    Object.assign(window, {
      isTauri: true,
      startupPreferenceTest: bridge,
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "main" } },
        transformCallback() {
          return ++next;
        },
        async invoke(
          command: string,
          args: { mode?: string; request?: Request } = {},
        ) {
          if (command === "plugin:event|listen") return ++next;
          if (
            [
              "plugin:event|unlisten",
              "network_monitor_error",
              "retain_interface",
            ].includes(command)
          )
            return null;
          if (command === "plugin:window|is_visible") return true;
          if (command === "interface_context")
            return {
              view: "connection",
              document_id: 1,
              diagnostic_pending: false,
              tray_result: null,
            };
          if (command === "windows_startup") {
            if (args.mode === undefined) bridge.reads++;
            else {
              bridge.writes.push(args.mode);
              if (bridge.hold)
                await new Promise<void>((resolve) => {
                  bridge.release = resolve;
                });
              bridge.mode = args.mode;
              if (bridge.failAfterWrite) {
                bridge.failAfterWrite = false;
                throw "Reply timed out after Windows saved the setting.";
              }
            }
            return {
              available: bridge.available,
              mode: bridge.available ? bridge.mode : null,
              message: bridge.available
                ? null
                : "Install Quad9 Connect to manage Windows startup.",
            };
          }
          if (command === "service_request" && args.request) {
            bridge.dnsCommands.push(args.request.command.type);
            return {
              version: 1,
              id: args.request.id,
              result:
                args.request.command.type === "get_status"
                  ? {
                      type: "status",
                      revision: 0,
                      journal: null,
                      observed_matches: false,
                      recovery_blocked: false,
                    }
                  : { type: "adapters", adapters: [] },
            };
          }
          throw new Error(`Unexpected IPC: ${command}`);
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    });
    localStorage.setItem(
      "quad9-preferences-v1",
      JSON.stringify({ theme: "dark", targets: [] }),
    );
  });
});

test("startup stays off until selected, persists on re-entry, and sends no DNS mutation", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.getByRole("status")).toHaveText("Ready when you are");
  expect(
    await page.evaluate(() => (window as any).startupPreferenceTest.reads),
  ).toBe(0);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const group = page.getByRole("group", {
    name: "Start Quad9 when signing in to Windows",
  });
  await expect(
    group.getByRole("radio", { name: "Off", exact: true }),
  ).toBeChecked();
  expect(
    await page.evaluate(() => (window as any).startupPreferenceTest.writes),
  ).toEqual([]);
  await group.getByRole("radio", { name: "Tray", exact: true }).check();
  await expect(
    group.getByRole("radio", { name: "Tray", exact: true }),
  ).toBeChecked();
  await expect(page.getByText("Windows startup setting saved.")).toBeVisible();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    group.getByRole("radio", { name: "Tray", exact: true }),
  ).toBeChecked();
  await group.getByRole("radio", { name: "Open window", exact: true }).check();
  await expect(
    group.getByRole("radio", { name: "Open window", exact: true }),
  ).toBeChecked();
  await group.getByRole("radio", { name: "Off", exact: true }).check();
  await expect(
    group.getByRole("radio", { name: "Off", exact: true }),
  ).toBeChecked();
  expect(
    await page.evaluate(() => (window as any).startupPreferenceTest.writes),
  ).toEqual(["tray", "window", "off"]);
  expect(
    await page.evaluate(() =>
      (window as any).startupPreferenceTest.dnsCommands.filter(
        (command: string) => !["get_status", "list_adapters"].includes(command),
      ),
    ),
  ).toEqual([]);
  await expect(
    page.getByRole("radio", { name: "Dark", exact: true }),
  ).toBeChecked();
});

test("an uncertain reply blocks more changes until the actual setting is read again", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const group = page.getByRole("group", {
    name: "Start Quad9 when signing in to Windows",
  });
  await expect(
    group.getByRole("radio", { name: "Off", exact: true }),
  ).toBeChecked();
  await page.evaluate(() => {
    const bridge = (window as any).startupPreferenceTest;
    bridge.failAfterWrite = true;
    bridge.hold = true;
  });
  await group.getByRole("radio", { name: "Tray", exact: true }).check();
  await expect(
    group.getByRole("radio", { name: "Open window", exact: true }),
  ).toBeDisabled();
  await page.evaluate(() => (window as any).startupPreferenceTest.release());
  await expect(
    page.getByRole("alert").filter({ hasText: "Reply timed out" }),
  ).toBeVisible();
  await expect(
    group.getByRole("radio", { name: "Off", exact: true }),
  ).toBeChecked();
  await expect(
    group.getByRole("radio", { name: "Tray", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "Read startup setting again", exact: true })
    .click();
  await expect(
    group.getByRole("radio", { name: "Tray", exact: true }),
  ).toBeChecked();
  await expect(
    group.getByRole("radio", { name: "Tray", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(() => (window as any).startupPreferenceTest.writes),
  ).toEqual(["tray"]);
});

test("portable app explains installation requirement without offering startup changes", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    (window as any).startupPreferenceTest.available = false;
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    page.getByText("Install Quad9 Connect to manage Windows startup."),
  ).toBeVisible();
  await expect(
    page.getByRole("group", { name: "Start Quad9 when signing in to Windows" }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(() => (window as any).startupPreferenceTest.writes),
  ).toEqual([]);
});

test("browser fixture shows Windows startup in both themes", async ({
  page,
}) => {
  // This page has no native bridge shim from beforeEach.
  const fixture = await page.context().newPage();
  await fixture.goto("/?preview=off");
  await expect(
    fixture.getByText("INTERACTIVE PREVIEW", { exact: false }),
  ).toBeVisible();
  await fixture.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(fixture.getByText("Ethernet", { exact: true })).toBeVisible();
  for (const theme of ["Light", "Dark"]) {
    await fixture.getByRole("radio", { name: theme, exact: true }).check();
    await fixture.screenshot({
      path: `../../output/playwright/windows-startup-${theme.toLowerCase()}.png`,
      fullPage: true,
    });
  }
  await fixture.close();
});
