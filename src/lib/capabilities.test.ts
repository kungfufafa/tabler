/// <reference types="node" />
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const dir = path.resolve(import.meta.dirname, "../../src-tauri/capabilities");

interface Capability {
  identifier: string;
  windows: string[];
  permissions: Array<string | { identifier: string }>;
}

const capabilities: Capability[] = readdirSync(dir)
  .filter((f) => f.endsWith(".json"))
  .map((f) => JSON.parse(readFileSync(path.join(dir, f), "utf8")) as Capability);

/** Every permission granted to the `main` window (and `w-2`, the windows opened later). */
function grantedTo(window: string): Set<string> {
  const granted = new Set<string>();
  for (const c of capabilities) {
    if (
      !c.windows.some((w) => w === window || (w.endsWith("*") && window.startsWith(w.slice(0, -1))))
    )
      continue;
    for (const p of c.permissions) granted.add(typeof p === "string" ? p : p.identifier);
  }
  return granted;
}

describe("window capabilities", () => {
  it.each(["main", "w-2"])("lets the renderer set the native theme in %s", (window) => {
    const granted = grantedTo(window);
    // Theme (Settings ▸ Appearance) also sets the menu bar, file panels and dialogs.
    expect(granted.has("core:app:allow-set-app-theme")).toBe(true);
    expect(granted.has("core:window:allow-set-theme")).toBe(true);
  });

  it("grants them in default.json and desktop.json", () => {
    for (const file of ["default.json", "desktop.json"]) {
      const c = JSON.parse(readFileSync(path.join(dir, file), "utf8")) as Capability;
      const ids = c.permissions.map((p) => (typeof p === "string" ? p : p.identifier));
      expect(ids, file).toContain("core:app:allow-set-app-theme");
      expect(ids, file).toContain("core:window:allow-set-theme");
    }
  });
});
