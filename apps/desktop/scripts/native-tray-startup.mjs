// SPDX-License-Identifier: AGPL-3.0-or-later
// Read-only native lifecycle check, isolated profile, and owned process job.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { version } from "node:os";
import { expect } from "@playwright/test";
import {
  freePort,
  reopen,
  closeWindow,
  launchSecond,
} from "./native-session.mjs";

if (process.platform !== "win32" || !process.argv[2])
  throw new Error(
    "Pass a built Windows desktop executable. Run with the service absent.",
  );
const executable = resolve(process.argv[2]);
const output = resolve(
  process.argv[3] ?? "../../output/playwright/native-tray-startup",
);
const installedStartup = process.argv.includes("--installed-startup");
if (
  installedStartup &&
  (process.env.GITHUB_ACTIONS !== "true" ||
    process.env.RUNNER_ENVIRONMENT !== "github-hosted")
)
  throw new Error(
    "Changing actual startup preferences in this harness requires the disposable hosted installer VM.",
  );
await mkdir(output, { recursive: true });
const profile = await mkdtemp(join(output, "profile-"));
const runtimeAssignedPort =
  installedStartup || process.argv.includes("--runtime-debug-port");
const requestedPort = runtimeAssignedPort ? 0 : await freePort();
const report = {
  schema: 1,
  passed: false,
  os: version(),
  installed_startup_test: installedStartup,
  executable_sha256: createHash("sha256")
    .update(await readFile(executable))
    .digest("hex"),
  method:
    "Functional tray launch, cumulative owned-job process accounting, repeated tray and invalid launches, then normal second-launch activation through the production single-instance callback. No DNS mutation. Optional startup-preference writes are restricted to the disposable hosted installer VM. Not a resource-budget or startup-speed qualification.",
};
// Recreated browsers may publish a different port. Resolve it for each attach.
const activePortFile = join(profile, "EBWebView", "DevToolsActivePort");
const port = runtimeAssignedPort
  ? async () => {
      const value = Number(
        (await readFile(activePortFile, "utf8")).split(/\r?\n/)[0],
      );
      if (!Number.isInteger(value) || value < 1 || value > 65535)
        throw new Error("WebView2 has not published a valid debugging port.");
      return value;
    }
  : requestedPort;
const launcher = spawn(
  "powershell.exe",
  [
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    resolve("../../scripts/validation/Start-Quad9TrayTestHost.ps1"),
    "-Executable",
    executable,
    "-JobName",
    `Local\\Quad9TrayStartup-${randomUUID()}`,
  ],
  {
    windowsHide: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: {
      ...process.env,
      WEBVIEW2_USER_DATA_FOLDER: profile,
      QUAD9_TRAY_DIAGNOSTICS: join(output, "native-processes.json"),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${requestedPort} --remote-debugging-address=127.0.0.1 --enable-logging --v=1 --log-file="${join(output, "webview.log")}"`,
    },
  },
);
const exited = new Promise((ok) => launcher.once("close", ok));
let browser;
try {
  const ready = await new Promise((ok, reject) => {
    let errors = "";
    const lines = createInterface({ input: launcher.stdout });
    const timer = setTimeout(
      () => failed(new Error("Tray startup check timed out.")),
      45000,
    );
    const failed = (error) => {
      clearTimeout(timer);
      lines.close();
      reject(error);
    };
    launcher.once("error", failed);
    launcher.once("exit", (code) =>
      failed(new Error(`Tray test host exited ${code}: ${errors}`)),
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
          !Number.isSafeInteger(value.tray_checked_unix_ms)
        )
          throw new Error("Invalid tray test host identity/timestamp.");
        ok(value);
      } catch (error) {
        reject(error);
      }
    });
  });
  report.hidden_launch = ready;
  const session = await reopen(executable, port, {
    get exitCode() {
      return launcher.exitCode;
    },
  });
  browser = session.browser;
  const page = session.page;
  await expect(page.getByRole("status")).toHaveText(
    installedStartup ? "Ready when you are" : "Service unavailable",
  );
  if (!installedStartup) {
    await expect(
      page.getByRole("button", { name: "Enable Quad9" }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Reconnect to service", exact: true }),
    ).toBeVisible();
  }
  report.probe_negative_control = await new Promise((ok, reject) => {
    const lines = createInterface({ input: launcher.stdout });
    const timer = setTimeout(() => {
      lines.close();
      reject(new Error("Open-window probe timed out."));
    }, 10000);
    lines.once("line", (line) => {
      clearTimeout(timer);
      lines.close();
      try {
        ok(JSON.parse(line));
      } catch (error) {
        reject(error);
      }
    });
    launcher.stdin.write("check-open\n");
  });
  expect(report.probe_negative_control).toEqual({
    application_window_detected: true,
    child_processes_detected: true,
  });
  const native = await page.evaluate(async () => ({
    timeline: await window.__TAURI_INTERNALS__.invoke("startup_timeline"),
    monitor_error: await window.__TAURI_INTERNALS__.invoke(
      "network_monitor_error",
    ),
  }));
  expect(native.monitor_error).toBeNull();
  for (const name of [
    "tray_ready",
    "monitor_ready",
    "run_ready",
    "status_started",
    "status_finished",
  ])
    expect(native.timeline.phases_ms[name], name).not.toBeNull();
  // The read completed while there was no document to request it. On opening,
  // the frontend still performs a fresh read through the same coordinator.
  expect(
    native.timeline.main_entered_unix_ms +
      native.timeline.phases_ms.status_finished,
  ).toBeLessThanOrEqual(ready.tray_checked_unix_ms);
  expect(native.timeline.phases_ms.navigation_started).toBeGreaterThan(
    native.timeline.phases_ms.status_finished,
  );
  report.native = native;
  await expect(
    page.getByRole("note", { name: "Closing and quitting Quad9" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: "Dark", exact: true }).check();
  if (installedStartup) {
    const group = page.getByRole("group", {
      name: "Start Quad9 when signing in to Windows",
    });
    await expect(
      group.getByRole("radio", { name: "Off", exact: true }),
    ).toBeChecked({ timeout: 20000 });
    for (const mode of ["Tray", "Open window", "Off", "Tray"]) {
      await group.getByRole("radio", { name: mode, exact: true }).check();
      if (mode === "Tray" && !report.pending_save_navigation_checked) {
        await page
          .getByRole("button", { name: "Connection", exact: true })
          .click();
        await page
          .getByRole("button", { name: "Settings", exact: true })
          .click();
        report.pending_save_navigation_checked = true;
      }
      await expect(
        group.getByRole("radio", { name: mode, exact: true }),
      ).toBeEnabled();
      await expect(
        group.getByRole("radio", { name: mode, exact: true }),
      ).toBeChecked();
    }
    await page.screenshot({
      path: join(output, "native-windows-startup.png"),
      fullPage: true,
    });
    report.native_startup_preference_flow = true;
  }
  await launchSecond(executable, ["--start-in-tray"]);
  await expect(
    page.getByRole("heading", { name: "Settings", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("radio", { name: "Dark", exact: true }),
  ).toBeChecked();
  report.repeated_tray_launch_preserved_visible_settings = true;
  await closeWindow(page);
  const reopened = await reopen(executable, port, {
    get exitCode() {
      return launcher.exitCode;
    },
  });
  browser = reopened.browser;
  await reopened.page
    .getByRole("button", { name: "Settings", exact: true })
    .click();
  await expect(
    reopened.page.getByRole("radio", { name: "Dark", exact: true }),
  ).toBeChecked();
  if (installedStartup)
    await expect(
      reopened.page
        .getByRole("group", { name: "Start Quad9 when signing in to Windows" })
        .getByRole("radio", { name: "Tray", exact: true }),
    ).toBeChecked();
  report.open_close_reopen_preserved_preferences = true;
  report.passed = true;
  console.log(
    "Tray startup passed: no initial WebView, read-only status, repeated tray launch, normal activation, and close/reopen.",
  );
} finally {
  report.runtime_debugging_port = await readFile(activePortFile, "utf8").catch(
    () => null,
  );
  await browser?.close().catch(() => {});
  launcher.stdin.end();
  await exited;
  await writeFile(
    join(output, "native-tray-startup.json"),
    JSON.stringify(report, null, 2),
  );
}
