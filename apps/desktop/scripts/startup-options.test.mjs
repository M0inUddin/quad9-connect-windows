// SPDX-License-Identifier: AGPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { startupOptions } from "./startup-options.mjs";

test("service flag preserves the ignored default output directory", () => {
  for (const args of [
    ["app.exe", "5", "--require-service"],
    ["--require-service", "app.exe", "5"],
    ["app.exe", "--require-service"],
  ]) {
    assert.deepEqual(startupOptions(args), {
      executable: "app.exe",
      runs: 5,
      output: "../../output/playwright/native-startup",
      requireService: true,
    });
  }
});
test("explicit output and absent service flag retain their meaning", () => {
  assert.equal(
    startupOptions([
      "app.exe",
      "20",
      "../../artifacts/run",
      "--require-service",
    ]).output,
    "../../artifacts/run",
  );
  assert.equal(startupOptions(["app.exe"]).requireService, false);
});
test("invalid options and positional arguments fail before creating files", () => {
  for (const args of [
    [],
    ["app.exe", "--unknown"],
    ["app.exe", "0"],
    ["app.exe", "101"],
    ["app.exe", "1.5"],
    ["app.exe", "5", "out", "extra"],
  ])
    assert.throws(() => startupOptions(args));
});
