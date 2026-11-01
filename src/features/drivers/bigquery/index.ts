import type { LaneModule } from "../types";
import { bigquerySqlUi } from "./dialect";
import { bigqueryUi } from "./form";
import { bigqueryObjectMenu } from "./menu";

/**
 * The renderer of the bigquery lane (BigQuery; docs/drivers/bigquery.md): the connection form
 * and its URLs, GoogleSQL in the editor, and Estimate Full Scan… in a table's menu (the `dryRun`
 * call). The grid (cursor pages), the structure view, the editor's runs and Running Queries are
 * the app's own, which the session's caps drive. The lane's calls (`listProjects`,
 * `listDatasets`, `dryRun`) all read, so nothing here writes past Safe Mode.
 */
const lane: LaneModule = {
  kinds: { bigquery: bigqueryUi },
  drivers: { bigquery: { objectMenu: bigqueryObjectMenu } },
  dialects: { bigquery: bigquerySqlUi },
};

export default lane;
