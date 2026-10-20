import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createTransport } from "./transport";
import "./style.css";
import { markStartup } from "./startup";

markStartup("module");
createTransport().then(({ send, preview }) => {
  markStartup("transport");
  return createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App send={send} preview={preview} />
    </React.StrictMode>,
  );
});
