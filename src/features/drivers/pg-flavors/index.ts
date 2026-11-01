import type { LaneModule } from "../types";
import { redshiftSqlUi } from "./dialect";
import { cockroachdbUi, greenplumUi, redshiftUi } from "./kinds";

/**
 * The renderer of the pg-flavors lane (CockroachDB, Greenplum and Amazon Redshift;
 * docs/drivers/pg-flavors.md): the three tiles' additions to the PostgreSQL form and their URLs,
 * and Redshift SQL in the editor. The sessions are the PostgreSQL driver's, so the grid, the
 * structure view, the editor and the admin windows are the app's own, which the session's caps
 * (per engine and version) drive; the lane has no driver views or calls of its own.
 */
const lane: LaneModule = {
  kinds: { cockroachdb: cockroachdbUi, greenplum: greenplumUi, redshift: redshiftUi },
  dialects: { redshift: redshiftSqlUi },
};

export default lane;
