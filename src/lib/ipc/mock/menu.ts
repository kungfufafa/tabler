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

/** Where the app menu was last popped up (the browser has no native menu to show). */
export const mockMenuPopups: { x: number; y: number }[] = [];

registerMock("popup_app_menu", ({ x, y }) => {
  mockMenuPopups.push({ x: x as number, y: y as number });
  return null;
});
