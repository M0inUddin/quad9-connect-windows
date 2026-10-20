import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  connectionState,
  eligibleTargets,
  key,
  loadPreferences,
  makeRequest,
} from "./protocol";
import type {
  Adapter,
  Command,
  Probe,
  Request,
  Response,
  Status,
  Target,
  Transport,
} from "./protocol";

type View = "connection" | "diagnostics" | "settings";
type MonitorFailure = { generation: number; message: string };
type TrayResult = { Ok: Response } | { Err: string };
type InterfaceContext = {
  view: View;
  document_id: number;
  diagnostic_pending: boolean;
  tray_result: TrayResult | null;
};
const copy = {
  off: [
    "Ready when you are",
    "Enable encrypted Quad9 DNS on the adapters you select. Your previous settings will be saved for restoration.",
  ],
  configured: [
    "Quad9 configured",
    "Your selected adapters match the saved Quad9 configuration. Run a diagnostic to check the current resolver path.",
  ],
  conflict: [
    "Settings changed elsewhere",
    "An adapter no longer matches the settings Quad9 wrote. Restore preserves changes made by other software.",
  ],
  recovery: [
    "Recovery required",
    "Some settings still need attention. Retry restoration when the affected adapters are available.",
  ],
  unavailable: [
    "Service unavailable",
    "Connect to the installed Quad9 service to read the current DNS state.",
  ],
} as const;

function Icon({
  name,
  size = 20,
}: {
  name:
    | "power"
    | "shield"
    | "network"
    | "settings"
    | "diagnostics"
    | "arrow"
    | "check"
    | "alert";
  size?: number;
}) {
  const paths = {
    power: "M12 2v10M5 5a9 9 0 1 0 14 0",
    shield: "M12 2 3 6v6c0 5 9 10 9 10s9-5 9-10V6ZM8 12l3 3 5-6",
    network: "M8 3h8v6H8ZM3 16h6v5H3Zm12 0h6v5h-6ZM12 9v4M6 16v-3h12v3",
    settings: "M4 6h16M4 12h16M4 18h16M8 3v6M16 9v6M10 15v6",
    diagnostics: "M3 12h4l3-8 4 16 3-8h4",
    arrow: "m9 5 7 7-7 7",
    check: "m5 12 4 4L19 6",
    alert: "M12 3 2 21h20ZM12 9v5M12 17v1",
  };
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={paths[name]} />
    </svg>
  );
}

export function App({ send, preview }: { send: Transport; preview: boolean }) {
  const [view, setView] = useState<View>("connection");
  const [status, setStatus] = useState<Status | null>(null);
  const [adapters, setAdapters] = useState<Adapter[]>([]);
  const [prefs, setPrefs] = useState(() => loadPreferences(localStorage));
  const [busy, setBusy] = useState<string | null>("Reading service state");
  const [error, setError] = useState<string | null>(null);
  const [monitorUnavailable, setMonitorUnavailable] = useState(false);
  const [probe, setProbe] = useState<Probe | null>(null);
  const lock = useRef(false);
  const actionActive = useRef(false);
  const initializing = useRef(true);
  const diagnosticQueued = useRef(false);
  const retentionSequence = useRef(0);
  const documentId = useRef<number | null>(null);
  const nativeListenersReady = useRef<Promise<void>>(Promise.resolve());
  const handshakeReady = useRef(false);
  const navigationPending = useRef(false);
  const retry = useRef<Request | null>(null);
  const observationEpoch = useRef(0);
  const nativeGeneration = useRef(0);
  const refreshPending = useRef(false);
  const state = connectionState(status);
  const directSummary = probe?.direct_upstream
    ? probe.direct_upstream.endpoints.some(
        (endpoint) => endpoint.outcome === "busy",
      )
      ? ". Direct Quad9 HTTPS: previous check is still finishing."
      : `. Direct Quad9 HTTPS: ${probe.direct_upstream.endpoints.filter((endpoint) => endpoint.outcome === "https_response").length} of ${probe.direct_upstream.endpoints.length} endpoints replied.`
    : "";
  const owned =
    state === "configured" || state === "conflict" || state === "recovery";
  const activeTargets = owned
    ? (status?.journal?.targets.map((t) => t.target) ?? [])
    : prefs.targets;
  const selectedNames = [
    ...new Set(
      activeTargets.map(
        (t) =>
          adapters.find((a) => a.id === t.adapter_id)?.name ??
          "Unavailable adapter",
      ),
    ),
  ];

  const refresh = useCallback(async () => {
    const epoch = observationEpoch.current;
    const response = await send(makeRequest({ type: "get_status" }));
    if (epoch !== observationEpoch.current) return;
    if (response.result.type !== "status")
      throw new Error(
        response.result.type === "error"
          ? response.result.message
          : "Unexpected service status response.",
      );
    const list = await send(makeRequest({ type: "list_adapters" }));
    if (epoch !== observationEpoch.current) return;
    setStatus(response.result);
    if (list.result.type === "adapters") {
      const current = list.result.adapters;
      setAdapters(current);
      // Remove old choices only after a successful, authoritative adapter read.
      // Active restoration scope always comes from the service journal above.
      setPrefs((p) => {
        const targets = eligibleTargets(p.targets, current);
        return targets.length === p.targets.length ? p : { ...p, targets };
      });
    } else if (list.result.type === "error")
      throw new Error(list.result.message);
  }, [send]);

  const invalidate = useCallback(() => {
    const epoch = ++observationEpoch.current;
    setProbe(null);
    // Startup owns its reads until a complete status/adapter pair survives
    // invalidation. Focus and network bursts must not start competing reads.
    if (initializing.current) return;
    if (lock.current) {
      refreshPending.current = true;
      return;
    }
    const refreshVisible = (visible: boolean) => {
      // Native hide does not reliably update WebView2's document.hidden. Keep
      // invalidating evidence while a retained window is hidden, but defer its
      // redundant service reads until it reopens. A newer event or action can
      // overtake the asynchronous native visibility response.
      if (!visible || epoch !== observationEpoch.current) return;
      if (lock.current) refreshPending.current = true;
      else
        void refresh().catch(() => {
          if (epoch === observationEpoch.current) setStatus(null);
        });
    };
    if (isTauri())
      void getCurrentWindow()
        .isVisible()
        .then(refreshVisible, () => refreshVisible(true));
    else refreshVisible(!document.hidden);
  }, [refresh]);
  useEffect(() => {
    try {
      localStorage.setItem("quad9-preferences-v1", JSON.stringify(prefs));
    } catch {
      /* Preferences can remain session-only. */
    }
    const media = matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme =
        prefs.theme === "system"
          ? media.matches
            ? "dark"
            : "light"
          : prefs.theme;
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [prefs]);
  useEffect(() => {
    if (!isTauri()) return;
    handshakeReady.current = false;
    const unlisten = listen<string>("navigate", (event) => {
      if (["connection", "diagnostics", "settings"].includes(event.payload)) {
        setView(event.payload as View);
        // The initial new-document handshake resets the previous document's
        // reservation. Leave native pending work untouched until that reset.
        if (!handshakeReady.current) {
          navigationPending.current = true;
          return;
        }
        // Showing an already focused retained window need not deliver another
        // browser focus event. Read fresh state on every native reopen.
        invalidate();
        // Consume the saved tray receipt in an existing window too, so it
        // cannot reappear as a stale error after a later window recreation.
        void invoke<InterfaceContext>("interface_context")
          .then((context) => {
            if (context?.tray_result) acceptTrayResult(context.tray_result);
            if (context?.diagnostic_pending) requestTrayDiagnostic();
          })
          .catch((error) => setError(String(error)));
      }
    });
    const changed = listen("service-changed", invalidate);
    const network = listen<number>("network-changed", (event) => {
      nativeGeneration.current = Math.max(
        nativeGeneration.current,
        event.payload,
      );
      invalidate();
    });
    const reportMonitorError = (failure?: MonitorFailure) => {
      if (failure)
        nativeGeneration.current = Math.max(
          nativeGeneration.current,
          failure.generation,
        );
      setMonitorUnavailable(true);
      invalidate();
    };
    const monitorError = listen<MonitorFailure>(
      "network-monitor-error",
      (event) => reportMonitorError(event.payload),
    );
    // Registration may fail before the webview has installed its listeners.
    invoke<MonitorFailure | null>("network_monitor_error")
      .then((error) => {
        if (error) reportMonitorError(error);
      })
      .catch(() => reportMonitorError());
    const diagnostic = listen<{ generation: number; result: TrayResult }>(
      "diagnostic-result",
      (event) => {
        if (event.payload.generation < nativeGeneration.current) return;
        nativeGeneration.current = event.payload.generation;
        const result = event.payload.result;
        if ("Err" in result) setError(result.Err);
        else if (result.Ok.result.type === "diagnostics")
          setProbe(result.Ok.result.report);
        else if (result.Ok.result.type === "error")
          setError(result.Ok.result.message);
      },
    );
    const tray = listen<TrayResult>("tray-result", (event) =>
      acceptTrayResult(event.payload),
    );
    nativeListenersReady.current = Promise.all([
      unlisten,
      changed,
      network,
      monitorError,
      diagnostic,
      tray,
    ]).then(() => {});
    const close = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.key.toLowerCase() === "w") {
        event.preventDefault();
        void getCurrentWindow().close();
      }
    };
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("keydown", close);
      [unlisten, changed, network, monitorError, diagnostic, tray].forEach(
        (p) => p.then((fn) => fn()),
      );
    };
  }, [invalidate]);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        if (isTauri()) {
          await nativeListenersReady.current;
          if (!alive) return;
          const context = await invoke<InterfaceContext>("interface_context", {
            newDocument: true,
          });
          if (context) {
            documentId.current = context.document_id;
            if (!navigationPending.current) setView(context.view);
            if (context.tray_result) acceptTrayResult(context.tray_result);
            if (context.diagnostic_pending) diagnosticQueued.current = true;
            handshakeReady.current = true;
            if (navigationPending.current) {
              navigationPending.current = false;
              const pending =
                await invoke<InterfaceContext>("interface_context");
              if (pending?.tray_result) acceptTrayResult(pending.tray_result);
              if (pending?.diagnostic_pending) diagnosticQueued.current = true;
            }
          }
        }
        for (let attempt = 0; alive && attempt < 3; attempt++) {
          if (attempt) {
            await new Promise((resolve) => setTimeout(resolve, attempt * 150));
            if (!alive) return;
          }
          const epoch = observationEpoch.current;
          try {
            await refresh();
          } catch (error) {
            // A network event can also obsolete a failed connection attempt.
            // Only a failure from the current observation should end startup.
            if (epoch === observationEpoch.current) throw error;
          }
          // Return enters finally synchronously, so no promise callback can
          // invalidate this read before normal event handling takes over.
          if (epoch === observationEpoch.current) return;
        }
        if (alive)
          throw new Error(
            "The network keeps changing. Wait for it to settle, then reconnect.",
          );
      } catch (e) {
        if (alive) {
          setStatus(null);
          setError(String(e instanceof Error ? e.message : e));
        }
      } finally {
        if (alive) {
          initializing.current = false;
          if (diagnosticQueued.current) requestTrayDiagnostic();
          else setBusy(null);
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, [refresh]);

  function acceptTrayResult(result: TrayResult) {
    if ("Err" in result) setError(result.Err);
    else if (result.Ok.result.type === "error")
      setError(result.Ok.result.message);
    else if (result.Ok.result.type === "restored" && !result.Ok.result.complete)
      setError(
        `Restoration is incomplete. ${result.Ok.result.issues.join(" ")}`,
      );
  }
  useEffect(() => {
    document.addEventListener("visibilitychange", invalidate);
    window.addEventListener("focus", invalidate);
    window.addEventListener("online", invalidate);
    window.addEventListener("offline", invalidate);
    return () => {
      document.removeEventListener("visibilitychange", invalidate);
      window.removeEventListener("focus", invalidate);
      window.removeEventListener("online", invalidate);
      window.removeEventListener("offline", invalidate);
    };
  }, [invalidate]);

  function requestTrayDiagnostic() {
    diagnosticQueued.current = true;
    if (initializing.current || actionActive.current) return;
    diagnosticQueued.current = false;
    void perform({ type: "run_diagnostics" });
  }

  async function perform(command?: Command) {
    if (actionActive.current) return;
    actionActive.current = true;
    lock.current = true;
    setError(null);
    const epoch = observationEpoch.current;
    // Only the explicit Retry action may replay an uncertain mutation. A
    // read-only action must send its own command and leave that receipt pending.
    const request = command ? makeRequest(command) : retry.current;
    if (request?.command.type === "run_diagnostics") setProbe(null);
    setBusy(
      request?.command.type === "run_diagnostics"
        ? "Checking DNS and Quad9 reachability"
        : request?.command.type === "enable_quad9"
          ? "Saving settings and enabling Quad9"
          : request?.command.type === "restore_previous_settings"
            ? "Restoring previous DNS settings"
            : "Connecting to service",
    );
    try {
      // A closed busy window keeps its response and exact retry receipt alive.
      if (isTauri())
        await invoke("retain_interface", {
          retain: true,
          sequence: ++retentionSequence.current,
          documentId: documentId.current,
        });
      if (request) {
        const mutation =
          request.command.type === "enable_quad9" ||
          request.command.type === "restore_previous_settings";
        if (mutation) {
          retry.current = request;
          setProbe(null);
        }
        const response = await send(request);
        if (response.version !== 1 || response.id !== request.id)
          throw new Error(
            "The service response could not be matched to this request.",
          );
        if (retry.current?.id === request.id) retry.current = null;
        if (response.result.type === "error")
          throw new Error(
            `${response.result.message} (${response.result.code})`,
          );
        if (
          response.result.type === "diagnostics" &&
          epoch === observationEpoch.current
        )
          setProbe(response.result.report);
        if (response.result.type === "restored" && !response.result.complete)
          setError(
            `Restoration is incomplete. ${response.result.issues.join(" ")}`,
          );
      }
      await refresh();
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
      // Preserve the exact uncertain mutation for an explicit retry. Never build
      // a new Enable request after a lost response.
      setStatus(null);
    } finally {
      lock.current = false;
      if (refreshPending.current) {
        refreshPending.current = false;
        // An event may have invalidated the final post-action read. Unlock first
        // so any event during this replacement read can schedule its own read.
        try {
          await refresh();
        } catch {
          setStatus(null);
        }
      }
      actionActive.current = false;
      if (diagnosticQueued.current) {
        // Coalesce tray requests and keep the retained document across the
        // handoff, including the final replacement read of the earlier action.
        if (!initializing.current) requestTrayDiagnostic();
      } else {
        setBusy(null);
        if (isTauri() && !retry.current)
          void invoke("retain_interface", {
            retain: false,
            sequence: ++retentionSequence.current,
            documentId: documentId.current,
          }).catch(() => {});
      }
    }
  }

  function toggle(target: Target) {
    setPrefs((p) => ({
      ...p,
      targets: p.targets.some((t) => key(t) === key(target))
        ? p.targets.filter((t) => key(t) !== key(target))
        : [...p.targets, target],
    }));
  }
  const primary = () =>
    retry.current
      ? perform()
      : state === "unavailable"
        ? perform()
        : owned
          ? perform({
              type: "restore_previous_settings",
              expected_revision: status!.revision,
            })
          : perform({
              type: "enable_quad9",
              expected_revision: status!.revision,
              targets: prefs.targets,
            });

  return (
    <div className="app-shell">
      {preview && (
        <div className="preview-note">
          INTERACTIVE PREVIEW <span>Sample state · DNS is unchanged</span>
        </div>
      )}
      <header className="app-header">
        <img src="/quad9-icon.svg" alt="Quad9" width="37" height="37" />
        <div>
          <h1>
            Quad9 <span>Connect</span>
          </h1>
          <p>FOR WINDOWS</p>
        </div>
        <span className="version">
          0.1 <span>DEV</span>
        </span>
      </header>
      <nav aria-label="Main navigation">
        {(["connection", "diagnostics", "settings"] as View[]).map((item) => (
          <button
            key={item}
            aria-current={view === item ? "page" : undefined}
            onClick={() => setView(item)}
          >
            <Icon name={item === "connection" ? "shield" : item} />
            {item[0].toUpperCase() + item.slice(1)}
          </button>
        ))}
      </nav>
      <main id="main-content">
        {monitorUnavailable && (
          <div className="error-box" role="alert">
            <Icon name="alert" />
            <div>
              <strong>Automatic DNS checks are unavailable</strong>
              <p>
                Quit and reopen the app to restart monitoring. Manual checks and
                restoration remain available.
              </p>
            </div>
          </div>
        )}
        {error && (
          <div className="error-box" role="alert">
            <Icon name="alert" />
            <div>
              <strong>Action needs attention</strong>
              <p>{error}</p>
              {retry.current && (
                <p>
                  The result is uncertain. Retry the same request to retrieve
                  its outcome.
                </p>
              )}
            </div>
          </div>
        )}
        {view === "connection" && (
          <>
            <section
              className={`connection-hero ${state}`}
              aria-labelledby="state-heading"
            >
              <div className="status-symbol">
                <Icon
                  name={
                    state === "configured"
                      ? "shield"
                      : state === "recovery" || state === "conflict"
                        ? "alert"
                        : "power"
                  }
                  size={35}
                />
              </div>
              <span className="eyebrow">
                {state === "off"
                  ? "DNS MANAGEMENT IS OFF"
                  : state === "configured"
                    ? "CONFIGURATION MATCHES"
                    : state === "unavailable"
                      ? "CONNECTION NEEDED"
                      : "REVIEW REQUIRED"}
              </span>
              <h2 id="state-heading">{copy[state][0]}</h2>
              <p>{copy[state][1]}</p>
            </section>
            <section
              className="profile-panel"
              aria-labelledby="profile-heading"
            >
              <div className="panel-title">
                <Icon name="shield" />
                <h3 id="profile-heading">Quad9 with threat blocking</h3>
                <span className="pill">DoH</span>
              </div>
              <div className="profile-details">
                <span>
                  Primary DNS<strong>9.9.9.9</strong>
                </span>
                <span>
                  Secondary DNS<strong>149.112.112.112</strong>
                </span>
              </div>
              <div className="profile-foot">
                <span>
                  <Icon name="check" size={14} />
                  Encryption required
                </span>
                <span>No plaintext fallback</span>
              </div>
            </section>
            <button className="scope-row" onClick={() => setView("settings")}>
              <Icon name="network" />
              <span>
                <strong>
                  {selectedNames.length
                    ? selectedNames.join(", ")
                    : "Choose your adapters"}
                </strong>
                <small>
                  {activeTargets.length
                    ? `${activeTargets.length} selected address ${activeTargets.length === 1 ? "family" : "families"}`
                    : "Select a physical Wi-Fi or Ethernet adapter"}
                </small>
              </span>
              <Icon name="arrow" size={17} />
            </button>
            <div className="verification-row">
              <span
                className={`tiny-dot ${probe?.outcome === "doh_observed" ? "good" : ""}`}
              />
              <span>
                {probe
                  ? probe.outcome === "doh_observed"
                    ? "DoH observed in the last diagnostic"
                    : "The last diagnostic did not observe DoH"
                  : "No current diagnostic result"}
              </span>
              <button
                onClick={() => {
                  setView("diagnostics");
                }}
                aria-label="Open diagnostics"
              >
                <Icon name="arrow" size={15} />
              </button>
            </div>
            <button
              className={`primary-button ${owned ? "restore-button" : ""}`}
              disabled={
                !!busy || (state === "off" && prefs.targets.length === 0)
              }
              onClick={primary}
            >
              <Icon name={owned ? "power" : "shield"} size={19} />
              {busy ??
                (retry.current
                  ? "Retry last request"
                  : state === "unavailable"
                    ? "Reconnect to service"
                    : owned
                      ? "Turn off and restore DNS"
                      : "Enable Quad9")}
            </button>
            <p className="action-note">
              {owned
                ? "Restore the settings saved before Quad9 was enabled."
                : "Your previous DNS settings stay available to restore."}
            </p>
          </>
        )}
        {view === "diagnostics" && (
          <section className="secondary-view">
            <span className="eyebrow">CHECK THE CURRENT PATH</span>
            <h2>Diagnostics</h2>
            <p className="intro">
              Check the Windows system resolver and direct Quad9 reachability.
            </p>
            <div className="check-list">
              <div>
                <Icon name={status ? "check" : "alert"} />
                <span>
                  <strong>Windows service</strong>
                  <small>
                    {status
                      ? `Connected · revision ${status.revision}`
                      : "Unavailable"}
                  </small>
                </span>
              </div>
              <div>
                <Icon name={state === "configured" ? "check" : "network"} />
                <span>
                  <strong>Selected adapter settings</strong>
                  <small>
                    {state === "configured"
                      ? "Match the saved Quad9 configuration"
                      : state === "off"
                        ? "Quad9 is not managing DNS"
                        : copy[state][0]}
                  </small>
                </span>
              </div>
              <div>
                <Icon
                  name={
                    probe?.outcome === "doh_observed" ? "check" : "diagnostics"
                  }
                />
                <span>
                  <strong>System resolver transport</strong>
                  <small>
                    {probe
                      ? probe.outcome === "doh_observed"
                        ? "DNS over HTTPS observed"
                        : `Could not verify DoH · ${probe.outcome.replaceAll("_", " ")}`
                      : "Ready to check"}
                  </small>
                </span>
                {probe && <span className="timing">{probe.elapsed_ms} ms</span>}
              </div>
              <div>
                <Icon name="network" />
                <span>
                  <strong>Direct Quad9 HTTPS</strong>
                  {probe?.direct_upstream ? (
                    probe.direct_upstream.endpoints.map((endpoint) => (
                      <small key={endpoint.address}>
                        {endpoint.address}:{" "}
                        {endpoint.outcome === "https_response"
                          ? `HTTPS response · HTTP ${endpoint.http_status}`
                          : endpoint.outcome === "timed_out"
                            ? "Timed out"
                            : endpoint.outcome === "busy"
                              ? "Previous check is still finishing; retry shortly"
                              : "No HTTPS response"}
                      </small>
                    ))
                  ) : (
                    <small>
                      {probe
                        ? "Not provided by this service version"
                        : "Not checked"}
                    </small>
                  )}
                </span>
                {probe?.direct_upstream && (
                  <span className="timing">
                    {probe.direct_upstream.elapsed_ms} ms
                  </span>
                )}
              </div>
            </div>
            {probe && (
              <p className="checked-at">
                Checked{" "}
                {new Date(
                  probe.checked_at_unix_seconds * 1000,
                ).toLocaleTimeString()}{" "}
                · One lookup only
              </p>
            )}
            <aside className="context-note">
              This check queries <code>proto.on.quad9.net</code>. It does not
              establish how every application resolves DNS. Browser DNS and VPNs
              may use another path. The separate HTTPS check contacts Quad9
              directly. A response shows that the endpoint replied; it does not
              verify a DNS answer or Windows DNS protection.
            </aside>
            <button
              className="primary-button"
              disabled={!!busy || !status}
              onClick={() => perform({ type: "run_diagnostics" })}
            >
              <Icon name="diagnostics" />
              {busy ?? "Run diagnostics"}
            </button>
          </section>
        )}
        {view === "settings" && (
          <section className="secondary-view">
            <span className="eyebrow">MAKE IT YOURS</span>
            <h2>Settings</h2>
            <h3 className="section-label">Appearance</h3>
            <fieldset className="theme-picker">
              <legend className="sr-only">Color theme</legend>
              {(["system", "light", "dark"] as const).map((theme) => (
                <label key={theme}>
                  <input
                    type="radio"
                    name="theme"
                    value={theme}
                    checked={prefs.theme === theme}
                    onChange={() => setPrefs((p) => ({ ...p, theme }))}
                  />
                  <span>{theme[0].toUpperCase() + theme.slice(1)}</span>
                </label>
              ))}
            </fieldset>
            <div className="settings-section-title">
              <h3 className="section-label">Network adapters</h3>
              <span>Explicit selection</span>
            </div>
            <p className="field-note">
              Select the address families Quad9 should manage. New adapters are
              never enrolled automatically.
            </p>
            <div className="adapter-list">
              {adapters.length === 0 && (
                <p className="field-note">
                  Connect to the service to read adapters.
                </p>
              )}
              {adapters.map((adapter) => (
                <fieldset
                  className={`adapter ${!adapter.candidate ? "excluded" : ""}`}
                  key={adapter.id}
                >
                  <legend className="adapter-heading">
                    <Icon name="network" />
                    <strong>{adapter.name}</strong>
                    <span>{adapter.candidate ? "Available" : "Excluded"}</span>
                  </legend>
                  {adapter.candidate ? (
                    <>
                      <div className="family-choices">
                        {(["ipv4", "ipv6"] as const).map((family) => (
                          <label key={family}>
                            <input
                              type="checkbox"
                              disabled={
                                !!busy ||
                                owned ||
                                !adapter[
                                  family === "ipv4"
                                    ? "ipv4_enabled"
                                    : "ipv6_usable"
                                ]
                              }
                              checked={prefs.targets.some(
                                (t) =>
                                  t.adapter_id === adapter.id &&
                                  t.family === family,
                              )}
                              onChange={() =>
                                toggle({ adapter_id: adapter.id, family })
                              }
                            />
                            {family === "ipv4" ? "IPv4" : "IPv6"}
                          </label>
                        ))}
                      </div>
                      {!adapter.ipv6_usable && (
                        <small>
                          IPv6 has no usable global address and gateway.
                        </small>
                      )}
                    </>
                  ) : (
                    <small>{adapter.exclusion_reason}</small>
                  )}
                </fieldset>
              ))}
            </div>
            {owned && (
              <p className="field-note">
                Restore the active configuration before changing its scope.
              </p>
            )}
            <aside className="context-note">
              Closing the window keeps the tray and service running. Quit
              interface leaves committed DNS settings active. Use restore to
              turn off DNS management.
            </aside>
          </section>
        )}
      </main>
      <footer>
        <span className="footer-dot" />
        {preview
          ? "Preview service"
          : status
            ? "Local Windows service"
            : "Waiting for Windows service"}
        <span className="footer-right">Independent community client</span>
      </footer>
      <div className="sr-only" role="status" aria-live="polite">
        {busy ??
          (probe
            ? `Diagnostic complete. ${probe.outcome === "doh_observed" ? "DNS over HTTPS observed" : `Could not verify DoH. ${probe.outcome.replaceAll("_", " ")}`}${directSummary}`
            : copy[state][0])}
      </div>
    </div>
  );
}
