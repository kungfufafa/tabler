import { registerMock } from "./transport";

/** Native menu accelerators as the backend last received them (the browser has no menu bar). */
export const mockMenuAccelerators = new Map<string, string | null>();

registerMock("set_menu_accelerators", ({ accelerators }) => {
  for (const [id, accelerator] of Object.entries(
    (accelerators ?? {}) as Record<string, string | null>,
  ))
    mockMenuAccelerators.set(id, accelerator);
  return null;
});
