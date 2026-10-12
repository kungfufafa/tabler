import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createContext, useContext, useState, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { AddForeignKeyDialog } from "./KeyDialogs";

// Radix Select needs pointer / layout APIs jsdom lacks: stand in a native <select>.
vi.mock("@/components/ui/select", () => {
  interface Ctx {
    value: string;
    onValueChange: (v: string) => void;
    label?: string;
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
    SelectTrigger: (props: { "aria-label"?: string }) => {
      useContext(SelectCtx).label = props["aria-label"];
      return null;
    },
    SelectValue: () => null,
    SelectContent: ({ children }: { children: ReactNode }) => {
      const ctx = useContext(SelectCtx);
      return (
        <select
          aria-label={ctx.label}
          value={ctx.value}
          onChange={(e) => ctx.onValueChange(e.target.value)}
        >
          <option value="" />
          {children}
        </select>
      );
    },
    SelectItem: ({ value, children }: { value: string; children: ReactNode }) => (
      <option value={value}>{children}</option>
    ),
  };
});

const POSTS = { schema: "public", name: "posts" };

async function connect() {
  const info = await ipc.connections.connect("p-local-pg");
  useConnectionsStore.getState().add(info);
  return info.connectionId;
}

function FkHarness({ connectionId }: { connectionId: string }) {
  const [open, setOpen] = useState(true);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        reopen
      </button>
      {/* Stays mounted while closed, as in the structure tab. */}
      <AddForeignKeyDialog
        open={open}
        onOpenChange={setOpen}
        connectionId={connectionId}
        table={POSTS}
        columns={["id", "user_id", "title"]}
        onAdd={() => {}}
      />
    </>
  );
}

const pressed = (group: HTMLElement) =>
  within(group)
    .getAllByRole("button")
    .filter((b) => b.getAttribute("aria-pressed") === "true")
    .map((b) => b.textContent);

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
});

describe("AddForeignKeyDialog", () => {
  it("pre-selects the referenced primary key again after the dialog is reopened", async () => {
    const connectionId = await connect();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <FkHarness connectionId={connectionId} />
      </QueryClientProvider>,
    );
    const pickUsers = async () => {
      const select = screen.getByLabelText("Referenced table");
      await within(select).findByRole("option", { name: "users" });
      fireEvent.change(select, { target: { value: "users" } });
      const group = await screen.findByRole("group", { name: "Referenced columns" });
      await waitFor(() => expect(pressed(group)).toEqual(["1id"]));
    };

    await pickUsers();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "reopen" }));
    await screen.findByRole("dialog");
    await pickUsers();
  });
});
