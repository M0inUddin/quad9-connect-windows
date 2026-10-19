# Desktop UI screenshots

These screenshots come from the running React interface in a local Edge browser at 480 logical pixels wide. Playwright captures settled states after the corresponding interaction. The preview uses sample adapter/service data, displays a visible preview label, and does not change DNS. The production Tauri app uses the real service; browser preview fixtures cannot activate in a production build.

## Connection, light theme

![Connection screen in light theme](connection-light.png)

## Configured, dark theme

![Matching Quad9 configuration in dark theme](configured-dark.png)

## Settings, dark theme

![Explicit adapter selection and themes](settings-dark.png)

## Diagnostics, dark theme

![One system-resolver diagnostic result](diagnostics-dark.png)

To regenerate, run `npm run build` and `npm run test:e2e` in `apps/desktop`. Generated output stays under the ignored `output/playwright/` directory. These four reviewed captures are deliberately tracked for PR review.
