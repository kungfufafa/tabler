import type { LaneModule } from "../types";
import { tsqlUi } from "./dialect";
import { sqlserverUi } from "./form";

/**
 * The renderer of the sqlserver lane (SQL Server; docs/drivers/sqlserver.md): the connection form
 * and its connection strings, and T-SQL in the editor. The grid, the structure view and the
 * editor's runs are the app's own, which the session's caps drive; the lane has no views or
 * calls of its own.
 */
const lane: LaneModule = {
  kinds: { sqlserver: sqlserverUi },
  dialects: { tsql: tsqlUi },
};

export default lane;
