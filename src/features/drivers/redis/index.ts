import type { LaneModule } from "../types";
import { redisConsole } from "./console";
import { redisUi } from "./form";
import { redisObjectMenu } from "./menu";
import { RedisInspector } from "./RedisInspector";

/**
 * The renderer of the redis lane (Redis and Valkey; docs/drivers/redis.md): the connection form
 * with its `redis://` and `rediss://` URLs, the redis-cli console (split and classified as the
 * backend does), the per-type key inspector, and a database's menu. The key browser is the app's
 * grid: the session's `(key, type)` rows, SCAN cursor pages and the read-only `type` column fit
 * it, so the lane gives no TableView or StructureView (the Structure tab shows `definition()`).
 */
const lane: LaneModule = {
  kinds: { redis: redisUi },
  drivers: {
    redis: { console: redisConsole, Inspector: RedisInspector, objectMenu: redisObjectMenu },
  },
};

export default lane;
