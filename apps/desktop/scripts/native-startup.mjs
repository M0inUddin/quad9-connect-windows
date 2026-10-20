// SPDX-License-Identifier: AGPL-3.0-or-later
// Read-only phase diagnostic. The owned Windows job cleans up every descendant.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { version, cpus, totalmem } from "node:os";
import { expect } from "@playwright/test";
import { freePort, connect } from "./native-session.mjs";
import { startupOptions } from "./startup-options.mjs";

if (process.platform !== "win32" || !process.argv[2])
  throw new Error("Pass a built Windows desktop executable.");
const options = startupOptions(process.argv.slice(2));
const executable = resolve(options.executable);
const runs = options.runs;
const output = resolve(options.output);
await mkdir(output, { recursive: true });
const report = {
  schema: 1,
  passed: false,
  method:
    "Fresh WebView profiles; normal OS caches and other apps. Native process creation to Node CDP observation of settled state, excluding the PowerShell launcher. Native phases use Instant relative to main entry. Document marks use Performance.timeOrigin, which is not renderer process creation. Instrumented phase diagnostic; no release qualification.",
  os: version(),
  logical_processors: cpus().length,
  memory_gib: totalmem() / 2 ** 30,
  executable_sha256: createHash("sha256")
    .update(await readFile(executable))
    .digest("hex"),
  require_service: options.requireService,
  startup_target_pass: null,
  runs: [],
};
try {
  for (let run = 1; run <= runs; run++) {
    const profile = await mkdtemp(join(output, "profile-"));
    const port = await freePort();
    const launcher = spawn(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        resolve("../../scripts/validation/Start-Quad9ResourceHost.ps1"),
        "-Executable",
        executable,
        "-JobName",
        `Local\\Quad9Startup-${randomUUID()}`,
      ],
      {
        windowsHide: true,
        stdio: ["pipe", "pipe", "pipe"],
        env: {
          ...process.env,
          WEBVIEW2_USER_DATA_FOLDER: profile,
          WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
        },
      },
    );
    let browser;
    const exited = new Promise((resolve) => launcher.once("close", resolve));
    try {
      const ready = await new Promise((ok, reject) => {
        let errors = "";
        const lines = createInterface({ input: launcher.stdout });
        const timer = setTimeout(
          () => reject(new Error("Accounting host launch timed out.")),
          30000,
        );
        const failed = (error) => {
          clearTimeout(timer);
          lines.close();
          reject(error);
        };
        launcher.once("error", failed);
        launcher.once("exit", (code) =>
          failed(new Error(`Accounting host exited ${code}: ${errors}`)),
        );
        launcher.stderr.on("data", (part) => {
          errors = (errors + part).slice(-4096);
        });
        lines.once("line", (line) => {
          clearTimeout(timer);
          lines.close();
          try {
            const value = JSON.parse(line);
            if (
              !Number.isSafeInteger(value.pid) ||
              value.pid <= 0 ||
              !Number.isSafeInteger(value.started_unix_ms)
            )
              throw new Error("Invalid accounting host identity/timestamp.");
            ok(value);
          } catch (error) {
            reject(error);
          }
        });
      });
      const session = await connect(port, {
        get exitCode() {
          return launcher.exitCode;
        },
      });
      browser = session.browser;
      const observedEpoch = performance.timeOrigin + performance.now();
      const page = session.page;
      const status = await page.getByRole("status").innerText();
      if (report.require_service) expect(status).toBe("Ready when you are");
      await expect
        .poll(() =>
          page.evaluate(
            () =>
              performance.getEntriesByName("quad9-startup-settled", "mark")
                .length,
          ),
        )
        .toBe(1);
      const native = await page.evaluate(() =>
        window.__TAURI_INTERNALS__.invoke("startup_timeline"),
      );
      const document = await page.evaluate(() => ({
        time_origin_unix_ms: performance.timeOrigin,
        marks_ms: Object.fromEntries(
          performance
            .getEntriesByType("mark")
            .filter((e) => e.name.startsWith("quad9-startup-"))
            .map((e) => [e.name, e.startTime]),
        ),
        navigation: performance
          .getEntriesByType("navigation")
          .map((e) => e.toJSON()),
        paint: performance.getEntriesByType("paint").map((e) => e.toJSON()),
      }));
      const phases = native.phases_ms;
      for (const name of [
        "main_entered",
        "builder_ready",
        "setup_entered",
        "tray_ready",
        "monitor_ready",
        "setup_finished",
        "run_ready",
        "interface_context",
        "status_started",
        "status_finished",
      ])
        expect(phases[name], name).toBeGreaterThanOrEqual(0);
      expect(phases.setup_entered).toBeGreaterThanOrEqual(phases.builder_ready);
      expect(phases.monitor_ready).toBeGreaterThanOrEqual(phases.tray_ready);
      expect(phases.status_started).toBeGreaterThanOrEqual(
        phases.monitor_ready,
      );
      expect(phases.status_finished).toBeGreaterThanOrEqual(
        phases.status_started,
      );
      expect(native.main_entered_unix_ms).toBeGreaterThanOrEqual(
        ready.started_unix_ms,
      );
      const entry = {
        run,
        observed_ms: observedEpoch - ready.started_unix_ms,
        main_after_creation_ms:
          native.main_entered_unix_ms - ready.started_unix_ms,
        document_origin_after_creation_ms:
          document.time_origin_unix_ms - ready.started_unix_ms,
        status,
        browser_version: browser.version(),
        native,
        document,
      };
      report.runs.push(entry);
      console.log(
        JSON.stringify({
          run,
          observed_ms: entry.observed_ms,
          native_ms: phases,
          document_marks_ms: document.marks_ms,
        }),
      );
    } finally {
      await browser?.close().catch(() => {});
      launcher.stdin.end();
      await exited;
    }
  }
  report.passed = true;
} finally {
  await writeFile(
    join(output, "native-startup.json"),
    JSON.stringify(report, null, 2),
  );
}
