import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { dropEditor, seedEditor } from "@/features/editor/editorStates";
import { EditorPanes, type EditorPaneHandlers } from "@/features/editor/EditorPanes";
import { registerEditorMenuExtension } from "@/features/editor/menuExtensions";
import { resetMockPlugins } from "@/lib/ipc/mock/plugins";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { editorPluginMenu } from "./integration";
import { refreshPlugins, resetPluginsStoreForTests } from "./store";

// jsdom has no layout: CodeMirror measures through these.
beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.scrollIntoView ??= () => {};
});

const TAB = "plugins-editor-menu-tab";
const noop = () => {};
const handlers = new Proxy({}, { get: () => noop }) as EditorPaneHandlers;

let off: (() => void) | null = null;
beforeEach(() => {
  resetPrefsForTests();
  resetPluginsStoreForTests();
  resetMockPlugins();
  seedEditor(TAB, "SELECT 1");
  off = registerEditorMenuExtension(editorPluginMenu);
});
afterEach(() => {
  off?.();
  dropEditor(TAB);
  resetPrefsForTests();
});

function mount() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const { container } = render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <EditorPanes
          tabId={TAB}
          connectionId="c1"
          dialect="postgres"
          currentDatabase="app"
          handlers={handlers}
        />
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return container;
}

describe("the SQL editor's own context menu", () => {
  it("ends with Plugins ▸ listing the enabled commands for the editor", async () => {
    setPref("pluginStates", { "tabler.uuid-generator": { enabled: true } });
    await refreshPlugins();
    fireEvent.contextMenu(mount().querySelector<HTMLElement>("[data-pane=main]")!);
    const menu = await screen.findByRole("menu");
    const items = within(menu).getAllByRole("menuitem");
    // After the split item, as the last entry.
    expect(items[items.length - 1]).toHaveTextContent("Plugins");
    fireEvent.keyDown(items[items.length - 1]!, { key: "ArrowRight" });
    expect(await screen.findByRole("menuitem", { name: "Insert UUID" })).toBeInTheDocument();
  });

  it("has no Plugins entry while no enabled plugin has an editor command", async () => {
    setPref("pluginStates", {
      "tabler.sql-formatter": { enabled: false },
      "tabler.uuid-generator": { enabled: false },
    });
    fireEvent.contextMenu(mount().querySelector<HTMLElement>("[data-pane=main]")!);
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByRole("menuitem", { name: /Plugins/ })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: /Run Current/ })).toBeInTheDocument();
  });
});
