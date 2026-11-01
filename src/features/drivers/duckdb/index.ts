import type { LaneModule } from "../types";
import { duckdbSqlUi } from "./dialect";
import { duckdbUi } from "./form";

/**
 * The renderer of the duckdb lane (DuckDB; docs/drivers/duckdb.md): the connection form (the file,
 * New…, `:memory:`, the encryption key, autoload) and its URLs, and DuckDB SQL in the editor. The
 * grid, the structure view, the editor's runs, Show DDL and the macros are the app's own, which the
 * session's caps drive; Safe Mode reads DuckDB SQL with the backend's lexer profile
 * (`bindings/lex/duckdb.json`). The session has no calls of its own, so the lane has no views,
 * console or object menu.
 */
const lane: LaneModule = {
  kinds: { duckdb: duckdbUi },
  dialects: { duckdb: duckdbSqlUi },
};

export default lane;
