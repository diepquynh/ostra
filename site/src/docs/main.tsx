import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@ostra/design/index.css";
import "../shared/site.css";
import "@ostra/design/docs.css";
import "./docs.css";
import { DocsApp } from "./DocsApp";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <DocsApp />
  </StrictMode>,
);
