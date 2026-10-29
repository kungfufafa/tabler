// The drivers' own calls (`session_call`, `driver_util`). Like the backend, the core drivers have
// none: a call is refused as unknown, and a database this build lacks is refused before it. A
// mock driver beyond the core ones answers its own.
import { driverLabel, KIND_DESCRIPTORS, kindOf } from "@/features/drivers/catalog";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { Driver } from "../bindings/Driver";
import type { ProfileInput } from "../bindings/ProfileInput";
import { isMockBuilt, loadMockDriver } from "./drivers";
import { mockLaneSession } from "./handlers";
import { mockError, mockInvoke, registerMock } from "./transport";

/** The backend's refusal of a call the driver does not know (`unknown_call`). */
function unknownCall(driver: Driver, method: unknown): never {
  mockError("UNSUPPORTED", `unsupported: ${driverLabel(driver)}: unknown call “${String(method)}”`);
}

registerMock("session_call", async ({ connectionId, method, args }) => {
  const all = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const c = all.find((x) => x.connectionId === connectionId);
  if (!c) mockError("NOT_CONNECTED", "not connected");
  const lane = mockLaneSession(c.connectionId);
  if (lane?.call) return lane.call(String(method), args ?? null);
  unknownCall(c.driver, method);
});

registerMock("driver_util", async ({ input, method, args }) => {
  const { profile } = input as ProfileInput;
  const kind = kindOf(profile);
  if (!isMockBuilt(kind))
    mockError(
      "UNSUPPORTED",
      `unsupported: ${KIND_DESCRIPTORS[kind].label} is not built into this Tabler`,
    );
  const mock = await loadMockDriver(profile.driver);
  if (mock?.util) return mock.util(String(method), args ?? null);
  unknownCall(profile.driver, method);
});
