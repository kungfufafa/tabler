import type { LaneModule } from "../types";
import { clickhouseSqlUi } from "./dialect";
import { clickhouseUi } from "./form";

/**
 * The renderer of the clickhouse lane (ClickHouse; docs/drivers/clickhouse.md): the connection
 * form and its URLs, and ClickHouse SQL in the editor. The grid (offset pages, edits by primary
 * key), the structure view, the editor's runs and Running Queries are the app's own, which the
 * session's caps drive; the lane has no views of its own, and no session or form calls (the
 * backend has none), so nothing here writes past Safe Mode.
 */
const lane: LaneModule = {
  kinds: { clickhouse: clickhouseUi },
  dialects: { clickhouse: clickhouseSqlUi },
};

export default lane;
