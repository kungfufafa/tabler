import type { LaneModule } from "../types";
import { d1Ui } from "./d1";
import { libsqlUi } from "./libsql";

/**
 * The renderer of the sqlite-remote lane (LibSQL and Cloudflare D1; docs/drivers/sqlite-remote.md):
 * the two connection forms and their URLs. Both sessions speak the core SQLite dialect, so the
 * grid, the structure view and the SQL editor (its splitter, classifier, highlighting and
 * formatter) are the app's own: the lane has no views, console or dialect of its own.
 */
const lane: LaneModule = {
  kinds: { libsql: libsqlUi, d1: d1Ui },
};

export default lane;
