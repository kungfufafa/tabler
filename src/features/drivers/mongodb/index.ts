import type { LaneModule } from "../types";

/**
 * The renderer of the mongodb lane (MongoDB; docs/drivers/mongodb.md): its connection
 * forms, views and console. Empty until the lane fills it in; `registry.ts` imports it lazily.
 */
const lane: LaneModule = {};

export default lane;
