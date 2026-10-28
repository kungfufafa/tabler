// The drivers' own calls (`session_call`, `driver_util`). Like the backend, the core drivers have
// none: a call is refused as unknown, and a database this build lacks is refused before it.
import { driverLabel, isCoreKind, KIND_DESCRIPTORS, kindOf } from "@/features/drivers/catalog";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { Driver } from "../bindings/Driver";
import type { ProfileInput } from "../bindings/ProfileInput";
import { mockError, mockInvoke, registerMock } from "./transport";

/** The backend's refusal of a call the driver does not know (`unknown_call`). */
function unknownCall(driver: Driver, method: unknown): never {
  mockError("UNSUPPORTED", `unsupported: ${driverLabel(driver)}: unknown call “${String(method)}”`);
}

registerMock("session_call", async ({ connectionId, method }) => {
  const all = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const c = all.find((x) => x.connectionId === connectionId);
  if (!c) mockError("NOT_CONNECTED", "not connected");
  unknownCall(c.driver, method);
});

registerMock("driver_util", ({ input, method }) => {
  const { profile } = input as ProfileInput;
  const kind = kindOf(profile);
  if (!isCoreKind(kind))
    mockError(
      "UNSUPPORTED",
      `unsupported: ${KIND_DESCRIPTORS[kind].label} is not built into this Tabler`,
    );
  unknownCall(profile.driver, method);
});
