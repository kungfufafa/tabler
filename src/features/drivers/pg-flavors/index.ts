import type { LaneModule } from "../types";

/**
 * The renderer of the pg-flavors lane (CockroachDB, Greenplum and Amazon Redshift; docs/drivers/pg-flavors.md): its connection
 * forms, views and console. Empty until the lane fills it in; `registry.ts` imports it lazily.
 */
const lane: LaneModule = {};

export default lane;
