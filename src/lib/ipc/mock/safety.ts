import { mockStoredPassword } from "./handlers";
import { mockInvoke, registerMock } from "./transport";

/**
 * Safe-mode password check, like `commands/safety.rs`: true only for the profile's stored DB
 * password ("secret" for the seeded Local Postgres, or whatever the connection form saved).
 */
registerMock("verify_connection_password", async ({ profileId, password }) => {
  // NOT_FOUND for an unknown profile, as the backend.
  await mockInvoke("get_profile", { id: profileId });
  const stored = mockStoredPassword(String(profileId));
  return stored !== null && password === stored;
});
