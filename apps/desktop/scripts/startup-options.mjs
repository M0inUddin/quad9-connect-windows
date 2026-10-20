// SPDX-License-Identifier: AGPL-3.0-or-later
import { parseArgs } from "node:util";

export function startupOptions(args) {
  const { values, positionals } = parseArgs({
    args,
    allowPositionals: true,
    options: { "require-service": { type: "boolean", default: false } },
  });
  if (positionals.length < 1 || positionals.length > 3)
    throw new Error(
      "Pass executable, optional run count, and optional output directory.",
    );
  const [
    executable,
    count = "5",
    output = "../../output/playwright/native-startup",
  ] = positionals;
  const runs = Number(count);
  if (!Number.isInteger(runs) || runs < 1 || runs > 100)
    throw new Error("Run count must be between 1 and 100.");
  return {
    executable,
    runs,
    output,
    requireService: values["require-service"],
  };
}
