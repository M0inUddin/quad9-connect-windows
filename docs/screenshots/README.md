# Desktop UI screenshots

The first capture shows the actual packaged Tauri app using real Rust IPC on Windows build 10.0.26300.0. The remaining captures show the running React interface in a local Edge browser at 480 logical pixels wide. Playwright captures settled states after the corresponding interaction. Browser previews use sample adapter/service data, display a visible preview label, and do not change DNS. The production Tauri app uses the real service; browser preview fixtures cannot activate in a production build.

## Native Windows client, service absent

The service is deliberately absent on this host. The client displays its actual unavailable state, prevents Enable, and offers reconnect. This checks native startup and IPC without installing the privileged service or changing host DNS.

![Actual Tauri client with unavailable service](native-service-unavailable.png)

## Connection, light theme

![Connection screen in light theme](connection-light.png)

## Configured, dark theme

![Matching Quad9 configuration in dark theme](configured-dark.png)

## Settings, dark theme

![Explicit adapter selection and themes](settings-dark.png)

## Diagnostics, dark theme

![One system-resolver diagnostic result](diagnostics-dark.png)

To regenerate, run `npm run build` and `npm run test:e2e` in `apps/desktop`. Generated output stays under the ignored `output/playwright/` directory. These four reviewed captures are deliberately tracked for PR review.
