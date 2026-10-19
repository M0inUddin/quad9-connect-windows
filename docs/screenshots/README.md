# Desktop UI screenshots

Native captures show the packaged Tauri app with embedded assets and real Rust IPC on Windows 11 Pro, build 10.0.26300. Browser captures show labeled sample states in Edge at 480 logical pixels wide. Playwright captures settled states after the corresponding interaction. Browser preview fixtures cannot activate in a production build.

## Native release client, authorized service

The real installer enrolled a temporary standard local account. A fresh token launched this packaged release client alongside the LocalSystem service, with DNS management off. It reached the authenticated "Ready when you are" state. The [combined resource record](../validation/windows11-26300-combined-resources.json) identifies the candidate and confirms safe cleanup with the original DNS baseline unchanged. This capture predates direct-upstream diagnostics.

![Native client with an authorized service and DNS management off](native-authorized-service-off.png)

## Native release client, service absent

These release-build captures use a fresh application profile and the standard-user token after the temporary service test completed and cleaned up. The client displays its real unavailable-service state. The application changes its own theme. Build hashes, display scaling, test scope, and cleanup results are recorded in the [Windows 11 validation record](../validation/windows11-26300-performance-accessibility.json).

![Native connection screen in the light theme](native-connection-light.png)

![Native connection screen in the dark theme](native-connection-dark.png)

## Earlier native smoke capture

This earlier native smoke capture used an absent service. The client displays its actual unavailable state, prevents Enable, and offers reconnect.

![Actual Tauri client with unavailable service](native-service-unavailable.png)

## Browser fixture, connection in the light theme

![Connection screen in light theme](connection-light.png)

## Browser fixture, configured in the dark theme

![Matching Quad9 configuration in dark theme](configured-dark.png)

## Browser fixture, settings in the dark theme

![Explicit adapter selection and themes](settings-dark.png)

## Browser fixture, diagnostics in the dark theme

![One system-resolver diagnostic result](diagnostics-dark.png)

To regenerate browser captures, run `npm run build` and `npm run test:e2e` in `apps/desktop`. Run `node scripts/native-validation.mjs <release-desktop.exe> 20 <output-directory>` unelevated for the native captures. Add `--require-service` only when the service is installed, the current token is authorized, and DNS management is off. `native-smoke.mjs` generates the earlier unavailable-service capture when the service is absent. Generated profiles and raw validation output stay under ignored directories. Only selected screenshots and sanitized validation records are tracked.
