import { beforeEach, describe, expect, it } from "vitest";
import {
  desiredShape,
  loadSavedShape,
  planReshape,
  savedWelcomeLabels,
  storeSavedShape,
  type WindowFrame,
} from "@/stores/windowShape";
import { applyShape, type WindowDriver } from "./windowDriver";

const BIG: WindowFrame = { x: 100, y: 80, width: 1400, height: 900 };
const SMALL: WindowFrame = { x: 580, y: 300, width: 760, height: 480 };

/** A window that records what it was asked to do. */
function fakeWindow(
  init: { frame?: WindowFrame; fullscreen?: boolean; maximized?: boolean; visible?: boolean } = {},
) {
  const state = {
    frame: init.frame ?? BIG,
    fullscreen: init.fullscreen ?? false,
    maximized: init.maximized ?? false,
    visible: init.visible ?? true,
    resizable: true,
    minSize: null as { width: number; height: number } | null,
  };
  const calls: string[] = [];
  const driver: WindowDriver = {
    label: "main",
    frame: async () => state.frame,
    isFullscreen: async () => state.fullscreen,
    isMaximized: async () => state.maximized,
    isVisible: async () => state.visible,
    unmaximize: async () => {
      calls.push("unmaximize");
      state.maximized = false;
    },
    hide: async () => {
      calls.push("hide");
      state.visible = false;
    },
    reveal: async () => {
      calls.push("reveal");
      state.visible = true;
    },
    setSize: async (width, height) => {
      calls.push(`size ${width}x${height}`);
      state.frame = { ...state.frame, width, height };
    },
    setPosition: async (x, y) => {
      calls.push(`position ${x},${y}`);
      state.frame = { ...state.frame, x, y };
    },
    center: async () => {
      calls.push("center");
    },
    setResizable: async (resizable) => {
      calls.push(`resizable ${resizable}`);
      state.resizable = resizable;
    },
    setMaximizable: async () => undefined,
    setMinSize: async (size) => {
      calls.push(size ? `min ${size.width}x${size.height}` : "min none");
      state.minSize = size;
    },
  };
  return { driver, state, calls };
}

const noPause = async () => undefined;

beforeEach(() => localStorage.clear());

describe("desiredShape", () => {
  it("waits for the startup, then follows what is open", () => {
    const none = { connections: 0, opening: false, decided: false };
    expect(desiredShape(none)).toBeNull();
    expect(desiredShape({ ...none, decided: true })).toBe("welcome");
    // The startup restore is connecting: a workspace window already.
    expect(desiredShape({ ...none, opening: true })).toBe("workspace");
    expect(desiredShape({ ...none, connections: 1 })).toBe("workspace");
    // The standalone SQL Viewer needs the room.
    expect(desiredShape({ ...none, decided: true, roomy: true })).toBe("workspace");
  });
});

describe("planReshape", () => {
  it("remembers the workspace frame on the way to Welcome and returns it on the way back", () => {
    const toWelcome = planReshape(null, "welcome", BIG);
    expect(toWelcome).toEqual({
      saved: { shape: "welcome", frame: BIG },
      change: { kind: "toWelcome" },
    });
    expect(planReshape(toWelcome.saved, "workspace", SMALL)).toEqual({
      saved: { shape: "workspace", frame: BIG },
      change: { kind: "toWorkspace", frame: BIG },
    });
  });

  it("keeps a window that has the shape already", () => {
    // Never shaped: a workspace window.
    expect(planReshape(null, "workspace", BIG).change).toEqual({ kind: "keep" });
    const saved = { shape: "welcome", frame: BIG } as const;
    expect(planReshape(saved, "welcome", SMALL)).toEqual({ saved, change: { kind: "keep" } });
  });
});

describe("saved shapes", () => {
  it("round-trips by window label and ignores what it cannot read", () => {
    expect(loadSavedShape("main")).toBeNull();
    storeSavedShape("main", { shape: "welcome", frame: BIG });
    storeSavedShape("w-2", { shape: "workspace", frame: null });
    expect(loadSavedShape("main")).toEqual({ shape: "welcome", frame: BIG });
    expect(savedWelcomeLabels()).toEqual(["main"]);
    localStorage.setItem("tabler.windowShape.w-3", "{nope");
    localStorage.setItem("tabler.windowShape.w-4", JSON.stringify({ shape: "welcome", frame: 3 }));
    expect(loadSavedShape("w-3")).toBeNull();
    expect(loadSavedShape("w-4")).toEqual({ shape: "welcome", frame: null });
  });
});

describe("applyShape", () => {
  it("turns a workspace window into the Welcome window, hidden while it resizes", async () => {
    const { driver, state, calls } = fakeWindow();
    const sized: string[] = [];
    const got = await applyShape("welcome", driver, (s) => sized.push(s), noPause);
    expect(got).toBe("welcome");
    expect(calls).toEqual([
      "hide",
      "min none",
      "size 760x480",
      "center",
      "resizable false",
      "reveal",
    ]);
    expect(sized).toEqual(["welcome"]);
    expect(state.visible).toBe(true);
    expect(loadSavedShape("main")).toEqual({ shape: "welcome", frame: BIG });
  });

  it("gives the workspace window its frame back", async () => {
    storeSavedShape("main", { shape: "welcome", frame: BIG });
    const { driver, state, calls } = fakeWindow({ frame: SMALL });
    expect(await applyShape("workspace", driver, undefined, noPause)).toBe("workspace");
    expect(calls).toEqual([
      "hide",
      "resizable true",
      "size 1400x900",
      "position 100,80",
      "min 960x600",
      "reveal",
    ]);
    expect(state.frame).toEqual(BIG);
    expect(loadSavedShape("main")).toEqual({ shape: "workspace", frame: BIG });
  });

  it("takes the default size when no workspace frame is remembered", async () => {
    storeSavedShape("main", { shape: "welcome", frame: null });
    const { driver, calls } = fakeWindow({ frame: SMALL });
    await applyShape("workspace", driver, undefined, noPause);
    expect(calls).toContain("size 1280x820");
    expect(calls).toContain("center");
  });

  it("only sets the constraints of a window that has the shape already", async () => {
    const workspace = fakeWindow();
    await applyShape("workspace", workspace.driver, undefined, noPause);
    expect(workspace.calls).toEqual(["resizable true", "min 960x600"]);

    // Restarted as the Welcome window: the restored geometry is kept where it was.
    storeSavedShape("main", { shape: "welcome", frame: BIG });
    const welcome = fakeWindow({ frame: SMALL });
    await applyShape("welcome", welcome.driver, undefined, noPause);
    expect(welcome.calls).toEqual(["min none", "size 760x480", "resizable false"]);
  });

  it("leaves a full-screen or zoomed window a workspace window", async () => {
    const full = fakeWindow({ fullscreen: true });
    expect(await applyShape("welcome", full.driver, undefined, noPause)).toBe("workspace");
    expect(full.calls).toEqual([]);
    expect(loadSavedShape("main")).toBeNull();

    const zoomed = fakeWindow({ maximized: true });
    expect(await applyShape("welcome", zoomed.driver, undefined, noPause)).toBe("workspace");
    expect(zoomed.calls).toEqual([]);
  });

  it("does not remember the Welcome window's own size as the workspace frame", async () => {
    // Remembered as a workspace window, found at the Welcome size (the app was killed).
    storeSavedShape("main", { shape: "workspace", frame: null });
    const toWelcome = fakeWindow({ frame: SMALL });
    await applyShape("welcome", toWelcome.driver, undefined, noPause);
    expect(loadSavedShape("main")).toEqual({ shape: "welcome", frame: null });

    storeSavedShape("main", { shape: "workspace", frame: null });
    const stays = fakeWindow({ frame: SMALL });
    await applyShape("workspace", stays.driver, undefined, noPause);
    // Below the workspace minimum: the default size, not a 760 px workspace.
    expect(stays.calls).toContain("size 1280x820");
  });

  it("never leaves the window hidden when a step fails", async () => {
    const { driver, state } = fakeWindow();
    driver.setSize = async () => {
      throw new Error("not allowed");
    };
    await expect(applyShape("welcome", driver, undefined, noPause)).rejects.toThrow("not allowed");
    expect(state.visible).toBe(true);
  });
});

describe("the Welcome window built hidden", () => {
  it("is laid out before it first shows, and never hidden again", async () => {
    // Opened before: it has the Welcome shape already, and is centred all the same.
    storeSavedShape("main", { shape: "welcome", frame: null });
    const { driver, state, calls } = fakeWindow({ frame: SMALL, visible: false });
    const steps: string[] = [];
    const shape = await applyShape(
      "welcome",
      driver,
      (to) => steps.push(`sized ${to}`),
      async () => {
        steps.push(`pause while ${state.visible ? "shown" : "hidden"}`);
      },
    );
    expect(shape).toBe("welcome");
    expect(steps).toEqual(["sized welcome", "pause while hidden"]);
    expect(calls).not.toContain("hide");
    expect(calls).toContain("center");
    expect(calls.at(-1)).toBe("reveal");
    expect(state.visible).toBe(true);
    expect(state.resizable).toBe(false);
  });
});
