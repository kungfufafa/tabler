import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type DdlPlan, type ObjectOp } from "@/lib/ipc";
import { ObjectDialogView } from "./ObjectDialogs";
import { pendingObjectOps, resetPendingObjectOps } from "./pendingOps";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() } }));

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
