import { confirmsWithPasscode } from "@/features/drivers/catalog";
import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { PasscodeStatus } from "../bindings/PasscodeStatus";
import { mockStoredPassword } from "./handlers";
import { mockInvoke, registerMock } from "./transport";

/**
 * Safe-mode check, like `verify_safe_mode` in `commands/safety.rs`: the profile's stored DB
 * password ("secret" for the seeded Local Postgres, or whatever the connection form saved); for a
 * profile whose secret is a token (`confirmsWithPasscode`), the app passcode when one is set, else
 * the connection's name.
 */
registerMock("verify_connection_password", async ({ profileId, password }) => {
  // NOT_FOUND for an unknown profile, as the backend.
  const profile = await mockInvoke<ConnectionProfile>("get_profile", { id: profileId });
  if (confirmsWithPasscode(profile)) {
    const { enabled } = await mockInvoke<PasscodeStatus>("passcode_status", {});
    if (enabled) return mockInvoke<boolean>("verify_passcode", { passcode: password });
    const name = profile.name.trim();
    return name !== "" && String(password).trim() === name;
  }
  const stored = mockStoredPassword(String(profileId));
  return stored !== null && password === stored;
});
