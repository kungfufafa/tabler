import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { QuestionDialog, type Question } from "./QuestionDialog";

export type { Question };

/** Asks `q`: the text typed (or "" for a confirmation), or null when declined. */
export function ask(q: Question): Promise<string | null> {
  return new Promise((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    const done = (answer: string | null) => {
      resolve(answer);
      // Unmounted after the closing animation's frame.
      setTimeout(() => {
        root.unmount();
        host.remove();
      }, 200);
    };
    root.render(createElement(QuestionDialog, { q, done }));
  });
}
