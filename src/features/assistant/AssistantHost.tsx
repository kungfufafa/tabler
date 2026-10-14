import { useAction } from "@/features/palette/actions";
import { useActiveConnection } from "@/stores/connections";
import { usePrefs } from "@/stores/prefs";
import { askAssistant } from "./store";

/**
 * Registers "Ask Assistant…" (command palette): opens the right sidebar on the Assistant tab and
 * puts the caret in its message field. Only while the Assistant is enabled (Settings ▸ AI
 * Assistant) and a connection is open, so the palette lists it exactly then. Mount once.
 */
export function AssistantHost() {
  const enabled = usePrefs((p) => p.assistantEnabled);
  const connection = useActiveConnection();
  useAction("assistant.ask", enabled && connection ? askAssistant : null);
  return null;
}
