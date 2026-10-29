import type { LaneModule } from "../types";

/**
 * The renderer of the sqlite-remote lane (LibSQL and Cloudflare D1; docs/drivers/sqlite-remote.md): its connection
 * forms, views and console. Empty until the lane fills it in; `registry.ts` imports it lazily.
 */
const lane: LaneModule = {};

export default lane;
