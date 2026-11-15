import { afterEach, describe, expect, it, vi } from "vitest";
import "@/lib/ipc/mock";
import { ipc } from "@/lib/ipc";
import { RENDERER_ID } from "@/lib/ipc/events";
import { mockEmit } from "@/lib/ipc/mock/transport";
import { installKeySync } from "./keys";

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("installKeySync", () => {
  afterEach(() => vi.restoreAllMocks());

  it("reads the key status again when the Settings window stored a key", async () => {
    const status = vi.spyOn(ipc.assistant, "keyStatus");
    const off = installKeySync();
    await tick();
    mockEmit("assistant:keys-changed", { from: RENDERER_ID });
    expect(status).not.toHaveBeenCalled();
    mockEmit("assistant:keys-changed", { from: "settings-window" });
    expect(status).toHaveBeenCalledOnce();
    off();
  });
});
