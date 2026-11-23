import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "@/App";
import { applyTheme, readStoredTheme } from "@/hooks/use-theme";
import { startLanguage } from "@/i18n/language";
import "@/index.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Root element #root not found");
}

applyTheme(readStoredTheme());

// The chosen language is read before the first render, so the app never opens in the wrong one.
void startLanguage().then(() => {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
});
