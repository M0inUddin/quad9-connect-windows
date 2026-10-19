import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createTransport } from "./transport";
import "./style.css";

createTransport().then(({ send, preview }) =>
  createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App send={send} preview={preview} />
    </React.StrictMode>,
  ),
);
