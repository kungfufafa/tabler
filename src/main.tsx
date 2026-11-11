import React from "react";
import { LucideProvider } from "lucide-react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {/* One stroke weight for every icon; an icon that sets its own keeps it. */}
    <LucideProvider strokeWidth={1.5}>
      <App />
    </LucideProvider>
  </React.StrictMode>,
);
