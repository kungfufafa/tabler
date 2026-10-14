import { openSettings } from "@/features/settings/store";

/** Element id of the Settings ▸ AI Assistant group. */
export const ASSISTANT_SETTINGS_ID = "assistant-settings";

/** Settings ▸ AI Assistant ("Open Settings" in the chat). */
export function openAssistantSettings(): void {
  openSettings("assistant");
  setTimeout(() => {
    document.getElementById(ASSISTANT_SETTINGS_ID)?.scrollIntoView({ block: "start" });
  }, 80);
}
