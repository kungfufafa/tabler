import type { MockLane } from "../types";

/**
 * The mock drivers of the sqlite-remote lane (LibSQL and Cloudflare D1), served in `vite --mode mock` with
 * `?mockDrivers=`. None until the lane adds its own.
 */
const lane: MockLane = { drivers: [] };

export default lane;
