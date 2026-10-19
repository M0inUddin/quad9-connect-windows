import { chromium } from "@playwright/test";
import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { setTimeout as delay } from "node:timers/promises";

export async function freePort() {
  const server = createServer();
  await new Promise((ok, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", ok);
  });
  const port = server.address().port;
  await new Promise((ok) => server.close(ok));
  return port;
}
export async function connect(port, child) {
  const deadline = Date.now() + 30000;
  let browser;
  while (Date.now() < deadline) {
    if (child.exitCode !== null)
      throw new Error(
        `Desktop process exited ${child.exitCode}. Close any other Quad9 interface before testing.`,
      );
    try {
      browser ??= await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {
        timeout: 500,
      });
      const page = browser
        .contexts()
        .flatMap((c) => c.pages())
        .find((p) => /tauri[.:]/.test(p.url()));
      if (page) {
        page.setDefaultTimeout(10000);
        await page
          .getByRole("status")
          .filter({ hasText: /Service unavailable|Ready when you are/ })
          .waitFor({ state: "attached" });
        return { browser, page };
      }
    } catch {
      await browser?.close().catch(() => {});
      browser = undefined;
    }
    await delay(50);
  }
  throw new Error(
    "Native startup exceeded 30 seconds. Run the UI harness unelevated; elevated WebView2 hosts ignore diagnostic environment flags.",
  );
}
export async function reopen(executable, port, child) {
  const second = spawn(executable, [], { windowsHide: true, stdio: "ignore" });
  await new Promise((ok, reject) => {
    const timer = setTimeout(() => {
      second.kill();
      reject(
        new Error("Second launch did not attach to the existing tray host."),
      );
    }, 10000);
    second.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    second.once("exit", (code) => {
      clearTimeout(timer);
      code === 0 ? ok() : reject(new Error(`Second launch exited ${code}`));
    });
  });
  return connect(port, child);
}
export async function closeWindow(page) {
  const closed = page.waitForEvent("close", { timeout: 10000 });
  await page.keyboard.press("Control+w").catch((error) => {
    if (!page.isClosed()) throw error;
  });
  await closed;
}
