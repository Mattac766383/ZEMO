import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { BetaErrorBoundary } from "./BetaErrorBoundary";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BetaErrorBoundary>
      <App />
    </BetaErrorBoundary>
  </React.StrictMode>,
);
