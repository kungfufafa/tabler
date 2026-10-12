import { SavedQueryDialog } from "./SavedQueryDialog";

/**
 * App-wide host of the saved query popup ("Add to Queries…" from the SQL editor and History,
 * New ▸ Query… and Edit… in the Queries pane), so it works whichever of them is on screen.
 */
export function QueriesHost() {
  return <SavedQueryDialog />;
}
