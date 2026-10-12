import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ipc, type Dialect, type TableSummary } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useStructureStore } from "./store";
import { TableDialogs, type TableDialogState } from "./TableDialogs";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

function Harness({
  connectionId,
  dialect,
  initial,
}: {
  connectionId: string;
  dialect: Dialect;
  initial: TableDialogState;
}) {
  const [state, setState] = useState<TableDialogState | null>(initial);
  return (
    <TableDialogs
      connectionId={connectionId}
      dialect={dialect}
      state={state}
      onClose={() => setState(null)}
    />
  );
}

async function setup(
  initial: (tables: TableSummary[]) => TableDialogState,
  profile = "p-local-pg",
) {
  const info = await ipc.connections.connect(profile);
  useConnectionsStore.getState().add(info);
  const schema = profile === "p-local-pg" ? "public" : "main";
  const tables = await ipc.schema.tables(info.connectionId, schema);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <Harness connectionId={info.connectionId} dialect={info.driver} initial={initial(tables)} />
      </TooltipProvider>
    </QueryClientProvider>,
  );
  const names = async () => (await ipc.schema.tables(info.connectionId, schema)).map((t) => t.name);
  return { connectionId: info.connectionId, names, tables };
}

const byName = (tables: TableSummary[], name: string) => tables.find((t) => t.name === name)!;

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useStructureStore.setState({ byTab: {} });
});

afterEach(() => {
  resetMockDdl();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("sidebar table dialogs", () => {
  it("Drop requires typing the table name and closes the table's tabs", async () => {
    const { names, connectionId } = await setup((t) => ({
      kind: "drop",
      table: byName(t, "audit_log"),
    }));
    useTabsStore.getState().openTable(connectionId, { schema: "public", name: "audit_log" });
    const drop = screen.getByRole("button", { name: "Drop table" });
    expect(drop).toBeDisabled();
    expect(screen.getByText(/CASCADE/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Type audit_log to confirm"), {
      target: { value: "audit_lo" },
    });
    expect(drop).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Type audit_log to confirm"), {
      target: { value: "audit_log" },
    });
    fireEvent.click(drop);
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Dropped table audit_log"));
    expect(await names()).not.toContain("audit_log");
    expect(useTabsStore.getState().tabs).toEqual([]);
  });

  it("Drop view runs DROP VIEW", async () => {
    const apply = vi.spyOn(ipc.ddl, "apply");
    const { names } = await setup((t) => ({ kind: "drop", table: byName(t, "active_users") }));
    fireEvent.change(screen.getByLabelText("Type active_users to confirm"), {
      target: { value: "active_users" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Drop view" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(apply.mock.calls[0]?.[2]).toEqual([
      { op: "rawSql", sql: 'DROP VIEW "public"."active_users"' },
    ]);
    expect(await names()).not.toContain("active_users");
    apply.mockRestore();
  });

  it("Rename renames the table and retargets open tabs", async () => {
    const { names, connectionId } = await setup((t) => ({
      kind: "rename",
      table: byName(t, "tags"),
    }));
    const id = useTabsStore.getState().openTable(connectionId, { schema: "public", name: "tags" });
    const input = screen.getByLabelText("New name");
    expect(screen.getByRole("button", { name: "Rename" })).toBeDisabled();
    fireEvent.change(input, { target: { value: "labels" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(await names()).toContain("labels");
    const tab = useTabsStore.getState().tabs.find((t) => t.id === id);
    expect(tab).toMatchObject({ title: "labels", table: { schema: "public", name: "labels" } });
  });

  it("Rename keeps the staged structure edits of the table's tabs, pointed at the new name", async () => {
    const { connectionId } = await setup((t) => ({ kind: "rename", table: byName(t, "tags") }));
    const tags = { schema: "public", name: "tags" };
    const id = useTabsStore.getState().openTable(connectionId, tags, "structure");
    const { stage } = useStructureStore.getState();
    stage(id, (ops) => [
      ...ops,
      { op: "alterColumnNullable", table: tags, name: "name", nullable: true },
    ]);
    stage(id, (ops) => [...ops, { op: "setTableComment", table: tags, comment: "labels" }]);
    fireEvent.change(screen.getByLabelText("New name"), { target: { value: "labels" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    const labels = { schema: "public", name: "labels" };
    const slice = useStructureStore.getState().byTab[id];
    expect(slice?.ops.map((o) => (o.op === "rawSql" ? null : o.table))).toEqual([labels, labels]);
    expect(slice?.past.at(-1)?.[0]).toMatchObject({ table: labels });
  });

  it("Truncate offers restart identity on PostgreSQL", async () => {
    const apply = vi.spyOn(ipc.ddl, "apply");
    await setup((t) => ({ kind: "truncate", table: byName(t, "posts") }));
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "Truncate" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Truncated posts"));
    expect(apply.mock.calls[0]?.[2]).toEqual([
      {
        op: "truncateTable",
        table: { schema: "public", name: "posts" },
        restartIdentity: true,
        cascade: false,
      },
    ]);
    apply.mockRestore();
  });

  it("New table: Preview SQL lists the problems before Create is pressed", async () => {
    await setup(() => ({ kind: "create", schema: "public" }));
    fireEvent.click(screen.getByRole("button", { name: /Preview SQL/ }));
    expect(screen.getByText("Fix these problems to see the SQL:")).toBeInTheDocument();
    expect(screen.getByText("Table name is required")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Table name"), { target: { value: "notes" } });
    expect(screen.queryByText("Table name is required")).not.toBeInTheDocument();
    await waitFor(() => expect(document.querySelector("pre")).toHaveTextContent(/CREATE TABLE/));
  });

  it("New table cannot be dismissed while CREATE runs", async () => {
    let finish!: (r: Awaited<ReturnType<typeof ipc.ddl.apply>>) => void;
    const apply = vi
      .spyOn(ipc.ddl, "apply")
      .mockImplementation(() => new Promise((resolve) => (finish = resolve)));
    await setup(() => ({ kind: "create", schema: "public" }));
    fireEvent.change(screen.getByLabelText("Table name"), { target: { value: "notes" } });
    fireEvent.click(screen.getByRole("button", { name: /Create table/ }));
    await waitFor(() => expect(apply).toHaveBeenCalled());
    const dialog = screen.getByRole("dialog");
    fireEvent.keyDown(dialog, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await act(async () => finish({ executed: [], failedIndex: null, error: null, durationMs: 1 }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Created table notes"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    apply.mockRestore();
  });

  it("New table validates, previews the SQL and creates the table", async () => {
    const { names } = await setup(() => ({ kind: "create", schema: "public" }));
    fireEvent.click(screen.getByRole("button", { name: /Create table/ }));
    expect(await screen.findByText("Table name is required")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Table name"), { target: { value: "notes" } });
    fireEvent.click(screen.getByRole("button", { name: /Add column/ }));
    fireEvent.change(screen.getByLabelText("Column 2 name"), { target: { value: "body" } });
    fireEvent.click(screen.getByRole("button", { name: /Preview SQL/ }));
    await waitFor(() =>
      expect(document.querySelector("pre")).toHaveTextContent(
        'CREATE TABLE "public"."notes" ( "id" serial NOT NULL, "body" text, PRIMARY KEY ("id") )',
        { normalizeWhitespace: true },
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: /Create table/ }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Created table notes"));
    expect(await names()).toContain("notes");
    expect(useTabsStore.getState().tabs[0]).toMatchObject({
      kind: "structure",
      table: { schema: "public", name: "notes" },
    });
  });
});
