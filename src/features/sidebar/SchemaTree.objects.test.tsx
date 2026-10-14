import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useDiagramStore } from "@/features/diagram/store";
import { closeImport, useImportStore } from "@/features/import/store";
import { ObjectsHost } from "@/features/objects/ObjectsHost";
import { pendingObjectOps, resetPendingObjectOps } from "@/features/objects/pendingOps";
import { closeItemOverview, closeObjectDialog, useObjectsStore } from "@/features/objects/store";
import { actions } from "@/features/palette/actions";
import { guardExecution } from "@/features/safety/guard";
import {
  ipc,
  type ConnectionInfo,
  type DdlPlan,
  type Driver,
  type ObjectOp,
  type RoutineSummary,
  type TableSummary,
  type WorkspaceGroup,
  type WorkspaceResult,
} from "@/lib/ipc";
import { isMac } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { createGroup, reloadGroups, useGroupDialogStore, useGroups } from "./groups";
import { SchemaTree } from "./SchemaTree";
import { reloadSidebarPrefs, setSidebarOption, useSidebarPrefs } from "./sidebarPrefs";
import { buildTreeRows, useTreeRows } from "./useSchemaTree";

vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn(async () => true) }));

function table(name: string, kind: TableSummary["kind"] = "table"): TableSummary {
  return { schema: "public", name, kind, rowEstimate: null, sizeBytes: null, comment: null };
}

const TABLES = [table("users"), table("posts"), table("tags"), table("user_stats", "view")];

function routine(name: string, kind: RoutineSummary["kind"] = "function"): RoutineSummary {
  return {
    schema: "public",
    name,
    kind,
    arguments: "value text",
    identityArguments: "value text",
    returnType: kind === "procedure" ? null : "text",
    language: "sql",
    comment: null,
  };
}

const MOD = isMac ? { metaKey: true } : { ctrlKey: true };

function connect(driver: Driver = "postgres") {
  const info: ConnectionInfo = {
    connectionId: "c1",
    profileId: "p1",
    name: "Local",
    color: null,
    driver,
    serverVersion: "17.6",
    currentDatabase: "tabler_dev",
    currentUser: "tabler",
    tunnel: null,
    readOnly: false,
  };
  useConnectionsStore.setState({
    byId: { c1: { info, status: "connected", message: null, currentSchema: "public" } },
    order: ["c1"],
    activeId: "c1",
  });
}

const SCOPE = "p1|tabler_dev";

/** The tree as the sidebar builds it, with the groups of `SCOPE`. */
function GroupedTree() {
  const groups = useGroups(SCOPE);
  const rows = useTreeRows(TABLES, "", undefined, undefined, undefined, groups);
  return (
    <SchemaTree
      connectionId="c1"
      rows={rows}
      loading={false}
      error={null}
      search=""
      pinScope={SCOPE}
      groups={groups}
    />
  );
}

function renderTree({ grouped = false }: { grouped?: boolean } = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      {grouped ? (
        <GroupedTree />
      ) : (
        <SchemaTree
          connectionId="c1"
          rows={buildTreeRows(TABLES, "")}
          loading={false}
          error={null}
          search=""
          pinScope={SCOPE}
        />
      )}
      <ObjectsHost />
    </QueryClientProvider>,
  );
}

const row = (name: string) => screen.getAllByRole("button", { name }).at(-1)!;
const sqlPreview = (dialog: HTMLElement) => within(dialog).getByLabelText("SQL preview");
const selected = () =>
  [...document.querySelectorAll<HTMLElement>("[data-selected]")].map((el) => el.textContent);

function planOf(op: ObjectOp): DdlPlan {
  const statements =
    op.op === "dropTables"
      ? op.tables.map((t) => `DROP TABLE "public"."${t.table.name}"${op.cascade ? " CASCADE" : ""}`)
      : op.op === "truncateTables"
        ? op.tables.map((t) => `TRUNCATE TABLE "public"."${t.name}"`)
        : op.op === "dropRoutines"
          ? op.routines.map(
              (r) =>
                `DROP ${r.kind === "procedure" ? "PROCEDURE" : "FUNCTION"} "public"."${r.name}"(${r.identityArguments})${op.cascade ? " CASCADE" : ""}`,
            )
          : op.op === "dropSequences"
            ? op.sequences.map(
                (s) => `DROP SEQUENCE "public"."${s.name}"${op.cascade ? " CASCADE" : ""}`,
              )
            : [`-- ${op.op}`];
  return { statements, warnings: [], transactional: true };
}

/**
 * The workspace commit (⌘S): previews every object group with `planOf`; `apply` runs them all
 * (`results` per group, in request order, when the test says how each went).
 */
function stubWorkspace(results?: (groups: WorkspaceGroup[]) => WorkspaceResult) {
  vi.spyOn(ipc.workspace, "preview").mockImplementation(async (_c, groups) => ({
    groups: groups.map((g) => ({
      ...(g.kind === "object"
        ? planOf(g.op)
        : { statements: [], warnings: [], transactional: true }),
      error: null,
    })),
    atomic: true,
  }));
  return vi.spyOn(ipc.workspace, "apply").mockImplementation(async (_c, groups) =>
    results
      ? results(groups)
      : {
          success: true,
          atomic: true,
          groups: groups.map((g) => {
            const sql = g.kind === "object" ? planOf(g.op).statements : [];
            return {
              status: "applied" as const,
              sql,
              executed: sql,
              failedIndex: null,
              rows: [],
              routine: null,
              error: null,
            };
          }),
          failedGroup: null,
          error: null,
          durationMs: 4,
        },
  );
}

describe("SchemaTree objects", () => {
  beforeEach(() => {
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(600);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(240);
    localStorage.clear();
    reloadSidebarPrefs();
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    closeObjectDialog();
    closeItemOverview();
    resetPendingObjectOps();
    reloadGroups();
    useDiagramStore.setState({ request: null });
    closeImport();
    connect();
    vi.spyOn(ipc.schema, "schemas").mockResolvedValue([{ name: "public", isDefault: true }]);
    // In the server's (name) order.
    vi.spyOn(ipc.objects, "routines").mockResolvedValue([
      routine("archive_posts", "procedure"),
      routine("slugify"),
    ]);
    vi.spyOn(ipc.objects, "sequences").mockResolvedValue([]);
    vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => planOf(op));
    vi.mocked(guardExecution).mockResolvedValue(true);
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists functions and procedures before the tables, and opens one in a SQL tab", async () => {
    vi.spyOn(ipc.objects, "routineDefinition").mockResolvedValue(
      "CREATE OR REPLACE FUNCTION public.slugify(value text)\n RETURNS text\nAS $$ SELECT value $$;\n",
    );
    renderTree();
    const header = await screen.findByRole("button", { name: /^Functions/ });
    expect(header).toHaveTextContent("Functions2");
    const labels = screen
      .getAllByRole("button")
      .filter((b) => b.dataset.rowIndex !== undefined)
      .map((b) => b.textContent);
    expect(labels.slice(0, 4)).toEqual(["Functions2", "archive_posts", "slugify", "Tables3"]);

    fireEvent.click(row("slugify"), { detail: 1 });
    await waitFor(() =>
      expect(useTabsStore.getState().tabs).toEqual([
        expect.objectContaining({ kind: "query", title: "slugify" }),
      ]),
    );
    expect(ipc.objects.routineDefinition).toHaveBeenCalledWith("c1", {
      schema: "public",
      name: "slugify",
      kind: "function",
      identityArguments: "value text",
    });
    // The second click of a double-click opens nothing more.
    fireEvent.click(row("slugify"), { detail: 2 });
    expect(ipc.objects.routineDefinition).toHaveBeenCalledOnce();
  });

  it("selects with ⌘-click, ⇧-click and ⌘A", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.click(row("users"));
    expect(selected()).toEqual(["users"]);
    fireEvent.click(row("tags"), MOD);
    expect(selected()).toEqual(["users", "tags"]);
    // The open table is the one filled row; the other selected rows get a lighter fill.
    expect(row("users")).toHaveClass("bg-sidebar-accent");
    expect(row("tags")).not.toHaveClass("bg-sidebar-accent");
    // ⇧-click: the range from the last ⌘-clicked row.
    fireEvent.click(row("posts"), { shiftKey: true });
    expect(row("users")).not.toHaveAttribute("data-selected");
    expect(row("posts")).toHaveAttribute("data-selected");
    expect(row("tags")).toHaveAttribute("data-selected");
    // ⌘-click toggles one off; only the clicked table opened.
    fireEvent.click(row("posts"), MOD);
    expect(row("posts")).not.toHaveAttribute("data-selected");
    expect(useTabsStore.getState().tabs.map((t) => t.title)).toEqual(["users"]);

    row("tags").focus();
    fireEvent.keyDown(row("tags"), { key: "a", ...MOD });
    for (const name of ["slugify", "archive_posts", "users", "posts", "tags", "user_stats"])
      expect(row(name)).toHaveAttribute("data-selected");

    // The menu of a selected row acts on the whole selection (tables and views here).
    fireEvent.contextMenu(row("tags"));
    expect(screen.getByRole("menuitem", { name: "Copy 4 Names" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /^Delete 4 Items…/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /^Truncate 3 Items…/ })).toBeInTheDocument();
  });

  it("⌫ stages dropping every selected table; ⌘S commits it after the safe mode", async () => {
    const apply = stubWorkspace();
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.click(row("posts"));
    fireEvent.click(row("tags"), MOD);
    fireEvent.keyDown(row("tags"), { key: "Backspace" });
    const dialog = await screen.findByRole("dialog", { name: "Delete 2 items?" });
    expect(within(dialog).getByRole("list", { name: "Items" })).toHaveTextContent("poststags");
    await waitFor(() =>
      expect(sqlPreview(dialog)).toHaveTextContent('DROP TABLE "public"."posts"; DROP TABLE'),
    );
    expect(within(dialog).getByRole("checkbox")).not.toBeChecked();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // Staged, not run: the rows are struck through until the commit.
    expect(apply).not.toHaveBeenCalled();
    expect(guardExecution).not.toHaveBeenCalled();
    expect(pendingObjectOps("c1")).toHaveLength(1);
    expect(row("posts")).toHaveAttribute("data-pending", "drop");
    expect(row("tags")).toHaveAttribute("data-pending", "drop");
    // A table already pending a drop is not offered again.
    fireEvent.contextMenu(row("posts"));
    expect(screen.getByRole("menuitem", { name: /^Delete…/ })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });

    // ⌘S (no tab has staged changes): the operations alone, after the safe mode.
    await actions.run("grid.commit");
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    expect(guardExecution).toHaveBeenCalledWith("c1", {
      kind: "commit",
      statements: ['DROP TABLE "public"."posts"', 'DROP TABLE "public"."tags"'],
    });
    expect(apply.mock.calls[0]![1]).toEqual([
      {
        kind: "object",
        op: {
          op: "dropTables",
          tables: [
            { table: { schema: "public", name: "posts" }, kind: "table" },
            { table: { schema: "public", name: "tags" }, kind: "table" },
          ],
          cascade: false,
          ignoreForeignKeys: false,
        },
      },
    ]);
    await waitFor(() => expect(pendingObjectOps("c1")).toHaveLength(0));
  });

  it("⇧⌫ stages a CASCADE drop and ⌥⌫ a truncate without a dialog; a declined safe mode keeps them pending", async () => {
    const apply = stubWorkspace();
    const message = vi.spyOn(toast, "message");
    vi.mocked(guardExecution).mockResolvedValue(false);
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.keyDown(row("users"), { key: "Backspace", shiftKey: true });
    // TablePlus: the shortcut skips the dialog.
    await waitFor(() => expect(row("users")).toHaveAttribute("data-pending", "drop"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(useObjectsStore.getState().dialog).toBeNull();
    expect(message).toHaveBeenCalledWith(
      "Staged: drop table users CASCADE",
      expect.objectContaining({ description: expect.stringMatching(/to commit, .* to discard/) }),
    );

    // Views cannot be truncated: ⌥⌫ on one does nothing.
    fireEvent.keyDown(row("user_stats"), { key: "Backspace", altKey: true });
    expect(useObjectsStore.getState().dialog).toBeNull();
    fireEvent.keyDown(row("tags"), { key: "Backspace", altKey: true });
    await waitFor(() => expect(row("tags")).toHaveAttribute("data-pending", "truncate"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(pendingObjectOps("c1")).toHaveLength(2);
    expect(within(row("tags")).getByText("Truncate")).toBeInTheDocument();

    await actions.run("grid.commit");
    await waitFor(() => expect(guardExecution).toHaveBeenCalledOnce());
    expect(vi.mocked(guardExecution).mock.calls[0]![1]).toEqual({
      kind: "commit",
      statements: ['DROP TABLE "public"."users" CASCADE', 'TRUNCATE TABLE "public"."tags"'],
    });
    expect(apply).not.toHaveBeenCalled();
    expect(pendingObjectOps("c1").map((o) => o.kind)).toEqual(["drop", "truncate"]);

    // ⇧⌘⌫ (Discard) drops them.
    await actions.run("grid.discard");
    expect(pendingObjectOps("c1")).toEqual([]);
    expect(row("tags")).not.toHaveAttribute("data-pending");
  });

  it("stops at the first failing operation and keeps it and the rest pending", async () => {
    const error = {
      code: "DB",
      message: "permission denied",
      detail: null,
      hint: null,
      sqlstate: null,
      vendorCode: null,
      position: null,
      statementIndex: 0,
      data: null,
    };
    // A database that commits DDL statement by statement (MySQL): the first op ran, the second
    // failed, the third never started.
    const apply = stubWorkspace((groups) => {
      const sql = (i: number) =>
        groups[i]!.kind === "object" ? planOf(groups[i]!.op).statements : [];
      return {
        success: false,
        atomic: false,
        groups: [
          {
            status: "applied",
            sql: sql(0),
            executed: sql(0),
            failedIndex: null,
            rows: [],
            routine: null,
            error: null,
          },
          {
            status: "failed",
            sql: sql(1),
            executed: [],
            failedIndex: 0,
            rows: [],
            routine: null,
            error,
          },
          {
            status: "notRun",
            sql: sql(2),
            executed: [],
            failedIndex: null,
            rows: [],
            routine: null,
            error: null,
          },
        ],
        failedGroup: 1,
        error,
        durationMs: 1,
      } as WorkspaceResult;
    });
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    for (const name of ["users", "posts", "tags"]) {
      fireEvent.click(row(name));
      fireEvent.keyDown(row(name), { key: "Backspace", altKey: true });
      await waitFor(() => expect(row(name)).toHaveAttribute("data-pending", "truncate"));
    }
    await actions.run("grid.commit");
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    await waitFor(() => expect(pendingObjectOps("c1")).toHaveLength(2));
    const left = pendingObjectOps("c1");
    expect(left.map((o) => o.op.op === "truncateTables" && o.op.tables[0]!.name)).toEqual([
      "posts",
      "tags",
    ]);
    expect(left[0]!.error).toBe("permission denied");
    expect(left[1]!.error).toBeNull();
  });

  it("previews the pending operations in a sheet, where one can be taken out", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.keyDown(row("tags"), { key: "Backspace" });
    const dialog = await screen.findByRole("dialog", { name: "Delete table tags?" });
    await waitFor(() => expect(sqlPreview(dialog)).toHaveTextContent("DROP TABLE"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    fireEvent.keyDown(row("users"), { key: "Backspace", altKey: true });
    await waitFor(() => expect(row("users")).toHaveAttribute("data-pending", "truncate"));

    stubWorkspace();
    await actions.run("grid.preview");
    const sheet = await screen.findByRole("dialog", { name: "Preview changes" });
    const ops = within(sheet).getByRole("region", { name: "Sidebar Item operations" });
    await waitFor(() => expect(ops).toHaveTextContent('Delete tagsDROP TABLE "public"."tags";'));
    expect(ops).toHaveTextContent('Truncate usersTRUNCATE TABLE "public"."users";');
    fireEvent.click(within(sheet).getByRole("button", { name: "Remove delete of tags" }));
    expect(pendingObjectOps("c1").map((o) => o.kind)).toEqual(["truncate"]);
  });

  it("⌫ on functions and sequences stages their drop: struck through until ⌘S", async () => {
    const apply = stubWorkspace();
    vi.mocked(ipc.objects.sequences).mockResolvedValue([
      {
        schema: "public",
        name: "posts_id_seq",
        dataType: "bigint",
        ownedBy: "posts.id",
        comment: null,
      },
    ]);
    setSidebarOption("showSequences", true);
    renderTree();
    await screen.findByRole("button", { name: /^Sequences/ });
    expect(ipc.objects.routines).toHaveBeenCalledTimes(1);

    // Both routines (⌫ acts on the selection), with CASCADE.
    fireEvent.keyDown(row("slugify"), { key: "a", ...MOD });
    fireEvent.keyDown(row("slugify"), { key: "Backspace" });
    let dialog = await screen.findByRole("dialog", { name: "Delete 2 routines?" });
    fireEvent.click(within(dialog).getByRole("checkbox", { name: /CASCADE/ }));
    await waitFor(() =>
      expect(sqlPreview(dialog)).toHaveTextContent(
        'DROP PROCEDURE "public"."archive_posts"(value text) CASCADE;',
      ),
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // The sequence from its menu.
    fireEvent.contextMenu(row("posts_id_seq"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Delete…/ }));
    dialog = await screen.findByRole("dialog", { name: "Delete sequence posts_id_seq?" });
    await waitFor(() => expect(sqlPreview(dialog)).toHaveTextContent("DROP SEQUENCE"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // Staged, not run: struck through like a table pending a drop.
    expect(apply).not.toHaveBeenCalled();
    expect(pendingObjectOps("c1").map((o) => o.op.op)).toEqual(["dropRoutines", "dropSequences"]);
    for (const name of ["slugify", "archive_posts", "posts_id_seq"]) {
      expect(row(name)).toHaveAttribute("data-pending", "drop");
      expect(within(row(name)).getByText(name)).toHaveClass("line-through");
    }
    expect(row("users")).not.toHaveAttribute("data-pending");
    // Not offered a second time: neither the menu nor ⌫ opens another dialog.
    fireEvent.contextMenu(row("posts_id_seq"));
    expect(screen.getByRole("menuitem", { name: /^Delete…/ })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    fireEvent.keyDown(row("slugify"), { key: "Backspace" });
    expect(useObjectsStore.getState().dialog).toBeNull();

    // ⌘S runs them after the safe mode, then refetches the routine list.
    await actions.run("grid.commit");
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    expect(apply.mock.calls[0]![1].map((g) => g.kind)).toEqual(["object", "object"]);
    expect(vi.mocked(guardExecution).mock.calls[0]![1]).toEqual({
      kind: "commit",
      statements: [
        'DROP PROCEDURE "public"."archive_posts"(value text) CASCADE',
        'DROP FUNCTION "public"."slugify"(value text) CASCADE',
        'DROP SEQUENCE "public"."posts_id_seq"',
      ],
    });
    await waitFor(() => expect(pendingObjectOps("c1")).toEqual([]));
    await waitFor(() => expect(ipc.objects.routines).toHaveBeenCalledTimes(2));
    expect(row("slugify")).not.toHaveAttribute("data-pending");
  });

  it("lists a table's columns when Show Columns is on and the row is expanded", async () => {
    setSidebarOption("showColumns", true);
    const meta = vi.spyOn(ipc.schema, "table").mockResolvedValue({
      table: { schema: "public", name: "posts" },
      kind: "table",
      columns: [
        { name: "id", ordinal: 1, dataType: "bigint", isPk: true },
        { name: "user_id", ordinal: 2, dataType: "bigint", isPk: false },
        { name: "title", ordinal: 3, dataType: "text", isPk: false },
      ],
      primaryKey: ["id"],
      foreignKeys: [{ columns: ["user_id"] }],
    } as never);
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    expect(row("posts")).toHaveAttribute("aria-expanded", "false");
    expect(meta).not.toHaveBeenCalled();
    // → expands, the chevron toggles.
    fireEvent.keyDown(row("posts"), { key: "ArrowRight" });
    expect(row("posts")).toHaveAttribute("aria-expanded", "true");
    const title = await screen.findByRole("button", { name: "titletext" });
    expect(meta).toHaveBeenCalledWith("c1", { schema: "public", name: "posts" });
    expect(screen.getByRole("button", { name: "Primary keyidbigint" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Foreign keyuser_idbigint" })).toBeInTheDocument();
    // Clicking a column opens its table.
    fireEvent.click(title);
    expect(useTabsStore.getState().tabs).toEqual([
      expect.objectContaining({ title: "posts", preview: true }),
    ]);
    fireEvent.keyDown(row("posts"), { key: "ArrowLeft" });
    expect(screen.queryByRole("button", { name: "titletext" })).toBeNull();
    const chevron = row("posts").querySelector<HTMLElement>("[data-slot=disclosure]")!;
    fireEvent.click(chevron);
    expect(await screen.findByRole("button", { name: "titletext" })).toBeInTheDocument();
    // Turned off: no chevrons, no columns.
    act(() => setSidebarOption("showColumns", false));
    expect(screen.queryByRole("button", { name: "titletext" })).toBeNull();
    expect(row("posts")).not.toHaveAttribute("aria-expanded");
  });

  it("shows groups above Tables; their items leave the regular sections", async () => {
    createGroup(SCOPE, "Tagging", [{ field: "name", op: "prefix", value: "tag" }]);
    const manual = createGroup(SCOPE, "Archive", null, ["t:public.posts"]);
    renderTree({ grouped: true });
    await screen.findByRole("button", { name: /^Functions/ });
    const labels = screen
      .getAllByRole("button")
      .filter((b) => b.dataset.rowIndex !== undefined)
      .map((b) => b.textContent);
    expect(labels).toEqual([
      "Tagging1",
      "tags",
      "Archive1",
      "posts",
      "Functions2",
      "archive_posts",
      "slugify",
      "Tables1",
      "users",
      "Views1",
      "user_stats",
    ]);
    // Add to Group from the menu moves the item into the manual group.
    fireEvent.contextMenu(row("users"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Add to Group" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Archive" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^Archive/ })).toHaveTextContent("Archive2"),
    );
    expect(screen.queryByRole("button", { name: /^Tables/ })).toBeNull();
    // The group header collapses on click and offers Edit / Delete.
    fireEvent.click(screen.getByRole("button", { name: /^Archive/ }));
    expect(screen.queryByRole("button", { name: "posts" })).toBeNull();
    fireEvent.contextMenu(screen.getByRole("button", { name: /^Archive/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete Group" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "posts" })).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: /^Archive/ })).toBeNull();
    expect(manual.id).toBeTruthy();
  });

  it("marks a group whose stored regex is unsafe, and keeps it empty", async () => {
    // Saved before the regex check existed: kept, but switched off and flagged on its header.
    createGroup(SCOPE, "Risky", [{ field: "name", op: "regex", value: "(\\w+)+x$" }]);
    renderTree({ grouped: true });
    await screen.findByRole("button", { name: /^Functions/ });
    const header = screen
      .getAllByRole("button")
      .find((b) => b.dataset.group !== undefined && b.textContent?.startsWith("Risky"));
    expect(header).toBeDefined();
    expect(header).toHaveTextContent("Risky0");
    expect(within(header!).getByLabelText(/This group is off/)).toBeInTheDocument();
    expect(header).toHaveAttribute("title", expect.stringMatching(/This group is off/));
  });

  it("drops dragged rows into a manual group", async () => {
    createGroup(SCOPE, "Archive", null);
    renderTree({ grouped: true });
    await screen.findByRole("button", { name: /^Functions/ });
    const data = new Map<string, string>();
    const dataTransfer = {
      setData: (k: string, v: string) => data.set(k, v),
      getData: (k: string) => data.get(k) ?? "",
      get types() {
        return [...data.keys()];
      },
      effectAllowed: "",
      dropEffect: "",
    };
    fireEvent.click(row("users"));
    fireEvent.click(row("slugify"), MOD);
    fireEvent.dragStart(row("slugify"), { dataTransfer });
    const header = screen.getByRole("button", { name: /^Archive/ });
    fireEvent.dragOver(header, { dataTransfer });
    expect(header).toHaveAttribute("data-drop-target");
    fireEvent.drop(header, { dataTransfer });
    await waitFor(() => expect(header).toHaveTextContent("Archive2"));
    expect(header).not.toHaveAttribute("data-drop-target");
  });

  it("generates a diagram of the selected tables, or of the whole schema", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.click(row("users"));
    fireEvent.click(row("tags"), MOD);
    fireEvent.contextMenu(row("tags"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Generate Diagram…" }));
    expect(useDiagramStore.getState().request).toEqual({
      connectionId: "c1",
      schema: "public",
      tables: [
        { schema: "public", name: "users" },
        { schema: "public", name: "tags" },
      ],
    });
    fireEvent.contextMenu(screen.getByRole("button", { name: /^Tables/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Generate Diagram…" }));
    expect(useDiagramStore.getState().request).toEqual({
      connectionId: "c1",
      schema: "public",
      tables: null,
    });
  });

  it("opens the Item Overview from the Tables header", async () => {
    vi.spyOn(ipc.objects, "overview").mockResolvedValue([
      {
        name: "users",
        schema: "public",
        kind: "table",
        estimatedRows: 1200,
        totalSize: 65536,
        dataSize: 49152,
        indexSize: 16384,
        engine: null,
        charset: null,
        collation: null,
        owner: "tabler",
        comment: "Registered accounts",
      },
    ]);
    renderTree();
    fireEvent.contextMenu(await screen.findByRole("button", { name: /^Tables/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Item Overview" }));
    // It opens as a tab of the workspace (ItemOverview.test.tsx renders its content).
    const { tabs, activeId } = useTabsStore.getState();
    expect(tabs.find((t) => t.id === activeId)).toMatchObject({
      kind: "tool",
      connectionId: "c1",
      tool: "overview",
      params: { schema: "public" },
    });
  });

  it("refetches the routines (and tables) whenever view.refresh is dispatched", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    expect(ipc.objects.routines).toHaveBeenCalledTimes(1);
    // ⌘R, the toolbar's Refresh and View ▸ Refresh all dispatch the action; nothing needs to
    // handle it for the sidebar to reload.
    await actions.run("view.refresh");
    await waitFor(() => expect(ipc.objects.routines).toHaveBeenCalledTimes(2));
    await actions.run("tab.new-query");
    expect(ipc.objects.routines).toHaveBeenCalledTimes(2);
  });

  it("⌘⌫ on MySQL stages a drop without foreign key checks; the toast's Undo takes it out", async () => {
    connect("mysql");
    const message = vi.spyOn(toast, "message");
    renderTree();
    await screen.findByRole("button", { name: /^Tables/ });
    fireEvent.click(row("posts"));
    fireEvent.click(row("tags"), MOD);
    fireEvent.keyDown(row("tags"), { key: "Backspace", ...MOD });
    await waitFor(() => expect(pendingObjectOps("c1")).toHaveLength(1));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(pendingObjectOps("c1")[0]!.op).toEqual({
      op: "dropTables",
      tables: [
        { table: { schema: "public", name: "posts" }, kind: "table" },
        { table: { schema: "public", name: "tags" }, kind: "table" },
      ],
      cascade: false,
      ignoreForeignKeys: true,
    });
    const [title, options] = message.mock.calls.at(-1)!;
    expect(title).toBe("Staged: drop 2 tables without foreign key checks");
    const undo = (options as { action: { label: string; onClick: () => void } }).action;
    expect(undo.label).toBe("Undo");
    act(() => undo.onClick());
    expect(pendingObjectOps("c1")).toEqual([]);
    expect(row("posts")).not.toHaveAttribute("data-pending");
  });

  it("⇧⌫ on a function stages its CASCADE drop right away; plain ⌫ still asks", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Functions/ });
    fireEvent.keyDown(row("slugify"), { key: "Backspace", shiftKey: true });
    await waitFor(() => expect(row("slugify")).toHaveAttribute("data-pending", "drop"));
    expect(pendingObjectOps("c1")[0]!.op).toMatchObject({ op: "dropRoutines", cascade: true });
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.keyDown(row("archive_posts"), { key: "Backspace" });
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    act(() => closeObjectDialog());
    // Read-only connections stage nothing.
    resetPendingObjectOps();
    act(() =>
      useConnectionsStore.setState((st) => ({
        byId: { c1: { ...st.byId.c1!, info: { ...st.byId.c1!.info, readOnly: true } } },
      })),
    );
    fireEvent.keyDown(row("users"), { key: "Backspace", altKey: true });
    await act(async () => {});
    expect(pendingObjectOps("c1")).toEqual([]);
  });

  it("a click on the Tables label opens the Item Overview; its arrow collapses the section", async () => {
    renderTree();
    const header = await screen.findByRole("button", { name: /^Tables/ });
    fireEvent.click(within(header).getByText("Tables"), { detail: 1 });
    const { tabs, activeId } = useTabsStore.getState();
    expect(tabs.find((t) => t.id === activeId)).toMatchObject({
      kind: "tool",
      tool: "overview",
      params: { schema: "public" },
    });
    expect(header).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(header.querySelector('[data-slot="disclosure"]')!, { detail: 1 });
    // (These rows are built once, without the collapsed sections: check the preference.)
    expect(useSidebarPrefs.getState().collapsed.has("Tables")).toBe(true);
    expect(useTabsStore.getState().tabs).toHaveLength(1);
    // Other headers (Functions) collapse on a click.
    const functions = screen.getByRole("button", { name: /^Functions/ });
    fireEvent.click(within(functions).getByText("Functions"), { detail: 1 });
    expect(screen.getByRole("button", { name: /^Functions/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(useTabsStore.getState().tabs).toHaveLength(1);
  });

  it("imports into a table From JSON… (Import ▸)", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Tables/ });
    fireEvent.contextMenu(row("users"));
    fireEvent.click(screen.getByRole("menuitem", { name: /Import/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "From JSON…" }));
    expect(useImportStore.getState().request).toEqual({
      connectionId: "c1",
      kind: "json",
      table: { schema: "public", name: "users" },
    });
  });

  it("offers New ▸ Group… in the item menu", async () => {
    renderTree();
    await screen.findByRole("button", { name: /^Tables/ });
    fireEvent.contextMenu(row("users"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^New/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Group…" }));
    expect(useGroupDialogStore.getState().request).toMatchObject({ scope: SCOPE });
  });

  it("hides the Functions section when it is turned off, and on SQLite", async () => {
    localStorage.setItem("tabler.sidebar.options.v2", JSON.stringify({ showFunctions: false }));
    reloadSidebarPrefs();
    const { unmount } = renderTree();
    await screen.findByRole("button", { name: /^Tables/ });
    expect(screen.queryByRole("button", { name: /^Functions/ })).toBeNull();
    unmount();
    localStorage.clear();
    reloadSidebarPrefs();
    connect("sqlite");
    renderTree();
    await screen.findByRole("button", { name: /^Tables/ });
    expect(screen.queryByRole("button", { name: /^Functions/ })).toBeNull();
    expect(ipc.objects.routines).not.toHaveBeenCalled();
  });
});
