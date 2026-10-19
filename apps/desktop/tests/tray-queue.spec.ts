import { test, expect } from "@playwright/test";
import type { Request, Result } from "../src/protocol";

type BridgeWindow = Window & {
  trayQueueTest: {
    calls: string[];
    diagnostics: number;
    releaseInitial?: () => void;
    releaseDiagnostic?: () => void;
    emit: (event: string, payload: unknown) => void;
  };
};

test("tray diagnostics survive startup navigation and coalesce behind a busy action", async ({
  page,
}) => {
  await page.addInitScript(() => {
    let next = 0;
    let diagnosticPending = true;
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    const bridge: BridgeWindow["trayQueueTest"] = {
      calls: [],
      diagnostics: 0,
      emit(event, payload) {
        if (event === "navigate" && payload === "diagnostics") diagnosticPending = true;
        listeners
          .get(event)
          ?.forEach((id) => callbacks.get(id)?.({ event, id, payload }));
      },
    };
    const initialRead = new Promise<void>((resolve) => {
      bridge.releaseInitial = resolve;
    });
    Object.assign(window, {
      isTauri: true,
      trayQueueTest: bridge,
      __TAURI_INTERNALS__: {
        transformCallback(callback: (event: unknown) => void) {
          callbacks.set(++next, callback);
          return next;
        },
        async invoke(
          command: string,
          args: { event?: string; handler?: number; request?: Request } = {},
        ) {
          if (command === "plugin:event|listen") {
            if (!listeners.has(args.event!))
              listeners.set(args.event!, new Set());
            listeners.get(args.event!)!.add(args.handler!);
            return args.handler;
          }
          if (
            command === "plugin:event|unlisten" ||
            command === "retain_interface" ||
            command === "network_monitor_error"
          )
            return null;
          if (command === "interface_context") {
            const diagnostic_pending = diagnosticPending;
            diagnosticPending = false;
            return { view: "settings", diagnostic_pending, tray_result: null };
          }
          if (command !== "service_request" || !args.request)
            throw new Error(`Unexpected IPC ${command}`);
          const request = args.request;
          bridge.calls.push(request.command.type);
          let result: Result;
          switch (request.command.type) {
            case "get_status":
              await initialRead;
              result = {
                type: "status",
                revision: 0,
                journal: null,
                recovery_blocked: false,
                observed_matches: false,
              };
              break;
            case "list_adapters":
              result = { type: "adapters", adapters: [] };
              break;
            case "run_diagnostics":
              if (++bridge.diagnostics === 1)
                await new Promise<void>((resolve) => {
                  bridge.releaseDiagnostic = resolve;
                });
              result = {
                type: "diagnostics",
                report: {
                  outcome: "doh_observed",
                  elapsed_ms: 1,
                  checked_at_unix_seconds: 1,
                  scope: "Mocked lookup",
                },
              };
              break;
            default:
              throw new Error("A tray diagnostic must not send a DNS mutation");
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
    });
  });
  await page.goto("/");
  await expect
    .poll(() =>
      page.evaluate(
        () => !!(window as BridgeWindow).trayQueueTest.releaseInitial,
      ),
    )
    .toBe(true);
  await page.evaluate(() => {
    (window as BridgeWindow).trayQueueTest.emit("navigate", "settings");
  });
  await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeVisible();
  expect(
    await page.evaluate(
      () => (window as BridgeWindow).trayQueueTest.diagnostics,
    ),
  ).toBe(0);
  await page.evaluate(() =>
    (window as BridgeWindow).trayQueueTest.releaseInitial!(),
  );
  await expect
    .poll(() =>
      page.evaluate(() => (window as BridgeWindow).trayQueueTest.diagnostics),
    )
    .toBe(1);
  await page.evaluate(() => {
    (window as BridgeWindow).trayQueueTest.emit("navigate", "diagnostics");
    (window as BridgeWindow).trayQueueTest.emit("navigate", "diagnostics");
  });
  expect(
    await page.evaluate(
      () => (window as BridgeWindow).trayQueueTest.diagnostics,
    ),
  ).toBe(1);
  await page.evaluate(() =>
    (window as BridgeWindow).trayQueueTest.releaseDiagnostic!(),
  );
  await expect
    .poll(() =>
      page.evaluate(() => (window as BridgeWindow).trayQueueTest.diagnostics),
    )
    .toBe(2);
  await expect(page.getByRole("status")).toHaveText(
    "Diagnostic complete. DNS over HTTPS observed",
  );
  expect(
    await page.evaluate(
      () =>
        (window as BridgeWindow).trayQueueTest.calls.filter(
          (type) => type === "run_diagnostics",
        ).length,
    ),
  ).toBe(2);
});
