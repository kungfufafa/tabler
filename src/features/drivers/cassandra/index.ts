import type { LaneModule } from "../types";
import { cqlUi } from "./dialect";
import { cassandraUi } from "./form";

/**
 * The renderer of the cassandra lane (Cassandra and ScyllaDB; docs/drivers/cassandra.md): the
 * connection form and its URLs, and CQL in the editor. The grid (pages by the paging state), the
 * structure view and the editor's runs are the app's own, which the session's caps drive; the
 * lane has no views of its own, and no session or form calls (the backend has none), so nothing
 * here writes past Safe Mode.
 */
const lane: LaneModule = {
  kinds: { cassandra: cassandraUi },
  dialects: { cql: cqlUi },
};

export default lane;
