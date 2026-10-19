import { test, expect } from "@playwright/test";
import type { Request, Result } from "../src/protocol";

test("stale persisted adapters cannot trap Enable in an unsupported scope", async ({
  page,
}) => {
  await page.addInitScript(() =>
    localStorage.setItem(
      "quad9-preferences-v1",
      JSON.stringify({
        theme: "system",
        targets: [
          {
            adapter_id: "11111111-1111-1111-1111-111111111111",
            family: "ipv4",
          },
          {
            adapter_id: "94ab1355-47d6-4f27-9dab-e211af887a51",
            family: "ipv4",
          },
          {
            adapter_id: "a956d6e1-8910-4d20-9a67-5e34915c2a10",
            family: "ipv6",
          },
        ],
      }),
    ),
  );
  await page.goto("/?preview=off");
  const enable = page.getByRole("button", {
    name: "Enable Quad9",
    exact: true,
  });
  await expect(enable).toBeVisible();
  await expect(enable).toBeDisabled();
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem("quad9-preferences-v1")!).targets,
      ),
    )
    .toEqual([]);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("checkbox", { name: "IPv4", exact: true }).check();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await enable.click();
  await expect(
    page.getByRole("heading", { name: "Quad9 configured" }),
  ).toBeVisible();
});

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

test("diagnostics cannot replay or consume an uncertain DNS mutation", async ({
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
  // Reconnecting learns the current state but cannot settle the original receipt.
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
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
  const calls: Request[] = await page.evaluate(
    () => (window as unknown as { previewCalls: Request[] }).previewCalls,
  );
  expect(calls.filter((c) => c.command.type === "enable_quad9")).toHaveLength(
    1,
  );
  expect(
    calls.filter((c) => c.command.type === "run_diagnostics"),
  ).toHaveLength(1);
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page
    .getByRole("button", { name: "Retry last request", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Turn off and restore DNS", exact: true }),
  ).toBeEnabled();
  const retried: Request[] = await page.evaluate(
    () => (window as unknown as { previewCalls: Request[] }).previewCalls,
  );
  const mutations = retried.filter((c) => c.command.type === "enable_quad9");
  expect(mutations).toHaveLength(2);
  expect(mutations[0]).toEqual(mutations[1]);
});

test("an event during the final mutation refresh queues a current state read", async ({
  page,
}) => {
  await page.addInitScript(() => {
    let changed = false;
    window.addEventListener("quad9-preview-request", (event) => {
      const command = (event as CustomEvent<Request>).detail.command.type;
      if (command === "enable_quad9" || command === "restore_previous_settings")
        changed = true;
      else if (command === "get_status" && changed) {
        changed = false;
        setTimeout(() => window.dispatchEvent(new Event("online")), 50);
      }
    });
  });
  await page.goto("/?preview=off");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("checkbox", { name: "IPv4", exact: true }).check();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page.getByRole("button", { name: "Enable Quad9", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Quad9 configured" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Turn off and restore DNS", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Ready when you are" }),
  ).toBeVisible();
});

test("network events clear an old diagnostic without rewriting DNS", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const calls: unknown[] = [];
    Object.assign(window, { previewCalls: calls });
    window.addEventListener("quad9-preview-request", (event) =>
      calls.push((event as CustomEvent).detail),
    );
  });
  await page.goto("/?preview=configured");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(
    page.getByText("DNS over HTTPS observed", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await expect(page.getByText("Ready to check", { exact: true })).toBeVisible();
  await expect(
    page.getByText("DNS over HTTPS observed", { exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Run diagnostics", exact: true }),
  ).toBeEnabled();
  const calls: Request[] = await page.evaluate(
    () => (window as unknown as { previewCalls: Request[] }).previewCalls,
  );
  expect(
    calls.filter((c) =>
      ["enable_quad9", "restore_previous_settings"].includes(c.command.type),
    ),
  ).toHaveLength(0);
});

test("a diagnostic completing after a network change stays invalidated", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.addEventListener("quad9-preview-request", (event) => {
      if (
        (event as CustomEvent<Request>).detail.command.type ===
        "run_diagnostics"
      )
        setTimeout(() => window.dispatchEvent(new Event("offline")), 100);
    });
  });
  await page.goto("/?preview=slow-probe");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  await page
    .getByRole("button", { name: "Run diagnostics", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Run diagnostics", exact: true }),
  ).toBeEnabled();
  await expect(page.getByText("Ready to check", { exact: true })).toBeVisible();
  await expect(
    page.getByText("DNS over HTTPS observed", { exact: true }),
  ).toHaveCount(0);
});

for (const failureSource of ["event", "saved"] as const) {
  test(`a native monitoring failure from ${failureSource} rejects late evidence and retains its warning`, async ({
    page,
  }) => {
    // Mock the native event bridge for this failure case. Other native startup
    // and IPC behavior is exercised by the packaged Windows smoke test.
    await page.addInitScript((failureSource: "event" | "saved") => {
      let next = 0;
      let savedFailure: ((failure: unknown) => void) | undefined;
      const callbacks = new Map<number, (event: unknown) => void>();
      const listeners = new Map<string, Set<number>>();
      Object.assign(window, {
        isTauri: true,
        __TAURI_INTERNALS__: {
          transformCallback(callback: (event: unknown) => void) {
            callbacks.set(++next, callback);
            return next;
          },
          async invoke(
            command: string,
            args: { event?: string; handler?: number; request?: Request },
          ) {
            if (command === "plugin:event|listen") {
              if (!listeners.has(args.event!))
                listeners.set(args.event!, new Set());
              listeners.get(args.event!)!.add(args.handler!);
              return args.handler;
            }
            if (command === "plugin:event|unlisten") return null;
            if (command === "network_monitor_error")
              return failureSource === "saved"
                ? new Promise((resolve) => {
                    savedFailure = resolve;
                  })
                : null;
            if (command !== "service_request" || !args.request)
              throw new Error(`Unexpected test IPC: ${command}`);
            const request = args.request;
            let result: Result;
            switch (request.command.type) {
              case "get_status":
                result = {
                  type: "status",
                  revision: 0,
                  recovery_blocked: false,
                  observed_matches: false,
                  journal: null,
                };
                break;
              case "list_adapters":
                result = { type: "adapters", adapters: [] };
                break;
              case "run_diagnostics":
                result = {
                  type: "diagnostics",
                  report: {
                    outcome: "doh_observed",
                    elapsed_ms: 1,
                    checked_at_unix_seconds: 1,
                    scope: "One mocked lookup",
                  },
                };
                break;
              default:
                throw new Error(
                  "A monitoring failure must not issue a DNS mutation",
                );
            }
            return { version: 1, id: request.id, result };
          },
        },
        __TAURI_EVENT_PLUGIN_INTERNALS__: {
          unregisterListener(event: string, id: number) {
            listeners.get(event)?.delete(id);
            callbacks.delete(id);
          },
        },
        emitNativeTest(event: string, payload: unknown) {
          if (event === "network-monitor-error" && failureSource === "saved") {
            if (!savedFailure) throw new Error("The startup getter is not pending");
            savedFailure(payload);
            return;
          }
          listeners
            .get(event)
            ?.forEach((id) => callbacks.get(id)?.({ event, id, payload }));
        },
      });
    }, failureSource);
    await page.goto("/");
    await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
    await page
      .getByRole("button", { name: "Run diagnostics", exact: true })
      .click();
    await expect(
      page.getByText("DNS over HTTPS observed", { exact: true }),
    ).toBeVisible();
    await page.evaluate(() =>
      (
        window as unknown as {
          emitNativeTest: (event: string, payload: unknown) => void;
        }
      ).emitNativeTest("network-monitor-error", {
        generation: 7,
        message: "injected failure",
      }),
    );
    await expect(
      page.getByText("Automatic DNS checks are unavailable", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("DNS over HTTPS observed", { exact: true }),
    ).toHaveCount(0);
    // A tray lookup completed before the failure, but its event arrived later.
    await page.evaluate(() =>
      (
        window as unknown as {
          emitNativeTest: (event: string, payload: unknown) => void;
        }
      ).emitNativeTest("diagnostic-result", {
        generation: 6,
        result: {
          Ok: {
            version: 1,
            id: "delayed-tray-diagnostic",
            result: {
              type: "diagnostics",
              report: {
                outcome: "doh_observed",
                elapsed_ms: 1,
                checked_at_unix_seconds: 1,
                scope: "One delayed mocked tray lookup",
              },
            },
          },
        },
      }),
    );
    // Tab interactions flush the event's state update before the absence check.
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
    await expect(
      page.getByText("DNS over HTTPS observed", { exact: true }),
    ).toHaveCount(0);
    await page
      .getByRole("button", { name: "Run diagnostics", exact: true })
      .click();
    await expect(
      page.getByText("DNS over HTTPS observed", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("Automatic DNS checks are unavailable", { exact: true }),
    ).toBeVisible();
  });
}

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
  await expect(
    page.getByRole("button", { name: "Enable Quad9", exact: true }),
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
  await expect(
    page.getByRole("button", { name: "Turn off and restore DNS", exact: true }),
  ).toBeEnabled();
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
  await expect(
    page.getByRole("button", { name: "Run diagnostics", exact: true }),
  ).toBeEnabled();
  await page.screenshot({
    path: "../../output/playwright/diagnostics-dark.png",
    fullPage: true,
  });
});
