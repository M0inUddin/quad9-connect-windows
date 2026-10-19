// Counters and close-to-tray validation for this app's process tree only.
import { expect } from "@playwright/test";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import {
  freePort,
  connect,
  closeWindow,
  reopen,
  memoryTarget,
} from "./native-session.mjs";

if (process.platform !== "win32" || !process.argv[2])
  throw new Error("Pass a release Windows desktop executable.");
const executable = resolve(process.argv[2]);
const output = resolve(
  process.argv[3] ?? "../../output/playwright/native-resources",
);
const desktopOnly = process.argv.includes("--desktop-only");
const shortRun = process.argv.includes("--counter-smoke");
const verifyPwsh = process.argv.includes("--verify-pwsh");
const serviceUnavailable = process.argv.includes("--service-unavailable");
const windowOnly = process.argv.includes("--window-only");
await mkdir(output, { recursive: true });
const profile = await mkdtemp(join(output, "profile-"));
const port = await freePort();
const execute = promisify(execFile);
const summary = {
  schema: 1,
  functional_checks_passed: false,
  complete_process_set: !desktopOnly,
  executable_sha256: createHash("sha256")
    .update(await readFile(executable))
    .digest("hex"),
  reports: {},
  memory_targets: {},
  phases_requested: windowOnly
    ? ["window"]
    : ["window", "retained-hidden", "tray"],
  method:
    "Isolated app profile; recursive process attribution; debugging listener enabled for native close control, with no CDP client connected during counter sampling. No axe or screen reader is active. Windows settings are unchanged.",
};
const jobName = `Local\\Quad9Resources-${randomUUID()}`;
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
    jobName,
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
const child = await new Promise((ok, reject) => {
  let errors = "";
  const lines = createInterface({ input: launcher.stdout });
  const timer = setTimeout(() => {
    launcher.stdin.end();
    reject(new Error("Accounting host launch timed out."));
  }, 30000);
  launcher.stderr.on("data", (part) => {
    errors += part;
  });
  launcher.once("error", reject);
  launcher.once("exit", (code) => {
    clearTimeout(timer);
    reject(new Error(`Accounting host exited ${code}: ${errors}`));
  });
  lines.once("line", (line) => {
    clearTimeout(timer);
    lines.close();
    try {
      const { pid } = JSON.parse(line);
      if (!Number.isSafeInteger(pid) || pid <= 0)
        throw new Error("Invalid accounting host PID.");
      ok({
        pid,
        get exitCode() {
          return launcher.exitCode;
        },
        kill() {
          launcher.stdin.end();
        },
      });
    } catch (error) {
      launcher.stdin.end();
      reject(error);
    }
  });
});
let browser;
async function sample(
  mode,
  shell = "powershell.exe",
  smoke = shortRun,
  phase = mode,
) {
  const file = join(output, `${phase}-${shell}.json`);
  const args = [
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    resolve("../../scripts/validation/Measure-Quad9Resources.ps1"),
    "-Mode",
    mode,
    "-HostProcessId",
    String(child.pid),
    "-CpuJobName",
    jobName,
    "-OutputPath",
    file,
  ];
  if (desktopOnly) args.push("-DesktopOnly");
  if (phase === "window") args.push("-RequireForeground");
  if (smoke) args.push("-DurationSeconds", "2", "-SettleSeconds", "0");
  const result = await execute(shell, args, {
    windowsHide: true,
    timeout: 370000,
  });
  console.log(result.stdout.trim());
  const report = JSON.parse(await readFile(file, "utf8"));
  expect(report.complete_process_set).toBe(!desktopOnly);
  if (desktopOnly) expect(report.cpu_measurement_complete).toBe(true);
  expect(report.desktop_job_processes_at_start).toBeGreaterThan(0);
  expect(report.desktop_job_processes_at_end).toBeGreaterThanOrEqual(
    report.desktop_job_processes_at_start,
  );
  if (smoke || desktopOnly) {
    expect(report.cpu_target_pass).toBe(null);
    expect(report.memory_target_pass).toBe(null);
  }
  expect(report.measured_seconds).toBeGreaterThanOrEqual(smoke ? 2 : 300);
  if (mode === "window")
    expect(report.samples.every((s) => s.webviews > 0)).toBe(true);
  if (mode === "tray")
    expect(report.samples.every((s) => s.webviews === 0)).toBe(true);
  summary.reports[`${phase}-${shell}`] = report;
}
try {
  let initial = await connect(port, child);
  browser = initial.browser;
  summary.webview_version = browser.version();
  const page = initial.page;
  summary.connection_state = await page.getByRole("status").textContent();
  if (!desktopOnly || serviceUnavailable)
    await expect(page.getByRole("status")).toHaveText(
      serviceUnavailable ? "Service unavailable" : "Ready when you are",
    );
  // Exercise the native Open path, which requests foreground activation.
  // The sampler independently records whether Windows granted it throughout.
  await browser.close();
  initial = await reopen(executable, port, child);
  browser = initial.browser;
  summary.memory_targets.window = await memoryTarget(initial.page);
  await browser.close();
  browser = undefined;
  await sample("window");
  if (verifyPwsh) await sample("window", "pwsh.exe", true);
  if (!windowOnly) {
    let session = await connect(port, child);
    browser = session.browser;
    // A busy/uncertain window remains alive when closed. Measure this inactive
    // view separately from the normal tray state, which has no WebView at all.
    await session.page.evaluate(() =>
      window.__TAURI_INTERNALS__.invoke("retain_interface", { retain: true }),
    );
    await session.page.keyboard.press("Control+w");
    await expect
      .poll(() =>
        session.page.evaluate(() =>
          window.__TAURI_INTERNALS__.invoke("plugin:window|is_visible", {
            label: "main",
          }),
        ),
      )
      .toBe(false);
    expect(session.page.isClosed()).toBe(false);
    expect(await memoryTarget(session.page)).not.toBe(null);
    await expect
      .poll(async () => (await memoryTarget(session.page))?.level)
      .toBe(1);
    summary.memory_targets.hidden = await memoryTarget(session.page);
    expect(summary.memory_targets.hidden.visible).toBe(false);
    await browser.close();
    browser = undefined;
    await sample("window", "powershell.exe", shortRun, "retained-hidden");
    if (verifyPwsh) await sample("window", "pwsh.exe", true, "retained-hidden");
    session = await reopen(executable, port, child);
    browser = session.browser;
    await expect
      .poll(async () => {
        const target = await memoryTarget(session.page);
        return target && (!target.foreground || target.level === 0);
      })
      .toBe(true);
    summary.memory_targets.reopened = await memoryTarget(session.page);
    await session.page
      .getByRole("button", { name: "Settings", exact: true })
      .click();
    await session.page
      .getByRole("radio", { name: "Dark", exact: true })
      .check();
    await session.page.evaluate(() =>
      window.__TAURI_INTERNALS__.invoke("retain_interface", { retain: false }),
    );
    summary.inactive_window_retained_and_reopened = true;
    await closeWindow(session.page);
    await browser.close();
    browser = undefined;
    expect(child.exitCode).toBe(null);
    await delay(1000);
    await sample("tray");
    if (verifyPwsh) await sample("tray", "pwsh.exe", true);
  }
  summary.functional_checks_passed = true;
} finally {
  await browser?.close().catch(() => {});
  child.kill();
  await delay(700);
  await writeFile(
    join(output, "resource-validation.json"),
    JSON.stringify(summary, null, 2),
  );
}
