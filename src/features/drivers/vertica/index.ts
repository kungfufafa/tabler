import type { LaneModule } from "../types";
import { verticaSqlUi } from "./dialect";
import { verticaUi } from "./form";

/**
 * The renderer of the vertica lane (Vertica; docs/drivers/vertica.md): the connection form and
 * its URLs, and Vertica SQL in the editor. The grid, the structure view (projections as indexes),
 * the editor's runs, Running Queries and the routines are the app's own, which the session's caps
 * drive; Safe Mode reads Vertica SQL with the backend's lexer profile (`bindings/lex/vertica.json`)
 * and its lookalike folding (`classify.ts`).
 * The session has no calls of its own, so the lane has no views, console or object menu.
 */
const lane: LaneModule = {
  kinds: { vertica: verticaUi },
  dialects: { vertica: verticaSqlUi },
};

export default lane;
