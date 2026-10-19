export type Family = "ipv4" | "ipv6";
export type Target = { adapter_id: string; family: Family };
export type Adapter = {
  id: string;
  name: string;
  candidate: boolean;
  exclusion_reason: string | null;
  ipv4_enabled: boolean;
  ipv6_enabled: boolean;
  ipv6_usable: boolean;
  has_ipv4_dns: boolean;
  has_ipv6_dns: boolean;
};
export type Phase =
  | "prepared"
  | "applying"
  | "verifying"
  | "committed"
  | "restoring"
  | "restored"
  | "recovery_required";
export type Status = {
  type: "status";
  revision: number;
  recovery_blocked: boolean;
  observed_matches: boolean;
  journal: null | {
    phase: Phase;
    targets: { target: Target; phase: string }[];
  };
};
export type Command =
  | { type: "get_status" | "list_adapters" | "run_diagnostics" }
  | { type: "enable_quad9"; expected_revision: number; targets: Target[] }
  | { type: "restore_previous_settings"; expected_revision: number };
export type Request = { version: 1; id: string; command: Command };
export type Probe = {
  outcome: string;
  elapsed_ms: number;
  checked_at_unix_seconds: number;
  scope: string;
  [key: string]: unknown;
};
export type Result =
  | Status
  | { type: "adapters"; adapters: Adapter[] }
  | { type: "diagnostics"; report: Probe }
  | { type: "enabled"; revision: number; already_enabled: boolean }
  | { type: "restored"; revision: number; complete: boolean; issues: string[] }
  | { type: "error"; code: string; message: string };
export type Response = { version: 1; id: string; result: Result };
export type Transport = (request: Request) => Promise<Response>;

export function makeRequest(command: Command): Request {
  return { version: 1, id: crypto.randomUUID(), command };
}
export function key(target: Target) {
  return `${target.adapter_id}:${target.family}`;
}

export function eligibleTargets(
  targets: Target[],
  adapters: Adapter[],
): Target[] {
  const seen = new Set<string>();
  return targets.filter((target) => {
    const adapter = adapters.find((a) => a.id === target.adapter_id);
    const eligible =
      adapter?.candidate &&
      (target.family === "ipv4"
        ? adapter.ipv4_enabled
        : adapter.ipv6_enabled && adapter.ipv6_usable);
    if (!eligible || seen.has(key(target))) return false;
    seen.add(key(target));
    return true;
  });
}
export function connectionState(
  status: Status | null,
): "unavailable" | "recovery" | "conflict" | "configured" | "off" {
  if (!status) return "unavailable";
  if (
    status.recovery_blocked ||
    (status.journal &&
      !["committed", "restored"].includes(status.journal.phase))
  )
    return "recovery";
  if (status.journal?.phase === "committed")
    return status.observed_matches ? "configured" : "conflict";
  return "off";
}

export function loadPreferences(storage: Pick<Storage, "getItem">) {
  try {
    const value = JSON.parse(storage.getItem("quad9-preferences-v1") ?? "{}");
    const theme: "system" | "light" | "dark" = ["light", "dark"].includes(
      value.theme,
    )
      ? value.theme
      : "system";
    const targets: Target[] = Array.isArray(value.targets)
      ? value.targets
          .filter(
            (t: Target) =>
              typeof t?.adapter_id === "string" &&
              /^[0-9a-f-]{36}$/i.test(t.adapter_id) &&
              ["ipv4", "ipv6"].includes(t.family),
          )
          .slice(0, 32)
      : [];
    return { theme, targets };
  } catch {
    return { theme: "system" as const, targets: [] as Target[] };
  }
}
