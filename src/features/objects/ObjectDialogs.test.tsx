import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type DdlPlan, type ObjectOp, type RoutineRef } from "@/lib/ipc";
import { ObjectDialogView } from "./ObjectDialogs";
import { pendingObjectOps, resetPendingObjectOps } from "./pendingOps";
import type { ObjectDialog } from "./store";
import { toast } from "sonner";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() } }));
vi.mock("@/features/safety/guard", () => ({ guardExecution: vi.fn(async () => true) }));

// CodeMirror (the SQL block) measures text ranges, which jsdom does not lay out.
if (!Range.prototype.getClientRects) {
  const empty = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] });
  Range.prototype.getClientRects = empty as unknown as Range["getClientRects"];
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}

const posts = { schema: "public", name: "posts" };

function planOf(op: ObjectOp): DdlPlan {
  const cascade = op.op === "truncateTables" && op.cascade ? " CASCADE" : "";
  return { statements: [`TRUNCATE TABLE posts${cascade}`], warnings: [], transactional: true };
}

describe("staging Truncate…", () => {
  beforeEach(() => resetPendingObjectOps());
  afterEach(() => vi.restoreAllMocks());

  it("waits for the plan of the options just changed, and stages that plan", async () => {
    let release: () => void = () => undefined;
    vi.spyOn(ipc.objects, "preview").mockImplementation((_c, op) =>
      op.op === "truncateTables" && op.cascade
        ? new Promise((r) => (release = () => r(planOf(op))))
        : Promise.resolve(planOf(op)),
    );
    const onClose = vi.fn();
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ObjectDialogView
          connectionId="c1"
          dialect="postgres"
          dialog={{ kind: "truncateTables", tables: [posts] }}
          onClose={onClose}
        />
      </QueryClientProvider>,
    );
    const confirm = () => screen.getByRole("button", { name: "Truncate" });
    await waitFor(() => expect(confirm()).toBeEnabled());

    fireEvent.click(screen.getByRole("checkbox", { name: /^CASCADE/ }));
    // The previous options' SQL still shows meanwhile: not what the button would stage.
    expect(confirm()).toBeDisabled();
    fireEvent.click(confirm());
    expect(pendingObjectOps("c1")).toEqual([]);

    await act(async () => release());
    await waitFor(() => expect(confirm()).toBeEnabled());
    fireEvent.click(confirm());
    expect(pendingObjectOps("c1")).toMatchObject([
      {
        op: { op: "truncateTables", tables: [posts], cascade: true },
        statements: ["TRUNCATE TABLE posts CASCADE"],
      },
    ]);
    expect(onClose).toHaveBeenCalledOnce();
  });
});

describe("staging Delete… of routines and sequences", () => {
  const slugify: RoutineRef = {
    schema: "public",
    name: "slugify",
    kind: "function",
    identityArguments: "value text",
  };
  const seq = { schema: "public", name: "posts_id_seq" };

  /** Plans like the backend's PostgreSQL ones; the CASCADE plan waits for `release`. */
  function mockPreview() {
    const gate: { release: () => void } = { release: () => undefined };
    vi.spyOn(ipc.objects, "preview").mockImplementation((_c, op) => {
      const cascade = "cascade" in op && op.cascade ? " CASCADE" : "";
      const statements =
        op.op === "dropRoutines"
          ? op.routines.map((r) => `DROP FUNCTION "public"."${r.name}"(${r.identityArguments})`)
          : op.op === "dropSequences"
            ? op.sequences.map((s) => `DROP SEQUENCE "public"."${s.name}"`)
            : [];
      const plan: DdlPlan = {
        statements: statements.map((s) => `${s}${cascade}`),
        warnings: [],
        transactional: true,
      };
      return cascade ? new Promise((r) => (gate.release = () => r(plan))) : Promise.resolve(plan);
    });
    return gate;
  }

  function renderDialog(dialog: ObjectDialog, onClose = vi.fn()) {
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ObjectDialogView connectionId="c1" dialect="postgres" dialog={dialog} onClose={onClose} />
      </QueryClientProvider>,
    );
    return onClose;
  }

  beforeEach(() => resetPendingObjectOps());
  afterEach(() => vi.restoreAllMocks());

  it("stages a routine drop with the plan of its CASCADE option, and runs nothing", async () => {
    const gate = mockPreview();
    const apply = vi.spyOn(ipc.objects, "apply");
    const onClose = renderDialog({
      kind: "dropRoutines",
      routines: [slugify],
      names: ["slugify(value text)"],
    });
    expect(screen.getByRole("dialog", { name: "Delete function slugify?" })).toHaveTextContent(
      "public.slugify(value text) is dropped when you commit.",
    );
    const confirm = () => screen.getByRole("button", { name: "Delete" });
    await waitFor(() => expect(confirm()).toBeEnabled());

    fireEvent.click(screen.getByRole("checkbox", { name: /^CASCADE/ }));
    // The previous options' SQL still shows meanwhile: not what the button would stage.
    expect(confirm()).toBeDisabled();
    fireEvent.click(confirm());
    expect(pendingObjectOps("c1")).toEqual([]);

    await act(async () => gate.release());
    await waitFor(() => expect(confirm()).toBeEnabled());
    fireEvent.click(confirm());
    expect(pendingObjectOps("c1")).toMatchObject([
      {
        kind: "drop",
        op: { op: "dropRoutines", routines: [slugify], cascade: true },
        statements: ['DROP FUNCTION "public"."slugify"(value text) CASCADE'],
      },
    ]);
    expect(onClose).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
  });

  it("stages a sequence drop", async () => {
    mockPreview();
    const apply = vi.spyOn(ipc.objects, "apply");
    const onClose = renderDialog({ kind: "dropSequences", sequences: [seq] });
    screen.getByRole("dialog", { name: "Delete sequence posts_id_seq?" });
    const confirm = screen.getByRole("button", { name: "Delete" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);
    expect(pendingObjectOps("c1")).toMatchObject([
      {
        kind: "drop",
        op: { op: "dropSequences", sequences: [seq], cascade: false },
        statements: ['DROP SEQUENCE "public"."posts_id_seq"'],
      },
    ]);
    expect(onClose).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
  });
});

describe("keyboard and Undo", () => {
  function renderDialog(dialog: ObjectDialog, onClose = vi.fn()) {
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ObjectDialogView connectionId="c1" dialect="postgres" dialog={dialog} onClose={onClose} />
      </QueryClientProvider>,
    );
    return onClose;
  }

  beforeEach(() => {
    resetPendingObjectOps();
    vi.mocked(toast.message).mockReset();
  });
  afterEach(() => vi.restoreAllMocks());

  it("focuses Truncate once its SQL is ready (never Cancel or an option), and Return stages it", async () => {
    vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => planOf(op));
    const onClose = renderDialog({ kind: "truncateTables", tables: [posts] });
    const confirm = screen.getByRole("button", { name: "Truncate" });
    expect(screen.getByRole("button", { name: "Cancel" })).not.toHaveFocus();
    await waitFor(() => expect(confirm).toHaveFocus());
    // Return on the focused default button submits the form.
    fireEvent.submit(confirm.closest("form")!);
    expect(pendingObjectOps("c1")).toHaveLength(1);
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("does not submit on Return while the SQL of the options just changed is loading", async () => {
    let release: () => void = () => undefined;
    vi.spyOn(ipc.objects, "preview").mockImplementation((_c, op) =>
      op.op === "truncateTables" && op.cascade
        ? new Promise((r) => (release = () => r(planOf(op))))
        : Promise.resolve(planOf(op)),
    );
    renderDialog({ kind: "truncateTables", tables: [posts] });
    const confirm = screen.getByRole("button", { name: "Truncate" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(screen.getByRole("checkbox", { name: /^CASCADE/ }));
    fireEvent.submit(confirm.closest("form")!);
    expect(pendingObjectOps("c1")).toEqual([]);
    await act(async () => release());
  });

  it("the pending message's Undo takes back just that op", async () => {
    vi.spyOn(ipc.objects, "preview").mockImplementation(async (_c, op) => planOf(op));
    renderDialog({ kind: "truncateTables", tables: [posts] });
    const confirm = screen.getByRole("button", { name: "Truncate" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);
    expect(pendingObjectOps("c1")).toHaveLength(1);
    expect(toast.message).toHaveBeenCalledWith("Truncate posts is pending", expect.anything());
    const { action } = vi.mocked(toast.message).mock.calls[0]![1] as {
      action: { label: string; onClick: () => void };
    };
    expect(action.label).toBe("Undo");
    act(() => action.onClick());
    expect(pendingObjectOps("c1")).toEqual([]);
  });

  it("Return in Clone's name field clones", async () => {
    vi.spyOn(ipc.objects, "preview").mockResolvedValue({
      statements: ["CREATE TABLE posts_copy (LIKE posts)"],
      warnings: [],
      transactional: true,
    });
    const apply = vi.spyOn(ipc.objects, "apply").mockResolvedValue({ error: null } as never);
    const onClose = renderDialog({ kind: "clone", table: posts } as ObjectDialog);
    const input = screen.getByRole("textbox", { name: "New table name" });
    expect(input).toHaveFocus();
    await waitFor(() => expect(screen.getByRole("button", { name: "Clone" })).toBeEnabled());
    // Focus stays in the field the user is typing in.
    expect(input).toHaveFocus();
    fireEvent.submit(input.closest("form")!);
    await waitFor(() => expect(apply).toHaveBeenCalledOnce());
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });
});
