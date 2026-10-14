import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createContext, useContext, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockAssistant } from "@/lib/ipc/mock/assistant";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { AssistantSettingsGroup } from "./AssistantSettings";

// Radix Select needs pointer / layout APIs jsdom lacks: stand in a native <select>.
vi.mock("@/components/ui/select", () => {
  interface Ctx {
    value: string;
    onValueChange: (v: string) => void;
    id?: string;
  }
  const SelectCtx = createContext<Ctx>({ value: "", onValueChange: () => {} });
  return {
    Select: ({
      value,
      onValueChange,
      children,
    }: {
      value: string;
      onValueChange: (v: string) => void;
      children: ReactNode;
    }) => <SelectCtx.Provider value={{ value, onValueChange }}>{children}</SelectCtx.Provider>,
    SelectTrigger: (props: { id?: string }) => {
      useContext(SelectCtx).id = props.id;
      return null;
    },
    SelectValue: () => null,
    SelectContent: ({ children }: { children: ReactNode }) => {
      const ctx = useContext(SelectCtx);
      return (
        <select
          data-testid={ctx.id}
          value={ctx.value}
          onChange={(e) => ctx.onValueChange(e.target.value)}
        >
          {children}
        </select>
      );
    },
    SelectItem: ({ value, children }: { value: string; children: ReactNode }) => (
      <option value={value}>{children}</option>
    ),
  };
});

beforeEach(() => {
  resetMockAssistant();
  resetPrefsForTests();
});

describe("Settings ▸ AI Assistant", () => {
  it("toggles the tab and notes that only structure is sent", () => {
    render(<AssistantSettingsGroup />);
    expect(screen.getByText(/never row data/)).toBeInTheDocument();
    const toggle = screen.getByRole("switch", { name: /Enable Assistant tab/ });
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    expect(getPrefs().assistantEnabled).toBe(false);
  });

  it("picks the Claude model, stores the key and tests it", async () => {
    render(<AssistantSettingsGroup />);
    fireEvent.change(screen.getByTestId("pref-assistant-anthropic-model"), {
      target: { value: "claude-sonnet-5-5" },
    });
    expect(getPrefs().assistantAnthropicModel).toBe("claude-sonnet-5-5");
    await waitFor(() =>
      expect(screen.getByPlaceholderText("•••••••• (stored)")).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(screen.getByPlaceholderText("Paste the key")).toBeInTheDocument());
    expect((await ipc.assistant.keyStatus()).anthropic).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Test" }));
    expect(await screen.findByRole("status")).toHaveTextContent("No API key stored");

    fireEvent.change(screen.getByPlaceholderText("Paste the key"), {
      target: { value: "sk-ant-new" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(screen.getByPlaceholderText("•••••••• (stored)")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Test" }));
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("claude-sonnet-5-5 is available"),
    );
  });

  it("validates the Ollama server and model, and lists the server's models", async () => {
    act(() => setPref("assistantProvider", "ollama"));
    render(<AssistantSettingsGroup />);
    expect(screen.queryByText("API key")).not.toBeInTheDocument();
    const server = screen.getByPlaceholderText("http://localhost:11434");
    fireEvent.change(server, { target: { value: "localhost:11434" } });
    fireEvent.blur(server);
    expect(screen.getByText(/http:\/\/ or https:\/\//)).toBeInTheDocument();
    expect(getPrefs().assistantOllamaBaseUrl).toBe("http://localhost:11434");
    fireEvent.change(server, { target: { value: "http://127.0.0.1:11434/" } });
    fireEvent.keyDown(server, { key: "Enter" });
    expect(getPrefs().assistantOllamaBaseUrl).toBe("http://127.0.0.1:11434/");

    const model = screen.getByPlaceholderText("model name");
    fireEvent.change(model, { target: { value: "sqlcoder:15b" } });
    fireEvent.blur(model);
    expect(getPrefs().assistantOllamaModel).toBe("sqlcoder:15b");
    fireEvent.click(screen.getByRole("button", { name: "Test" }));
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("sqlcoder:15b is available"),
    );
    expect(document.querySelectorAll("#assistant-models-ollama option")).toHaveLength(3);
  });
});
