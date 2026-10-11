import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useTabsStore } from "@/stores/tabs";
import { actions } from "./actions";
import { useDefaultActions } from "./defaultActions";

beforeEach(() => {
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("default tab.close", () => {
  it("closes the active tab, but not from inside another modal", async () => {
    const { unmount } = renderHook(() => useDefaultActions());
    const id = useTabsStore.getState().newQuery("c1");

    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    const input = document.createElement("input");
    dialog.append(input);
    document.body.append(dialog);
    input.focus();
    await actions.run("tab.close");
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual([id]);

    input.blur();
    await actions.run("tab.close");
    expect(useTabsStore.getState().tabs).toEqual([]);
    unmount();
  });
});
