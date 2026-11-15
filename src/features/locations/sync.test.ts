import { afterEach, describe, expect, it } from "vitest";
import "@/lib/ipc/mock";
import type { LocationsInfo } from "@/lib/ipc";
import { RENDERER_ID } from "@/lib/ipc/events";
import { mockEmit } from "@/lib/ipc/mock/transport";
import { resetLocationsStore, useLocationsStore } from "./store";
import { installLocationsSync } from "./sync";

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("installLocationsSync", () => {
  afterEach(() => resetLocationsStore());

  it("follows a folder moved in the Settings window, not its own announcements", async () => {
    const off = installLocationsSync();
    await tick();
    const info = { dataDir: "/elsewhere" } as unknown as LocationsInfo;
    mockEmit("locations:changed", { from: RENDERER_ID, info, queries: false });
    expect(useLocationsStore.getState().info).toBeNull();
    mockEmit("locations:changed", { from: "settings-window", info, queries: false });
    expect(useLocationsStore.getState().info).toBe(info);
    off();
    mockEmit("locations:changed", { from: "settings-window", info: null, queries: false });
    expect(useLocationsStore.getState().info).toBe(info);
  });
});
