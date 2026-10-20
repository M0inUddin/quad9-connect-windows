// SPDX-License-Identifier: AGPL-3.0-or-later
// Each fixed label records its first occurrence in this document. No timers,
// native calls, persistence, or network requests are needed for these marks.
export function markStartup(
  phase:
    "module" | "transport" | "effect" | "listeners" | "context" | "settled",
) {
  const name = `quad9-startup-${phase}`;
  if (!performance.getEntriesByName(name, "mark").length)
    performance.mark(name);
}
