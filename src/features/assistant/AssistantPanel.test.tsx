import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { actions as actionRegistry } from "@/features/palette/actions";
import { InspectorPanel } from "@/features/inspector/InspectorPanel";
import { ipc } from "@/lib/ipc";
import { resetMockAssistant } from "@/lib/ipc/mock/assistant";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { resetAgentsForTests } from "./agents";
import { AssistantHost } from "./AssistantHost";
import { resetAssistantForTests, useAssistantStore } from "./store";

const actions = vi.hoisted(() => ({
  insertIntoEditor: vi.fn(),
  runInNewTab: vi.fn(),
  copyText: vi.fn(),
}));
vi.mock("./actions", () => actions);

async function setup() {
  const info = await ipc.connections.connect("p-local-pg");
  act(() => {
    useConnectionsStore.getState().add(info);
    useUiStore.setState({ inspectorOpen: true });
  });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <InspectorPanel />
    </QueryClientProvider>,
  );
  return info;
}

beforeEach(() => {
  resetMockAssistant();
  resetMockSettings();
  resetAssistantForTests();
  resetAgentsForTests();
  resetPrefsForTests();
  vi.clearAllMocks();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
});

describe("Assistant tab", () => {
  it("chats with ⌘↩ and offers Insert / Run / Copy on SQL blocks", async () => {
    const info = await setup();
    fireEvent.click(screen.getByRole("radio", { name: "Assistant" }));
    expect(screen.getByRole("radio", { name: "Assistant" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByText(`Ask about ${info.name}`)).toBeInTheDocument();
    expect(screen.getAllByText(/never row data/).length).toBeGreaterThan(0);

    const box = screen.getByRole("textbox", { name: "Message the assistant" });
    fireEvent.change(box, { target: { value: "Show me recent users" } });
    fireEvent.keyDown(box, { key: "Enter", metaKey: true });

    const log = screen.getByRole("log", { name: "Assistant conversation" });
    await waitFor(() => expect(within(log).getByTitle("Copy reply")).toBeInTheDocument());
    expect(within(log).getByText("Show me recent users")).toBeInTheDocument();
    const code = log.querySelector("[data-code-block]")!;
    expect(code.textContent).toMatch(/SELECT/);

    fireEvent.click(within(code as HTMLElement).getByRole("button", { name: /Run/ }));
    expect(actions.runInNewTab).toHaveBeenCalledTimes(1);
    expect(actions.runInNewTab.mock.calls[0]![2]).toMatch(/^SELECT .*\nFROM .*LIMIT 10;$/s);
    fireEvent.click(within(code as HTMLElement).getByRole("button", { name: /Insert/ }));
    expect(actions.insertIntoEditor.mock.calls[0]![1]).toMatch(/^SELECT/);
    fireEvent.click(within(code as HTMLElement).getByRole("button", { name: "Copy code" }));
    expect(actions.copyText).toHaveBeenCalledWith(expect.stringMatching(/^SELECT/), "SQL");

    // Clear empties the conversation.
    fireEvent.click(screen.getByRole("button", { name: "Clear conversation" }));
    expect(screen.getByText(`Ask about ${info.name}`)).toBeInTheDocument();
  });

  it("links a missing key to Settings", async () => {
    await ipc.assistant.setKey("anthropic", null);
    await setup();
    fireEvent.click(screen.getByRole("radio", { name: "Assistant" }));
    await waitFor(() => expect(screen.getByText(/Add your Anthropic API key/)).toBeInTheDocument());
    fireEvent.change(screen.getByRole("textbox", { name: "Message the assistant" }), {
      target: { value: "hi" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("API key needed");
    expect(within(alert).getByRole("button", { name: "Open Settings" })).toBeInTheDocument();
  });

  it("is hidden when Settings ▸ Security turns it off", async () => {
    await setup();
    fireEvent.click(screen.getByRole("radio", { name: "Assistant" }));
    act(() => setPref("assistantEnabled", false));
    expect(screen.queryByRole("radio", { name: "Assistant" })).not.toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Details" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText("No row selected")).toBeInTheDocument();
  });
});

describe("Ask Assistant… (command palette)", () => {
  /** What AppShell mounts: the host, and the sidebar only while it is open. */
  function Shell() {
    const open = useUiStore((s) => s.inspectorOpen);
    return (
      <QueryClientProvider client={new QueryClient()}>
        <AssistantHost />
        {open && <InspectorPanel />}
      </QueryClientProvider>
    );
  }

  it("is only available while the Assistant is on and a connection is open", async () => {
    render(<Shell />);
    expect(actionRegistry.has("assistant.ask")).toBe(false);
    const info = await ipc.connections.connect("p-local-pg");
    act(() => useConnectionsStore.getState().add(info));
    await waitFor(() => expect(actionRegistry.has("assistant.ask")).toBe(true));
    act(() => setPref("assistantEnabled", false));
    await waitFor(() => expect(actionRegistry.has("assistant.ask")).toBe(false));
  });

  it("opens the sidebar on the Assistant tab and focuses its message field, each time", async () => {
    const info = await ipc.connections.connect("p-local-pg");
    act(() => {
      useConnectionsStore.getState().add(info);
      useUiStore.setState({ inspectorOpen: false });
    });
    render(<Shell />);
    expect(screen.queryByRole("textbox", { name: "Message the assistant" })).toBeNull();
    await waitFor(() => expect(actionRegistry.has("assistant.ask")).toBe(true));

    await act(() => actionRegistry.run("assistant.ask"));
    const box = await screen.findByRole("textbox", { name: "Message the assistant" });
    expect(useUiStore.getState().inspectorOpen).toBe(true);
    expect(useAssistantStore.getState().active).toBe(true);
    await waitFor(() => expect(box).toHaveFocus());

    // Already showing: it just takes the focus back.
    act(() => box.blur());
    expect(box).not.toHaveFocus();
    await act(() => actionRegistry.run("assistant.ask"));
    await waitFor(() => expect(box).toHaveFocus());
  });
});
