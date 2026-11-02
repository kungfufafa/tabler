import { createElement, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import type { RedisNewKeyArgs } from "@/lib/ipc/bindings/RedisNewKeyArgs";
import { NewKeyDialog } from "./NewKeyDialog";
import { QuestionDialog, type Question } from "./QuestionDialog";

export type { Question };

/** Renders a dialog in a root of its own until it is done (a menu item has no place in the tree). */
function open<T>(render: (done: (answer: T) => void) => ReactElement): Promise<T> {
  return new Promise((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    const done = (answer: T) => {
      resolve(answer);
      // Unmounted after the closing animation's frame.
      setTimeout(() => {
        root.unmount();
        host.remove();
      }, 200);
    };
    root.render(render(done));
  });
}

/** Asks `q`: the text typed (or "" for a confirmation), or null when declined. */
export const ask = (q: Question): Promise<string | null> =>
  open((done) => createElement(QuestionDialog, { q, done }));

/** New key… in database `db`: the `newKey` call's arguments, or null when cancelled. */
export const askNewKey = (db: number): Promise<RedisNewKeyArgs | null> =>
  open((done) => createElement(NewKeyDialog, { db, done }));
