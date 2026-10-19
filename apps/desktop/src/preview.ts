import type { Adapter, Status, Transport, Result, Response } from "./protocol";
const id = "a956d6e1-8910-4d20-9a67-5e34915c2a10";
export function previewTransport(): Transport {
  const receipts = new Map<string, Response>();
  let loseResponse = true;
  let diagnosticCount = 0;
  const mode = new URLSearchParams(location.search).get("preview");
  let status: Status = {
    type: "status",
    revision: 0,
    recovery_blocked: mode === "recovery",
    observed_matches: [
      "configured",
      "slow-probe",
      "failed-probe",
      "upstream-unavailable",
    ].includes(mode ?? ""),
    journal: [
      "configured",
      "recovery",
      "slow-probe",
      "failed-probe",
      "upstream-unavailable",
    ].includes(mode ?? "")
      ? {
          phase: mode === "recovery" ? "recovery_required" : "committed",
          targets: [
            { target: { adapter_id: id, family: "ipv4" }, phase: "applied" },
          ],
        }
      : null,
  };
  const adapters: Adapter[] = [
    {
      id,
      name: "Ethernet",
      candidate: true,
      exclusion_reason: null,
      ipv4_enabled: true,
      ipv6_enabled: true,
      ipv6_usable: false,
      has_ipv4_dns: true,
      has_ipv6_dns: false,
    },
    {
      id: "6a256b6b-1520-4124-9b79-66ece6592095",
      name: "Wi-Fi",
      candidate: false,
      exclusion_reason: "Adapter is disconnected",
      ipv4_enabled: true,
      ipv6_enabled: true,
      ipv6_usable: false,
      has_ipv4_dns: false,
      has_ipv6_dns: false,
    },
    {
      id: "94ab1355-47d6-4f27-9dab-e211af887a51",
      name: "vEthernet (WSL)",
      candidate: false,
      exclusion_reason: "Virtual adapter is excluded",
      ipv4_enabled: true,
      ipv6_enabled: true,
      ipv6_usable: false,
      has_ipv4_dns: false,
      has_ipv6_dns: false,
    },
  ];
  return async (request) => {
    window.dispatchEvent(
      new CustomEvent("quad9-preview-request", { detail: request }),
    );
    await new Promise((resolve) =>
      setTimeout(
        resolve,
        mode === "slow-probe" && request.command.type === "run_diagnostics"
          ? 1000
          : 140,
      ),
    );
    if (mode === "unavailable")
      throw new Error(
        "The Quad9 service is not installed or could not be reached.",
      );
    const command = request.command;
    const cached = receipts.get(request.id);
    if (cached) return cached;
    let result: Result = status;
    switch (command.type) {
      case "get_status":
        result = { ...status };
        break;
      case "list_adapters":
        result = { type: "adapters", adapters };
        break;
      case "run_diagnostics":
        if (mode === "failed-probe" && ++diagnosticCount > 1) {
          result = {
            type: "error",
            code: "diagnostics_failed",
            message: "The resolver check timed out.",
          };
          break;
        }
        result = {
          type: "diagnostics",
          report: {
            outcome: status.observed_matches
              ? "doh_observed"
              : "other_transport_observed",
            elapsed_ms: 42,
            checked_at_unix_seconds: Math.floor(Date.now() / 1000),
            scope: "One fresh system-resolver lookup of proto.on.quad9.net.",
            direct_upstream:
              mode === "legacy-probe"
                ? undefined
                : {
                    elapsed_ms: 85,
                    endpoints: [
                      {
                        address: "9.9.9.9",
                        outcome:
                          mode === "upstream-unavailable"
                            ? "timed_out"
                            : "https_response",
                        http_status:
                          mode === "upstream-unavailable"
                            ? null
                            : mode === "upstream-http-error"
                              ? 503
                              : 405,
                        windows_error: null,
                      },
                      {
                        address: "2620:fe::fe",
                        outcome: "failed",
                        http_status: null,
                        windows_error: 12029,
                      },
                    ],
                    scope:
                      "Fixed Quad9 HTTPS endpoints only; no system resolver verification.",
                  },
          },
        };
        break;
      case "enable_quad9":
        status = {
          ...status,
          revision: status.revision + 1,
          recovery_blocked: false,
          observed_matches: true,
          journal: {
            phase: "committed",
            targets: command.targets.map((target) => ({
              target,
              phase: "applied",
            })),
          },
        };
        result = {
          type: "enabled",
          revision: status.revision,
          already_enabled: false,
        };
        break;
      case "restore_previous_settings":
        status = {
          ...status,
          revision: status.revision + 1,
          recovery_blocked: false,
          observed_matches: false,
          journal: { phase: "restored", targets: [] },
        };
        result = {
          type: "restored",
          revision: status.revision,
          complete: true,
          issues: [],
        };
    }
    const response: Response = { version: 1, id: request.id, result };
    if (
      command.type === "enable_quad9" ||
      command.type === "restore_previous_settings"
    )
      receipts.set(request.id, response);
    if (
      mode === "lost-response" &&
      command.type === "enable_quad9" &&
      loseResponse
    ) {
      loseResponse = false;
      throw new Error("The service response was lost after applying settings.");
    }
    return response;
  };
}
