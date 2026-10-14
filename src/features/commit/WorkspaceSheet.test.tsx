import { act, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { applyToEditor } from "@/features/editor/editorStates";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { emptyStaging, setCell } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import {
  ipc,
  type ConnectionInfo,
  type DefinitionRestore,
  type Driver,
  type TableRef,
  type Value,
  type WorkspaceGroup,
} from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { useTabsStore } from "@/stores/tabs";
import { markDefinition, resetDefinitions } from "./definitions";
import { resetRowContexts, setRowContext } from "./rowContext";
import { openWorkspaceSheet, resetCommitStore, setFailure } from "./store";
import { WorkspaceSheet } from "./WorkspaceSheet";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

// CodeMirror (the SQL blocks) measures text ranges, which jsdom does not lay out.
if (!Range.prototype.getClientRects) {
  const empty = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] });
  Range.prototype.getClientRects = empty as unknown as Range["getClientRects"];
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}

const CONN = "c1";
const USERS: TableRef = { schema: "shop", name: "users" };
const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });

function useConnection(driver: Driver) {
  useConnectionsStore.setState({
    byId: {
      [CONN]: { info: { connectionId: CONN, driver, name: "Shop" } as ConnectionInfo },
    },
    order: [CONN],
    activeId: CONN,
  } as never);
}

/** A table tab with one staged edit (rows only). */
function rowsTab() {
  const tabId = useTabsStore.getState().openTable(CONN, USERS, "table");
  const original = [int(1), text("a")];
  useTableTabsStore.getState().ensure(tabId);
  useTableTabsStore.getState().update(tabId, {
    staging: setCell(emptyStaging(), "pk:1", 1, text("z"), original),
    stagedOn: "id\u0000name",
  });
  setRowContext(tabId, {
    table: USERS,
    columns: [
      { name: "id", isPk: true },
      { name: "name", isPk: false },
    ].map((c, i) => ({
      ...c,
      ordinal: i + 1,
      dataType: i === 0 ? "int4" : "text",
      kind: i === 0 ? "int" : "text",
      nullable: !c.isPk,
      defaultExpr: null,
      isAutoIncrement: c.isPk,
    })) as never,
    signature: "id\u0000name",
    identity: "pk",
    keys: new Map([["pk:1", { kind: "pk", value: { id: int(1) } }]]),
  });
  return tabId;
}

function previewStub(atomic: boolean) {
  return vi.spyOn(ipc.workspace, "preview").mockImplementation(async (_c, groups) => ({
    groups: groups.map((g: WorkspaceGroup) => ({
      statements: [
        g.kind === "rows"
          ? "UPDATE `users` SET `name` = 'z' WHERE `id` = 1"
          : g.kind === "definition"
            ? "CREATE VIEW v AS SELECT 1"
            : "ALTER TABLE `users` COMMENT = 'people'",
      ],
      warnings: [],
      transactional: true,
      error: null,
    })),
    atomic,
  }));
}

beforeEach(() => {
  resetPrefsForTests({ ...DEFAULT_PREFS, confirmCommit: false });
  resetCommitStore();
  resetRowContexts();
  resetDefinitions();
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useTableTabsStore.setState({ slices: {} });
  useStructureStore.setState({ byTab: {} });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

const sheet = () => screen.findByRole("dialog", { name: "Preview changes" });

describe("the Preview SQL sheet's wording on MySQL", () => {
  it("rows only: one transaction, no DDL talk, and a failure says everything was reverted", async () => {
    useConnection("mysql");
    const tabId = rowsTab();
    // The backend says "atomic" for rows only (MySQL runs them in one transaction).
    previewStub(true);
    render(<WorkspaceSheet />);
    act(() => openWorkspaceSheet(CONN));
    const dialog = await sheet();
    await waitFor(() => expect(dialog).toHaveTextContent("UPDATE `users`"));
    expect(dialog).toHaveTextContent("They run in a single transaction");
    expect(dialog).not.toHaveTextContent(/DDL/);

    act(() =>
      setFailure({
        connectionId: CONN,
        key: `rows:${tabId}`,
        message: "Duplicate entry 'x' for key 'users.email'",
        detail: null,
        statementIndex: 0,
        sql: ["UPDATE `users` SET `name` = 'z' WHERE `id` = 1"],
        atomic: true,
        restore: null,
      }),
    );
    expect(await within(dialog).findByText("All changes were reverted.")).toBeInTheDocument();
    expect(dialog).not.toHaveTextContent(/stay committed/);
  });

  it("does not mention DDL for rows-only changes even when the plan is not atomic", async () => {
    useConnection("mysql");
    rowsTab();
    // An older backend (or a plan it could not make atomic): still no talk of DDL.
    previewStub(false);
    render(<WorkspaceSheet />);
    act(() => openWorkspaceSheet(CONN));
    const dialog = await sheet();
    await waitFor(() => expect(dialog).toHaveTextContent("UPDATE `users`"));
    expect(dialog).not.toHaveTextContent(/DDL/);
    expect(dialog).not.toHaveTextContent(/item operations/);
  });

  it("with structure pending: explains that MySQL commits DDL on its own, and a failure says what stays", async () => {
    useConnection("mysql");
    const rows = rowsTab();
    useStructureStore
      .getState()
      .stage(rows, () => [{ op: "setTableComment", table: USERS, comment: "people" }]);
    previewStub(false);
    render(<WorkspaceSheet />);
    act(() => openWorkspaceSheet(CONN));
    const dialog = await sheet();
    await waitFor(() => expect(dialog).toHaveTextContent("ALTER TABLE `users`"));
    expect(dialog).toHaveTextContent("MySQL commits every DDL statement on its own");

    act(() =>
      setFailure({
        connectionId: CONN,
        key: `ddl:${rows}`,
        message: "lock timeout",
        detail: null,
        statementIndex: 0,
        sql: ["ALTER TABLE `users` COMMENT = 'people'"],
        atomic: false,
        restore: null,
      }),
    );
    expect(
      await within(dialog).findByText("Changes before it stay committed; the rest stays pending."),
    ).toBeInTheDocument();
  });
});

describe("a definition in the Preview SQL sheet", () => {
  const VIEW = "CREATE VIEW v AS SELECT 1";

  /** A New ▸ View tab with its template edited: a pending definition. */
  function viewTab() {
    const id = openSqlInNewQueryTab(CONN, VIEW, "mysql", "New view");
    markDefinition(id, { drop: null });
    applyToEditor(id, (s) => ({ changes: { from: s.doc.length, insert: "\n-- edited" } }));
    return id;
  }

  it("is a section of its own with the statements the commit would run", async () => {
    useConnection("mysql");
    const id = viewTab();
    previewStub(false);
    render(<WorkspaceSheet />);
    act(() => openWorkspaceSheet(CONN));
    const dialog = await sheet();
    const section = await within(dialog).findByRole("region", { name: "Definition New view" });
    await waitFor(() => expect(section).toHaveTextContent("CREATE VIEW v AS SELECT 1"));
    expect(section).toHaveTextContent("1 change");
    expect(dialog).toHaveTextContent("MySQL commits every DDL statement on its own");
    expect(id).toBeTruthy();
  });

  const restoreOf = (over: Partial<DefinitionRestore>): DefinitionRestore => ({
    previous: "CREATE FUNCTION `shop`.`add`(a INT) RETURNS int RETURN a",
    restored: true,
    withoutDefiner: false,
    error: null,
    ...over,
  });

  it("says that the previous version was put back when MySQL's CREATE failed after the DROP", async () => {
    useConnection("mysql");
    const id = viewTab();
    previewStub(false);
    render(<WorkspaceSheet />);
    act(() => openWorkspaceSheet(CONN));
    const dialog = await sheet();
    await within(dialog).findByRole("region", { name: "Definition New view" });
    act(() =>
      setFailure({
        connectionId: CONN,
        key: `def:${id}`,
        message: "You have an error in your SQL syntax",
        detail: null,
        statementIndex: 1,
        sql: [],
        atomic: false,
        restore: restoreOf({}),
      }),
    );
    const outcome = await within(dialog).findByTestId("restore-outcome");
    expect(outcome).toHaveTextContent("The previous version was put back; nothing changed");
    expect(within(outcome).getByRole("status")).toBeInTheDocument();
  });

  it("hands the old text back when it could not be put back", async () => {
    useConnection("mysql");
    const id = viewTab();
    previewStub(false);
    render(<WorkspaceSheet />);
    act(() => openWorkspaceSheet(CONN));
    const dialog = await sheet();
    await within(dialog).findByRole("region", { name: "Definition New view" });
    act(() =>
      setFailure({
        connectionId: CONN,
        key: `def:${id}`,
        message: "You have an error in your SQL syntax",
        detail: null,
        statementIndex: 1,
        sql: [],
        atomic: false,
        restore: restoreOf({
          restored: false,
          error: {
            code: "DB_ERROR",
            message: "CREATE command denied to user 'ro'@'%'",
            detail: null,
            hint: null,
            sqlstate: null,
            vendorCode: 1044,
            position: null,
            statementIndex: null,
            data: null,
          },
        }),
      }),
    );
    const outcome = await within(dialog).findByTestId("restore-outcome");
    expect(outcome).toHaveTextContent("could not be put back");
    expect(outcome).toHaveTextContent("CREATE command denied");
    // The text of the old routine is shown so it can be created again.
    expect(outcome).toHaveTextContent("CREATE FUNCTION `shop`.`add`(a INT) RETURNS int RETURN a");
  });
});
