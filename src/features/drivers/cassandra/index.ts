import type { LaneModule } from "../types";

/**
 * The renderer of the cassandra lane (Cassandra and ScyllaDB; docs/drivers/cassandra.md): its connection
 * forms, views and console. Empty until the lane fills it in; `registry.ts` imports it lazily.
 */
const lane: LaneModule = {};

export default lane;
