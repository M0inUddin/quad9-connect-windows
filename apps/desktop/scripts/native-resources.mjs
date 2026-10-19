// Counters and close-to-tray validation for this app's process tree only.
import { expect } from "@playwright/test";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { createHash } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { freePort, connect, closeWindow } from "./native-session.mjs";

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
  method:
    "Isolated app profile; recursive process attribution; debugging listener enabled for native close control, with no CDP client connected during counter sampling. No axe or screen reader is active. Windows settings are unchanged.",
};
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
async function sample(mode, shell = "powershell.exe", smoke = shortRun) {
  const file = join(output, `${mode}-${shell}.json`);
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
    "-OutputPath",
    file,
  ];
  if (desktopOnly) args.push("-DesktopOnly");
  if (smoke) args.push("-DurationSeconds", "2", "-SettleSeconds", "0");
  const result = await execute(shell, args, {
    windowsHide: true,
    timeout: 370000,
  });
  console.log(result.stdout.trim());
  const report = JSON.parse(await readFile(file, "utf8"));
  expect(report.complete_process_set).toBe(!desktopOnly);
  if (smoke || desktopOnly) {
    expect(report.cpu_target_pass).toBe(null);
    expect(report.memory_target_pass).toBe(null);
  }
  expect(report.measured_seconds).toBeGreaterThanOrEqual(smoke ? 2 : 300);
  if (mode === "window")
    expect(report.samples.every((s) => s.webviews > 0)).toBe(true);
  if (mode === "tray")
    expect(report.samples.every((s) => s.webviews === 0)).toBe(true);
  summary.reports[`${mode}-${shell}`] = report;
}
try {
  ({ browser } = await connect(port, child));
  if (spawnError) throw spawnError;
  summary.webview_version = browser.version();
  const page = browser
    .contexts()
    .flatMap((c) => c.pages())
    .find((p) => /tauri[.:]/.test(p.url()));
  summary.connection_state = await page.getByRole("status").textContent();
  if (!desktopOnly || serviceUnavailable)
    await expect(page.getByRole("status")).toHaveText(
      serviceUnavailable ? "Service unavailable" : "Ready when you are",
    );
  await browser.close();
  browser = undefined;
  await sample("window");
  if (verifyPwsh) await sample("window", "pwsh.exe", true);
  const session = await connect(port, child);
  browser = session.browser;
  await closeWindow(session.page);
  await browser.close();
  browser = undefined;
  expect(child.exitCode).toBe(null);
  await delay(1000);
  await sample("tray");
  if (verifyPwsh) await sample("tray", "pwsh.exe", true);
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
