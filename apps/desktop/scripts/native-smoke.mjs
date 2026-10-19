// Exercise the embedded Windows frontend and its real Tauri -> Rust -> pipe
// failure path, without installing a service or changing DNS.
import { chromium } from "@playwright/test";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp } from "node:fs/promises";
import { createServer } from "node:net";
import { resolve, join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

if (process.platform !== "win32")
  throw new Error("The native smoke test requires Windows.");
if (!process.argv[2])
  throw new Error("Pass the built desktop executable path.");
const executable = resolve(process.argv[2]);
const output = resolve("../../output/playwright");
await mkdir(output, { recursive: true });
const profile = await mkdtemp(join(output, "native-profile-"));
const server = createServer();
await new Promise((ok, reject) => {
  server.once("error", reject);
  server.listen(0, "127.0.0.1", ok);
});
const port = server.address().port;
await new Promise((ok) => server.close(ok));
const child = spawn(executable, [], {
  windowsHide: true,
  stdio: ["ignore", "ignore", "pipe"],
  env: {
    ...process.env,
    WEBVIEW2_USER_DATA_FOLDER: profile,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  },
});
let stderr = "";
child.stderr.on("data", (bytes) => {
  stderr = (stderr + bytes.toString()).slice(-4096);
});
let processError;
child.on("error", (error) => {
  processError = error;
});
let browser;
try {
  const deadline = Date.now() + 30000;
  while (!browser && Date.now() < deadline) {
    if (processError) throw processError;
    if (child.exitCode !== null)
      throw new Error(`Native app exited ${child.exitCode}: ${stderr}`);
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {
        timeout: 1000,
      });
    } catch {
      await delay(300);
    }
  }
  if (!browser)
    throw new Error(
      `The native WebView did not start before its deadline. ${stderr}`,
    );
  let page;
  while (!page && Date.now() < deadline) {
    page = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((p) => /tauri[.:]/.test(p.url()));
    if (!page) await delay(200);
  }
  if (!page)
    throw new Error("The native app did not load its embedded frontend.");
  page.setDefaultTimeout(10000);
  await page
    .getByRole("heading", { name: "Service unavailable", exact: true })
    .waitFor();
  const details = await page.getByRole("alert").textContent();
  if (!details.includes("WaitNamedPipeW failed with Windows error"))
    throw new Error(`The real Rust pipe client was not reached: ${details}`);
  if (
    await page
      .getByRole("button", { name: "Enable Quad9", exact: true })
      .count()
  )
    throw new Error("Native unavailable state exposed Enable.");
  if (await page.getByText("INTERACTIVE PREVIEW", { exact: false }).count())
    throw new Error("Native app activated browser fixtures.");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: "Light", exact: true }).check();
  await page.getByRole("button", { name: "Connection", exact: true }).click();
  await page.screenshot({
    path: join(output, "native-service-unavailable.png"),
    fullPage: true,
  });
  console.log(
    "Native Windows smoke passed: embedded assets, real Tauri IPC, unavailable service, disabled Enable, theme selection, and screenshot. DNS was unchanged.",
  );
} finally {
  await browser?.close().catch(() => {});
  child.kill();
}
