import { create } from "zustand";
import type { ParamField } from "./params";

/** A run waiting for the values of its query parameters (the dialog shows it). */
export interface ParamsRequest {
  /** Distinguishes successive requests. */
  id: number;
  tabId: string;
  fields: ParamField[];
  /** Prefilled values: what the tab used last time. */
  initial: Readonly<Record<string, string>>;
  /** The typed values (null: cancelled). */
  resolve: (values: Record<string, string> | null) => void;
}

export const useParamsPromptStore = create<{ request: ParamsRequest | null }>()(() => ({
  request: null,
}));

let nextId = 1;

/** The values a tab's runs used last, per variable. */
const remembered = new Map<string, Record<string, string>>();

export function rememberedParams(tabId: string): Readonly<Record<string, string>> {
  return remembered.get(tabId) ?? {};
}

/** Forgets a tab's values (it was closed for good). */
export function dropRememberedParams(tabId: string): void {
  remembered.delete(tabId);
}

/**
 * Asks for the values of `fields` (the dialog prefills what the tab used last) and resolves with
 * them, or null when the user cancels. A request still open is cancelled first.
 */
export function askParams(
  tabId: string,
  fields: ParamField[],
): Promise<Record<string, string> | null> {
  useParamsPromptStore.getState().request?.resolve(null);
  return new Promise((resolve) => {
    let settled = false;
    const request: ParamsRequest = {
      id: nextId++,
      tabId,
      fields,
      initial: rememberedParams(tabId),
      resolve: (values) => {
        if (settled) return;
        settled = true;
        if (useParamsPromptStore.getState().request === request)
          useParamsPromptStore.setState({ request: null });
        if (values) remembered.set(tabId, { ...rememberedParams(tabId), ...values });
        resolve(values);
      },
    };
    useParamsPromptStore.setState({ request });
  });
}
