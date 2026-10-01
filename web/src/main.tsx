import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { isMock } from "./api";
import { exchangeTokenFromUrl, watchForTokens } from "./auth";
import "@ostra/design/index.css";
import { watchInstall } from "./lib/install";
import { isAndroid } from "./lib/keys";
import { registerServiceWorker } from "./lib/push";

async function boot() {
  watchInstall();
  // Android turns an Esc the page leaves unhandled into Back, which closes the installed app.
  if (isAndroid)
    window.addEventListener(
      "keydown",
      (e) => {
        if (e.key === "Escape") e.preventDefault();
      },
      true,
    );
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
