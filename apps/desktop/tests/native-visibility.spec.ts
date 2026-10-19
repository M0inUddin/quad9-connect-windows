import { test, expect } from "@playwright/test";
import type { Request, Result } from "../src/protocol";

type TestWindow = Window & {
  visibilityTest: {
    visible: boolean;
    holdVisibility: boolean;
    holdMutation: boolean;
    queries: number;
    pending: ((visible: boolean) => void)[];
    releaseMutation?: () => void;
    calls: Request[];
    retention: boolean[];
    emit: (event: string, payload?: unknown) => void;
  };
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    let next = 0;
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    const adapterId = "11111111-1111-1111-1111-111111111111";
    const bridge: TestWindow["visibilityTest"] = {
      visible: true,
      holdVisibility: false,
      holdMutation: false,
      queries: 0,
      pending: [],
      calls: [],
      retention: [],
      emit(event, payload) {
        listeners
          .get(event)
          ?.forEach((id) => callbacks.get(id)?.({ event, id, payload }));
      },
    };
    localStorage.setItem(
      "quad9-preferences-v1",
      JSON.stringify({
        targets: [{ adapter_id: adapterId, family: "ipv4" }],
      }),
    );
    Object.assign(window, {
      isTauri: true,
      visibilityTest: bridge,
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "main" } },
        transformCallback(callback: (event: unknown) => void) {
          callbacks.set(++next, callback);
          return next;
        },
        async invoke(
          command: string,
          args: {
            event?: string;
            handler?: number;
            request?: Request;
            retain?: boolean;
          } = {},
        ) {
          if (command === "plugin:event|listen") {
            if (!listeners.has(args.event!))
              listeners.set(args.event!, new Set());
            listeners.get(args.event!)!.add(args.handler!);
            return args.handler;
          }
          if (
            command === "plugin:event|unlisten" ||
            command === "network_monitor_error"
          )
            return null;
          if (command === "interface_context")
            return {
              view: "connection",
              document_id: 1,
              diagnostic_pending: false,
              tray_result: null,
            };
          if (command === "retain_interface") {
            bridge.retention.push(args.retain!);
            return null;
          }
          if (command === "plugin:window|is_visible") {
            bridge.queries++;
            if (bridge.holdVisibility)
              return new Promise<boolean>((resolve) =>
                bridge.pending.push(resolve),
              );
            return bridge.visible;
          }
          if (command !== "service_request" || !args.request)
            throw new Error(`Unexpected IPC ${command}`);
          const request = args.request;
          bridge.calls.push(request);
          let result: Result;
          switch (request.command.type) {
            case "get_status":
              result = {
                type: "status",
                revision: 0,
                journal: null,
                recovery_blocked: false,
                observed_matches: false,
              };
              break;
            case "list_adapters":
              result = {
                type: "adapters",
                adapters: [
                  {
                    id: adapterId,
                    name: "Test Ethernet",
                    candidate: true,
                    exclusion_reason: null,
                    ipv4_enabled: true,
                    ipv6_enabled: false,
                    ipv6_usable: false,
                    has_ipv4_dns: true,
                    has_ipv6_dns: false,
                  },
                ],
              };
              break;
            case "enable_quad9":
              if (bridge.holdMutation)
                await new Promise<void>((resolve) => {
                  bridge.releaseMutation = resolve;
                });
              throw new Error("Simulated lost reply");
            default:
              throw new Error(`Unexpected command ${request.command.type}`);
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
  await expect(
    page.getByRole("button", { name: "Enable Quad9", exact: true }),
  ).toBeEnabled();
});

test("hidden native windows defer event reads and reopen without replaying an uncertain mutation", async ({
  page,
}) => {
  await page.getByRole("button", { name: "Enable Quad9", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Retry last request", exact: true }),
  ).toBeEnabled();
  const before = await page.evaluate(
    () => (window as TestWindow).visibilityTest.calls.length,
  );
  await page.evaluate(() => {
    const bridge = (window as TestWindow).visibilityTest;
    bridge.visible = false;
    bridge.emit("network-changed", 1);
    bridge.emit("service-changed");
  });
  await expect
    .poll(() =>
      page.evaluate(() => (window as TestWindow).visibilityTest.queries),
    )
    .toBeGreaterThanOrEqual(2);
  // Browser visibility remains true, just as it can in a hidden WebView2 host.
  expect(await page.evaluate(() => document.hidden)).toBe(false);
  await page.evaluate(() => new Promise(requestAnimationFrame));
  expect(
    await page.evaluate(
      () => (window as TestWindow).visibilityTest.calls.length,
    ),
  ).toBe(before);
  expect(
    await page.evaluate(() =>
      (window as TestWindow).visibilityTest.retention.at(-1),
    ),
  ).toBe(true);
  await page.evaluate(() => {
    const bridge = (window as TestWindow).visibilityTest;
    bridge.visible = true;
    // No browser focus event is sent by this reopen.
    bridge.emit("navigate", "connection");
  });
  await expect(
    page.getByRole("heading", { name: "Ready when you are" }),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() => (window as TestWindow).visibilityTest.calls.length),
    )
    .toBe(before + 2);
  const mutations = await page.evaluate(() =>
    (window as TestWindow).visibilityTest.calls.filter(
      (r) => r.command.type === "enable_quad9",
    ),
  );
  expect(mutations).toHaveLength(1);
  await page
    .getByRole("button", { name: "Retry last request", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Retry last request", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(() =>
      (window as TestWindow).visibilityTest.calls.filter(
        (r) => r.command.type === "enable_quad9",
      ),
    ),
  ).toEqual([mutations[0], mutations[0]]);
});

test("an old native visibility response cannot refresh after a newer hidden event", async ({
  page,
}) => {
  const before = await page.evaluate(
    () => (window as TestWindow).visibilityTest.calls.length,
  );
  await page.evaluate(() => {
    const bridge = (window as TestWindow).visibilityTest;
    bridge.holdVisibility = true;
    bridge.emit("network-changed", 1);
    bridge.emit("network-changed", 2);
  });
  await expect
    .poll(() =>
      page.evaluate(() => (window as TestWindow).visibilityTest.pending.length),
    )
    .toBe(2);
  await page.evaluate(() => {
    const bridge = (window as TestWindow).visibilityTest;
    bridge.pending[1](false);
    bridge.pending[0](true);
  });
  await page.evaluate(() => new Promise(requestAnimationFrame));
  expect(
    await page.evaluate(
      () => (window as TestWindow).visibilityTest.calls.length,
    ),
  ).toBe(before);
});

test("a delayed visibility response queues behind an action that has since started", async ({
  page,
}) => {
  await page.evaluate(() => {
    const bridge = (window as TestWindow).visibilityTest;
    bridge.holdVisibility = true;
    bridge.holdMutation = true;
    bridge.emit("service-changed");
  });
  await expect
    .poll(() =>
      page.evaluate(() => (window as TestWindow).visibilityTest.pending.length),
    )
    .toBe(1);
  await page.getByRole("button", { name: "Enable Quad9", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () => !!(window as TestWindow).visibilityTest.releaseMutation,
      ),
    )
    .toBe(true);
  await page.evaluate(() =>
    (window as TestWindow).visibilityTest.pending[0](true),
  );
  await page.evaluate(() => new Promise(requestAnimationFrame));
  expect(
    await page.evaluate(() =>
      (window as TestWindow).visibilityTest.calls.map((r) => r.command.type),
    ),
  ).toEqual(["get_status", "list_adapters", "enable_quad9"]);
  await page.evaluate(() =>
    (window as TestWindow).visibilityTest.releaseMutation!(),
  );
  await expect(
    page.getByRole("button", { name: "Retry last request", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(() =>
      (window as TestWindow).visibilityTest.calls.map((r) => r.command.type),
    ),
  ).toEqual([
    "get_status",
    "list_adapters",
    "enable_quad9",
    "get_status",
    "list_adapters",
  ]);
});
