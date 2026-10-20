import { test, expect, type Page } from "@playwright/test";
import type { Request, Result } from "../src/protocol";

type StartupWindow = Window & {
  startupTest: {
    calls: string[];
    release: ((error?: string) => void)[];
    invalidationDepth: number | null;
    emit: (event: string) => void;
  };
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    let next = 0;
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    const bridge: StartupWindow["startupTest"] = {
      calls: [],
      release: [],
      invalidationDepth: null,
      emit(event) {
        listeners
          .get(event)
          ?.forEach((id) => callbacks.get(id)?.({ event, id, payload: 1 }));
      },
    };
    Object.assign(window, {
      isTauri: true,
      startupTest: bridge,
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "main" } },
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
            command === "network_monitor_error" ||
            command === "plugin:event|unlisten"
          )
            return null;
          if (command === "interface_context")
            return {
              view: "connection",
              document_id: 1,
              diagnostic_pending: false,
              tray_result: null,
            };
          if (command === "plugin:window|is_visible") return true;
          if (command !== "service_request" || !args.request)
            throw new Error(`Unexpected IPC ${command}`);
          const request = args.request;
          bridge.calls.push(request.command.type);
          await new Promise<void>((resolve, reject) => {
            bridge.release.push((error) => {
              if (error) reject(new Error(error));
              else resolve();
              if (
                request.command.type === "list_adapters" &&
                bridge.invalidationDepth !== null
              ) {
                const depth = bridge.invalidationDepth;
                bridge.invalidationDepth = null;
                const enqueue = (remaining: number) => {
                  if (remaining) queueMicrotask(() => enqueue(remaining - 1));
                  else bridge.emit("service-changed");
                };
                enqueue(depth);
              }
            });
          });
          const result: Result =
            request.command.type === "get_status"
              ? {
                  type: "status",
                  revision: 0,
                  journal: null,
                  recovery_blocked: false,
                  observed_matches: false,
                }
              : { type: "adapters", adapters: [] };
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
  await calls(page, ["get_status"]);
});

async function calls(page: Page, expected: string[]) {
  await expect
    .poll(() =>
      page.evaluate(() => (window as StartupWindow).startupTest.calls),
    )
    .toEqual(expected);
}
async function release(page: Page, index: number, error?: string) {
  await page.evaluate(
    ({ index, error }) =>
      (window as StartupWindow).startupTest.release[index](error),
    { index, error },
  );
}
async function invalidate(page: Page) {
  await page.evaluate(() => {
    const bridge = (window as StartupWindow).startupTest;
    window.dispatchEvent(new Event("focus"));
    bridge.emit("network-changed");
    bridge.emit("service-changed");
    document.dispatchEvent(new Event("visibilitychange"));
  });
  // Drain asynchronous visibility replies before checking for duplicate reads.
  await page.evaluate(() => new Promise(requestAnimationFrame));
}

test("startup coalesces focus and network events and waits for a current result", async ({
  page,
}) => {
  await invalidate(page);
  await calls(page, ["get_status"]);
  await release(page, 0);
  await calls(page, ["get_status", "get_status"]);
  await expect(page.getByRole("status")).toHaveText("Reading service state");
  expect(
    await page.evaluate(
      () =>
        performance.getEntriesByName("quad9-startup-settled", "mark").length,
    ),
  ).toBe(0);
  await release(page, 1);
  await calls(page, ["get_status", "get_status", "list_adapters"]);
  await expect(page.getByRole("status")).toHaveText("Reading service state");
  await release(page, 2);
  await expect(page.getByRole("status")).toHaveText("Ready when you are");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          performance.getEntriesByName("quad9-startup-settled", "mark").length,
      ),
    )
    .toBe(1);
});

test("a network event during adapter inspection requires a complete fresh read", async ({
  page,
}) => {
  await release(page, 0);
  await calls(page, ["get_status", "list_adapters"]);
  await invalidate(page);
  await calls(page, ["get_status", "list_adapters"]);
  await release(page, 1);
  await calls(page, ["get_status", "list_adapters", "get_status"]);
  await expect(page.getByRole("status")).toHaveText("Reading service state");
  await release(page, 2);
  await calls(page, [
    "get_status",
    "list_adapters",
    "get_status",
    "list_adapters",
  ]);
  await release(page, 3);
  await expect(page.getByRole("status")).toHaveText("Ready when you are");
});

test("an obsolete startup error does not cancel the fresh read", async ({
  page,
}) => {
  await invalidate(page);
  await release(page, 0, "Obsolete connection failure");
  await calls(page, ["get_status", "get_status"]);
  await expect(page.getByRole("status")).toHaveText("Reading service state");
  await release(page, 1);
  await calls(page, ["get_status", "get_status", "list_adapters"]);
  await release(page, 2);
  await expect(page.getByRole("status")).toHaveText("Ready when you are");
  await expect(page.getByText("Obsolete connection failure")).toHaveCount(0);
});

test("a current startup failure ends loading and remains visible", async ({
  page,
}) => {
  await release(page, 0, "Current connection failure");
  await expect(page.getByRole("status")).toHaveText("Service unavailable");
  await expect(page.getByText("Current connection failure")).toBeVisible();
  await calls(page, ["get_status"]);
});

test("continuous startup invalidation stops with a reconnect action", async ({
  page,
}) => {
  for (let attempt = 0; attempt < 3; attempt++) {
    await calls(page, Array(attempt + 1).fill("get_status"));
    await invalidate(page);
    await release(page, attempt);
  }
  await expect(page.getByRole("status")).toHaveText("Service unavailable");
  await expect(
    page.getByText(
      "The network keeps changing. Wait for it to settle, then reconnect.",
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Reconnect to service", exact: true }),
  ).toBeEnabled();
  await calls(page, ["get_status", "get_status", "get_status"]);
});

test("promise-delivered invalidations survive every startup handoff boundary", async ({
  page,
}) => {
  // Sweep adjacent promise turns around adapter completion, including the
  // handoff from startup reads to normal event refreshes.
  for (const depth of [0, 1, 2, 3, 4, 5, 6, 7, 8]) {
    if (depth) {
      await page.reload();
      await calls(page, ["get_status"]);
    }
    await release(page, 0);
    await calls(page, ["get_status", "list_adapters"]);
    await page.evaluate((depth) => {
      (window as StartupWindow).startupTest.invalidationDepth = depth;
    }, depth);
    await release(page, 1);
    await calls(page, ["get_status", "list_adapters", "get_status"]);
    await release(page, 2);
    await calls(page, [
      "get_status",
      "list_adapters",
      "get_status",
      "list_adapters",
    ]);
    await release(page, 3);
    await expect(page.getByRole("status")).toHaveText("Ready when you are");
  }
});
