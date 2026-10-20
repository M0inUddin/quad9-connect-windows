import { freePort, connect, reopen, closeWindow, memoryTarget } from "./native-session.mjs";
// Real embedded assets, Rust IPC, and native close/reopen. Never sends a DNS mutation.
import { expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, writeFile, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { createHash } from "node:crypto";
import { cpus, totalmem, version } from "node:os";

if (process.platform !== "win32" || !process.argv[2])
  throw new Error("Pass a built Windows desktop executable.");
const executable = resolve(process.argv[2]);
const runs = Number(process.argv[3] ?? 1);
if (!Number.isInteger(runs) || runs < 1 || runs > 100)
  throw new Error("Invalid startup run count.");
const output = resolve(
  process.argv[4] ?? "../../output/playwright/native-validation",
);
await mkdir(output, { recursive: true });
const isVisible = (page) =>
  page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("plugin:window|is_visible", { label: "main" }),
  );
const report = {
  schema: 1,
  passed: false,
  os: version(),
  logical_processors: cpus().length,
  cpu: cpus()[0].model,
  memory_gib: totalmem() / 2 ** 30,
  executable_sha256: createHash("sha256")
    .update(await readFile(executable))
    .digest("hex"),
  startup: [],
  accessibility: [],
  lifecycle: {},
};

try {
  for (let run = 0; run < runs; run++) {
    const profile = await mkdtemp(join(output, "profile-"));
    const port = await freePort();
    const start = performance.now();
    const child = spawn(executable, [], {
      windowsHide: true,
      stdio: "ignore",
      env: {
        ...process.env,
        WEBVIEW2_USER_DATA_FOLDER: profile,
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
      },
    });
    let spawnError;
    child.once("error", (error) => {
      spawnError = error;
    });
    let browser;
    try {
      const session = await connect(port, child);
      browser = session.browser;
      let page = session.page;
      if (spawnError) throw spawnError;
      const elapsed = performance.now() - start;
      report.connection_state = await page.getByRole("status").textContent();
      report.device_pixel_ratio = await page.evaluate(() => devicePixelRatio);
      if (process.argv.includes("--require-service"))
        await expect(page.getByRole("status")).toHaveText("Ready when you are");
      report.webview_version ??= browser.version();
      report.startup.push(elapsed);
      console.log(
        `Process-cold startup ${run + 1}/${runs}: ${Math.round(elapsed)} ms to settled service state, including CDP observation.`,
      );
      if (run !== runs - 1) continue;

      const trayNotice = () => page.getByRole("note", { name: "Closing and quitting Quad9" });
      await expect(trayNotice()).toBeVisible();

      await page.locator("body").click({ position: { x: 2, y: 2 } });
      await page.keyboard.press("Tab");
      await expect(
        page.getByRole("button", { name: "Connection", exact: true }),
      ).toBeFocused();
      await page.keyboard.press("Tab");
      await page.keyboard.press("Enter");
      await expect(
        page.getByRole("heading", { name: "Diagnostics", exact: true }),
      ).toBeVisible();

      for (const theme of ["Light", "Dark"]) {
        await page
          .getByRole("button", { name: "Settings", exact: true })
          .click();
        await page.getByRole("radio", { name: theme, exact: true }).check();
        for (const view of ["Connection", "Diagnostics", "Settings"]) {
          await page.getByRole("button", { name: view, exact: true }).click();
          const result = await new AxeBuilder({ page })
            .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
            .analyze();
          report.accessibility.push({
            theme,
            view,
            violations: result.violations,
          });
          expect(result.violations, `${theme} ${view}`).toEqual([]);
          if (view === "Connection")
            await page.screenshot({
              path: join(output, `native-connection-${theme.toLowerCase()}.png`),
              fullPage: true,
            });
        }
      }
      await page.emulateMedia({
        forcedColors: "active",
        reducedMotion: "reduce",
      });
      expect(
        await page.evaluate(
          () =>
            matchMedia("(forced-colors: active)").matches &&
            matchMedia("(prefers-reduced-motion: reduce)").matches,
        ),
      ).toBe(true);
      await page.screenshot({
        path: join(output, "native-high-contrast.png"),
        fullPage: true,
      });
      await page.emulateMedia({ forcedColors: "none" });
      await page.evaluate(() => {
        document.documentElement.style.zoom = "2";
      });
      for (const view of ["Connection", "Diagnostics", "Settings"]) {
        await page.getByRole("button", { name: view, exact: true }).click();
        expect(
          await page
            .locator("button,input,h1,h2,h3,legend:not(.sr-only)")
            .evaluateAll((elements) =>
              elements
                .filter((el) => {
                  const rect = el.getBoundingClientRect();
                  return (
                    rect.width > 0 &&
                    (rect.left < -1 ||
                      rect.right > innerWidth + 1 ||
                      el.scrollWidth > el.clientWidth + 1)
                  );
                })
                .map((el) => el.outerHTML),
            ),
        ).toEqual([]);
      }
      await page.screenshot({
        path: join(output, "native-200-percent-content.png"),
        fullPage: true,
      });
      await page.evaluate(() => {
        document.documentElement.style.zoom = "1";
      });
      const cdp = await page.context().newCDPSession(page);
      await writeFile(
        join(output, "native-accessibility-tree.json"),
        JSON.stringify(await cdp.send("Accessibility.getFullAXTree"), null, 2),
      );
      await page.screenshot({
        path: join(output, "native-settings-dark.png"),
        fullPage: true,
      });
      await trayNotice().getByRole("button", { name: "Got it", exact: true }).focus();
      await page.keyboard.press("Enter");
      await expect(trayNotice()).toHaveCount(0);
      await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeFocused();

      // Reloading preserves the native window but creates a new document. Its
      // first retention sequence must work, and the old document cannot release it.
      const oldDocument = await page.evaluate(async () => {
        const context = await window.__TAURI_INTERNALS__.invoke("interface_context");
        await window.__TAURI_INTERNALS__.invoke("retain_interface", {
          retain: false, sequence: 1000, documentId: context.document_id,
        });
        return context.document_id;
      });
      await page.reload();
      await expect(page.getByRole("status")).toHaveText(/Service unavailable|Ready when you are/);
      await expect(trayNotice()).toHaveCount(0);
      const currentDocument = await page.evaluate(async () => {
        const context = await window.__TAURI_INTERNALS__.invoke("interface_context");
        await window.__TAURI_INTERNALS__.invoke("retain_interface", {
          retain: true, sequence: 1, documentId: context.document_id,
        });
        return context.document_id;
      });
      expect(currentDocument).not.toBe(oldDocument);
      const staleAccepted = await page.evaluate(async (documentId) => {
        try {
          await window.__TAURI_INTERNALS__.invoke("retain_interface", {
            retain: false, sequence: 1001, documentId,
          });
          return true;
        } catch { return false; }
      }, oldDocument);
      expect(staleAccepted).toBe(false);
      report.document_reload_retention = true;
      // Busy/uncertain windows must keep their frontend receipt state on close.
      await page.evaluate(() =>
        window.__TAURI_INTERNALS__.invoke("retain_interface", { retain: true }),
      );
      // Async native workers can acquire their lock out of invocation order.
      // An older idle message must not release a newer busy retention.
      await page.evaluate(async () => {
        await window.__TAURI_INTERNALS__.invoke("retain_interface", {
          retain: true,
          sequence: 2,
        });
        await window.__TAURI_INTERNALS__.invoke("retain_interface", {
          retain: false,
          sequence: 1,
        });
      });
      await page.evaluate(() => {
        window.quad9RetentionMarker = "same-document";
      });
      // Observe Windows Tauri's fetch IPC through CDP. Its invoke property is
      // read-only. Never replace the bridge or collect its authentication headers.
      let readRequests = 0;
      let visibilityReplies = 0;
      const ipcCommand = (request) => {
        const url = new URL(request.url());
        return url.hostname === "ipc.localhost" ? decodeURIComponent(url.pathname.slice(1)) : null;
      };
      page.on("request", (request) => {
        if (ipcCommand(request) === "service_request" &&
            ["get_status", "list_adapters"].includes(request.postDataJSON()?.request?.command?.type))
          readRequests++;
      });
      page.on("requestfinished", (request) => {
        if (ipcCommand(request) === "plugin:window|is_visible") visibilityReplies++;
      });
      // Positive controls prevent an unavailable IPC observation path from
      // making the later zero-read assertion pass without observing anything.
      await page.evaluate(() => window.dispatchEvent(new Event("focus")));
      await expect.poll(() => readRequests).toBeGreaterThan(0);
      await expect.poll(() => visibilityReplies).toBeGreaterThan(0);
      await page.keyboard.press("Control+w");
      await delay(300);
      expect(page.isClosed()).toBe(false);
      await expect.poll(() => isVisible(page)).toBe(false);
      // Current Evergreen runtimes expose the hint. Assert the actual COM
      // property, since working-set reduction itself is only best effort.
      expect(await memoryTarget(page)).not.toBe(null);
      await expect.poll(async () => (await memoryTarget(page))?.level).toBe(1);
      expect((await memoryTarget(page)).visible).toBe(false);
      report.lifecycle.hidden_memory_target_low = true;
      const hiddenReads = { reads: readRequests, visibility: visibilityReplies };
      await page.evaluate(() => window.dispatchEvent(new Event("offline")));
      await expect.poll(() => visibilityReplies).toBeGreaterThan(hiddenReads.visibility);
      await delay(100);
      expect(readRequests).toBe(hiddenReads.reads);
      const observedBrowser = browser;
      try {
        // Keep the observer connected across Open, which can refresh before a
        // second CDP connection attaches to the retained document.
        ({ browser, page } = await reopen(executable, port, child));
        await expect.poll(() => readRequests).toBeGreaterThan(hiddenReads.reads);
      } finally {
        await observedBrowser.close().catch(() => {});
      }
      report.lifecycle.hidden_event_reads_deferred_until_reopen = true;
      await expect.poll(async () => {
        const target = await memoryTarget(page);
        return target && (!target.foreground || target.level === 0);
      }).toBe(true);
      const reopenedMemory = await memoryTarget(page);
      report.lifecycle.foreground_memory_target_normal = reopenedMemory.foreground
        ? reopenedMemory.level === 0 : null;
      // Playwright wrappers change across reconnect; the retained document keeps its runtime marker.
      expect(
        await page.evaluate(() => document.documentElement.dataset.theme),
      ).toBe("dark");
      expect(await page.evaluate(() => window.quad9RetentionMarker)).toBe(
        "same-document",
      );
      await page.evaluate(() =>
        window.__TAURI_INTERNALS__.invoke("retain_interface", {
          retain: false,
        }),
      );
      await closeWindow(page);
      expect(child.exitCode).toBe(null);
      await browser.close();
      ({ browser, page } = await reopen(executable, port, child));
      await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
      await expect(trayNotice()).toHaveCount(0);
      expect(
        await page.evaluate(() => window.quad9RetentionMarker),
      ).toBeUndefined();
      await expect(
        page.getByRole("heading", {
          name: /Service unavailable|Ready when you are/,
        }),
      ).toBeVisible();
      report.lifecycle = {
        ...report.lifecycle,
        retained_window_survived_close: true,
        stale_retention_release_ignored: true,
        idle_window_destroyed: true,
        single_instance_reopen: true,
        preferences_preserved: true,
        tray_notice_dismissal_preserved: true,
      };
      // Post the busy reservation and close without waiting for the first
      // response. A title-bar close must not overtake accepted retention.
      await page.evaluate(async () => {
        window.quad9RetentionMarker = "reservation-before-close";
        const retention = window.__TAURI_INTERNALS__.invoke("retain_interface", { retain: true });
        const close = window.__TAURI_INTERNALS__.invoke("plugin:window|close", { label: "main" });
        await Promise.all([retention, close]);
      });
      await delay(300);
      expect(page.isClosed()).toBe(false);
      await expect.poll(() => isVisible(page)).toBe(false);
      await browser.close();
      ({ browser, page } = await reopen(executable, port, child));
      expect(await page.evaluate(() => window.quad9RetentionMarker)).toBe("reservation-before-close");
      report.lifecycle.retention_recorded_before_close = true;
      // A later Open must win even if the close worker has not run yet.
      for (let cycle = 0; cycle < 6; cycle++) {
        await page.evaluate(() =>
          window.__TAURI_INTERNALS__.invoke("retain_interface", { retain: true }),
        );
        await page.keyboard.press("Control+w");
        const previous = browser;
        ({ browser, page } = await reopen(executable, port, child));
        await previous.close().catch(() => {});
        await expect.poll(() => isVisible(page)).toBe(true);
        await delay(150);
        expect(page.isClosed()).toBe(false);
        expect(await isVisible(page)).toBe(true);
      }
      report.lifecycle.immediate_close_reopen_cycles = 6;
      // Release a retained hidden document concurrently with a second launch.
      // Either the old document is shown or a replacement is built; the latest
      // Open action must leave a live, visible window in both orderings.
      for (let cycle = 0; cycle < 6; cycle++) {
        await page.evaluate(() =>
          window.__TAURI_INTERNALS__.invoke("retain_interface", {
            retain: true,
          }),
        );
        await page.keyboard.press("Control+w");
        await expect
          .poll(() => isVisible(page))
          .toBe(false);
        await page
          .evaluate(() => {
            void window.__TAURI_INTERNALS__
              .invoke("retain_interface", { retain: false })
              .catch(() => {});
          })
          .catch((error) => {
            if (!page.isClosed()) throw error;
          });
        const previous = browser;
        ({ browser, page } = await reopen(executable, port, child));
        await previous.close().catch(() => {});
        await expect.poll(() => isVisible(page)).toBe(true);
        await delay(150);
        expect(page.isClosed()).toBe(false);
        await expect(
          page.getByRole("heading", {
            name: /Service unavailable|Ready when you are/,
          }),
        ).toBeVisible();
      }
      report.lifecycle.concurrent_release_reopen_cycles = 6;
      // Reloading a retained document must also discard its obsolete lease.
      await page.evaluate(async () => {
        const context = await window.__TAURI_INTERNALS__.invoke("interface_context");
        await window.__TAURI_INTERNALS__.invoke("retain_interface", {
          retain: true, sequence: 5000, documentId: context.document_id,
        });
      });
      await page.reload();
      await expect(page.getByRole("status")).toHaveText(/Service unavailable|Ready when you are/);
      // Verify the rebuilt document can be closed again and the tray host survives.
      await closeWindow(page);
      report.lifecycle.reload_discards_old_retention = true;
      expect(child.exitCode).toBe(null);
      console.log(
        "Native accessibility and close/reopen checks passed; no DNS mutation was sent.",
      );
    } finally {
      await browser?.close().catch(() => {});
      child.kill();
      await delay(700);
    }
  }
  report.passed = true;
} finally {
  const sorted = [...report.startup].sort((a, b) => a - b);
  report.startup_p95_ms = sorted.length
    ? sorted[Math.ceil(sorted.length * 0.95) - 1]
    : null;
  report.startup_target_pass =
    sorted.length >= 20 ? report.startup_p95_ms <= 3000 : null;
  report.method =
    "Process-cold starts with a new WebView profile each run. OS caches and other running apps are unchanged. CDP readiness observation adds overhead. Resource measurements run separately without CDP or accessibility instrumentation.";
  await writeFile(
    join(output, "native-validation.json"),
    JSON.stringify(report, null, 2),
  );
}
