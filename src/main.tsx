import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./styles/app.css";
import "./theme";

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

if (!import.meta.env.DEV) {
  // A released app must not behave like a web page: no reload, no zoom and
  // no browser context menu outside text fields.
  window.addEventListener(
    "keydown",
    (e) => {
      const reload = e.metaKey && e.key.toLowerCase() === "r";
      const zoom = e.metaKey && ["=", "+", "-", "0"].includes(e.key);
      if (reload || zoom) e.preventDefault();
    },
    true,
  );
  window.addEventListener("contextmenu", (e) => {
    if (!(e.target as Element).closest("input, textarea, [contenteditable]")) e.preventDefault();
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
