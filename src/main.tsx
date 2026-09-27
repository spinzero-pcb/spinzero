import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { SetupWindow } from "./ui/SetupWindow";
import { ErrorBoundary } from "./ui/ErrorBoundary";
import { installGlobalErrorHandlers } from "./lib/crashReporter";
import "./styles/app.css";

// Catch errors that escape React (event handlers, async tasks, rejected
// promises) before the first render, so nothing slips through unlogged.
installGlobalErrorHandlers();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      {/* `SpinZero --setup <dir>` opens this window alone. See src-tauri/src/setupwin.rs. */}
      {new URLSearchParams(window.location.search).has("setup") ? <SetupWindow /> : <App />}
    </ErrorBoundary>
  </React.StrictMode>,
);
