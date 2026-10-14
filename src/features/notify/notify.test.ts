import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { appInBackground, notifyInBackground, NOTIFY_ASKED_KEY, type NotifyApi } from "./notify";

function fakeApi(granted: boolean, answer = "granted") {
  const api = {
    isPermissionGranted: vi.fn(async () => granted),
    requestPermission: vi.fn(async () => answer),
    sendNotification: vi.fn(),
  } satisfies NotifyApi;
  return { api, load: async () => api };
}

const inBackground = () => true;

describe("notifyInBackground", () => {
  beforeEach(() => {
    localStorage.clear();
    resetPrefsForTests(DEFAULT_PREFS);
  });
  afterEach(() => vi.restoreAllMocks());

  it("posts only while Tabler is in the background", async () => {
    const { api, load } = fakeApi(true);
    expect(
      await notifyInBackground({ title: "Exported" }, { api: load, background: () => false }),
    ).toBe(false);
    expect(api.sendNotification).not.toHaveBeenCalled();
    expect(
      await notifyInBackground(
        { title: "Exported 3 rows", body: "users.csv" },
        { api: load, background: inBackground },
      ),
    ).toBe(true);
    expect(api.sendNotification).toHaveBeenCalledWith({
      title: "Exported 3 rows",
      body: "users.csv",
    });
  });

  it("respects the Show notifications preference", async () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, showNotifications: false });
    const { api, load } = fakeApi(true);
    expect(await notifyInBackground({ title: "x" }, { api: load, background: inBackground })).toBe(
      false,
    );
    expect(api.isPermissionGranted).not.toHaveBeenCalled();
  });

  it("asks for permission once", async () => {
    const { api, load } = fakeApi(false, "denied");
    expect(await notifyInBackground({ title: "x" }, { api: load, background: inBackground })).toBe(
      false,
    );
    expect(api.requestPermission).toHaveBeenCalledOnce();
    expect(localStorage.getItem(NOTIFY_ASKED_KEY)).toBe("1");
    await notifyInBackground({ title: "y" }, { api: load, background: inBackground });
    expect(api.requestPermission).toHaveBeenCalledOnce();
    expect(api.sendNotification).not.toHaveBeenCalled();
    // Granted when asked: it posts.
    localStorage.clear();
    const granted = fakeApi(false, "granted");
    expect(
      await notifyInBackground({ title: "z" }, { api: granted.load, background: inBackground }),
    ).toBe(true);
    expect(granted.api.sendNotification).toHaveBeenCalledWith({ title: "z" });
  });

  it("never fails the caller", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const broken = async (): Promise<NotifyApi> => {
      throw new Error("no plugin");
    };
    expect(
      await notifyInBackground({ title: "x" }, { api: broken, background: inBackground }),
    ).toBe(false);
  });

  it("tells the background from the focus and the visibility", () => {
    expect(appInBackground({ hasFocus: () => true, visibilityState: "visible" })).toBe(false);
    expect(appInBackground({ hasFocus: () => false, visibilityState: "visible" })).toBe(true);
    expect(appInBackground({ hasFocus: () => true, visibilityState: "hidden" })).toBe(true);
  });
});
