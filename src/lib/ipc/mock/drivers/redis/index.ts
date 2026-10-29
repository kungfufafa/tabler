import type { MockLane } from "../types";

/**
 * The mock drivers of the redis lane (Redis and Valkey), served in `vite --mode mock` with
 * `?mockDrivers=`. None until the lane adds its own.
 */
const lane: MockLane = { drivers: [] };

export default lane;
