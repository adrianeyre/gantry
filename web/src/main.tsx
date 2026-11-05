import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applySavedAppearance } from "./theme";
import "./themes.css";
import "./styles.css";

// index.html painted the saved mode and theme already; from here "System" follows the device live.
applySavedAppearance();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
