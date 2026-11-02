import type { LaneModule } from "../types";
import { snowflakeSqlUi } from "./dialect";
import { snowflakeUi } from "./form";

/**
 * The renderer of the snowflake lane (Snowflake; docs/drivers/snowflake.md): the connection form
 * with its URLs and the form's calls (the key's fingerprint, the warehouses, roles and databases
 * the sign-in sees: all reads), and Snowflake SQL in the editor with Format. The grid (offset
 * pages, costly counts), the structure view, the editor's runs, Running Queries with Kill Query,
 * the routines and Show DDL are the app's own, which the session's caps drive; Safe Mode reads
 * Snowflake SQL with the backend's lexer profile (`bindings/lex/snowflake.json`) and its quoted
 * NEXTVAL check (`classify.ts`). The session has no calls of its own, so the lane has no views,
 * console or object menu.
 */
const lane: LaneModule = {
  kinds: { snowflake: snowflakeUi },
  dialects: { snowflake: snowflakeSqlUi },
};

export default lane;
