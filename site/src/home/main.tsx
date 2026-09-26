import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@ostra/design/index.css";
import "../shared/site.css";
import "./home.css";
import { Homepage } from "./Homepage";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Homepage />
  </StrictMode>,
);
