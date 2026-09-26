import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { isMock } from "./api";
import { exchangeTokenFromUrl, watchForTokens } from "./auth";
import "@ostra/design/index.css";
import { registerServiceWorker } from "./lib/push";

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
