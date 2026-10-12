import { useEffect, type ReactNode } from "react";
import { actions } from "@/features/palette/actions";
import { comboFromEvent } from "./combo";
import { findShortcut, type Scope } from "./keymap";

/** Resolves the keyboard scope from the focused element (`data-scope` attributes, CodeMirror, dialogs). */
export function resolveScope(target: Element | null): Scope {
  if (!target) return "global";
  if (target.closest(".cm-editor")) return "editor";
  if (target.closest("[role='dialog'], [role='alertdialog']")) return "dialog";
  const scoped = target.closest<HTMLElement>("[data-scope]");
  const scope = scoped?.dataset.scope as Scope | undefined;
  return scope ?? "global";
}

/** Text fields, selects and contenteditable regions: where a plain key types instead. */
function isTextEntry(el: Element | null): boolean {
  if (!el) return false;
  if (el instanceof HTMLElement && el.isContentEditable) return true;
  return el.matches("input, textarea, select");
}

/** One capture-phase keydown listener for the whole app; dispatches into the action registry. */
export function KeyboardProvider({ children }: { children: ReactNode }) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      const combo = comboFromEvent(e);
      const target = e.target instanceof Element ? e.target : document.activeElement;
      // Shortcuts without a modifier (Space for the row detail…) never steal typing.
      if (!e.metaKey && !e.ctrlKey && !e.altKey && isTextEntry(target)) return;
      const scope = resolveScope(target);
      const def = findShortcut(combo, scope);
      if (!def) return;
      if (!actions.has(def.id)) {
        // Nothing handles it yet; still stop the webview from doing something odd (⌘R reload, ⌘P print).
        if (def.id === "view.refresh" || def.id === "palette.open") e.preventDefault();
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      void actions.run(def.id);
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, []);
  return children;
}
