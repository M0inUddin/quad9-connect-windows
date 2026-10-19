import { invoke, isTauri } from "@tauri-apps/api/core";
import type { Response, Transport } from "./protocol";

export async function createTransport(): Promise<{
  send: Transport;
  preview: boolean;
}> {
  if (isTauri())
    return {
      send: (request) => invoke<Response>("service_request", { request }),
      preview: false,
    };
  // Fixtures are a development-only module. Production browser content never
  // fabricates a service result, even when a preview query parameter is present.
  if (
    import.meta.env.DEV &&
    new URLSearchParams(location.search).has("preview")
  ) {
    const { previewTransport } = await import("./preview");
    return { send: previewTransport(), preview: true };
  }
  return {
    send: async () => {
      throw new Error(
        "Open the installed Windows app to connect to the Quad9 service.",
      );
    },
    preview: false,
  };
}
