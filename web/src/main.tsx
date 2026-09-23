import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { isMock } from "./api";
import { exchangeTokenFromUrl, watchForTokens } from "./auth";
import { registerServiceWorker } from "./lib/push";
import "./styles.css";

async function boot() {
  const exchangeError = isMock ? null : await exchangeTokenFromUrl();
  if (!isMock) watchForTokens();
  if (!isMock) void registerServiceWorker();
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App exchangeError={exchangeError} />
    </StrictMode>,
  );
}

void boot();
