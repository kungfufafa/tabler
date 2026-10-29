import type { MockLane } from "../types";

/**
 * The mock drivers of the elasticsearch lane (Elasticsearch and OpenSearch), served in `vite --mode mock` with
 * `?mockDrivers=`. None until the lane adds its own.
 */
const lane: MockLane = { drivers: [] };

export default lane;
