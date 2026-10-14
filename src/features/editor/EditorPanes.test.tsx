import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { formatCombo } from "@/lib/keys/combo";
import { KEYMAP } from "@/lib/keys/keymap";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { dropEditor } from "./editorStates";
import { EditorPanes, type EditorPaneHandlers } from "./EditorPanes";

// jsdom has no layout: CodeMirror measures through these.
beforeAll(() => {
  const rects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= rects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  Element.prototype.getClientRects ??= rects;
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.scrollIntoView ??= () => {};
});

const TAB = "editor-panes-test-tab";

const handlers = (): EditorPaneHandlers => ({
  run: vi.fn(),
  runCurrent: vi.fn(),
  cancel: vi.fn(),
  explain: vi.fn(),
  analyze: vi.fn(),
  format: vi.fn(),
  minify: vi.fn(),
  toggleResults: vi.fn(),
  history: vi.fn(),
  open: vi.fn(),
  save: vi.fn(),
  saveAs: vi.fn(),
  saveSelection: vi.fn(),
  addToQueries: vi.fn(),
  exportResults: vi.fn(),
  split: vi.fn(),
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
          handlers={handlers()}
        />
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return container;
}

/** Right-clicks the editor pane and returns the open context menu. */
async function openMenu(container: HTMLElement) {
  const pane = container.querySelector<HTMLElement>("[data-pane=main]")!;
  fireEvent.contextMenu(pane);
  return screen.findByRole("menu");
}

const itemText = (menu: HTMLElement, label: string) =>
  within(menu).getByRole("menuitem", { name: new RegExp(`^${label}`) }).textContent ?? "";

beforeEach(() => resetPrefsForTests());
afterEach(() => {
  resetPrefsForTests();
  dropEditor(TAB);
});

describe("the editor pane's context menu", () => {
  it("shows the comment toggles' default keys", async () => {
    const menu = await openMenu(mount());
    expect(itemText(menu, "Toggle Line Comment")).toContain(formatCombo("Mod+/"));
    expect(itemText(menu, "Toggle Block Comment")).toContain(formatCombo("Mod+Alt+/"));
  });

  it("follows a remap of the comment toggles instead of the hard-coded ⌘/ and ⌥⌘/", async () => {
    setPref("keymapOverrides", {
      "query.comment-line": ["Mod+Shift+7"],
      "query.comment-block": ["Mod+Shift+8"],
    });
    expect(KEYMAP.find((d) => d.id === "query.comment-line")?.keys).toBe("Mod+Shift+7");
    const menu = await openMenu(mount());
    await waitFor(() =>
      expect(itemText(menu, "Toggle Line Comment")).toContain(formatCombo("Mod+Shift+7")),
    );
    expect(itemText(menu, "Toggle Line Comment")).not.toContain(formatCombo("Mod+/"));
    expect(itemText(menu, "Toggle Block Comment")).toContain(formatCombo("Mod+Shift+8"));
    expect(itemText(menu, "Toggle Block Comment")).not.toContain(formatCombo("Mod+Alt+/"));
  });

  it("shows no key for a comment toggle the user unbound", async () => {
    setPref("keymapOverrides", { "query.comment-line": [""] });
    const menu = await openMenu(mount());
    expect(itemText(menu, "Toggle Line Comment")).toBe("Toggle Line Comment");
  });
});
