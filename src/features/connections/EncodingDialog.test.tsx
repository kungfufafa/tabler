import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { ipc } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { isUtf8Encoding } from "./encoding";
import { ConnectionExtrasHost } from "./EncodingDialog";
import { profileKeys } from "./useConnections";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

const MY = SEED_PROFILES.find((p) => p.driver === "mysql" && !p.ssh)!;
const LITE = SEED_PROFILES.find((p) => p.driver === "sqlite")!;

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});

afterEach(() => {
  vi.mocked(toast.success).mockClear();
  for (const id of useConnectionsStore.getState().order) useConnectionsStore.getState().remove(id);
});

function renderHost() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(profileKeys.all, SEED_PROFILES);
  render(
    <QueryClientProvider client={qc}>
      <ConnectionExtrasHost />
    </QueryClientProvider>,
  );
  return { qc };
}

describe("Connection ▸ View Using Encoding…", () => {
  it("changes the live session's encoding and reloads the connection's queries", async () => {
    const info = await ipc.connections.connect(MY.id);
    useConnectionsStore.getState().add(info);
    const set = vi.spyOn(ipc.connections, "setEncoding");
    const user = userEvent.setup();
    const { qc } = renderHost();
    qc.setQueryData(["rows", info.connectionId, "t"], "cached");
    await waitFor(() => expect(actions.has("connection.encoding")).toBe(true));
    void actions.run("connection.encoding");
    const dialog = await screen.findByRole("dialog", { name: "View Using Encoding" });
    const field = within(dialog).getByLabelText("Encoding");
    await waitFor(() => expect(field).toHaveValue("utf8mb4"));
    await user.clear(field);
    await user.type(field, "latin1{Enter}");
    await vi.waitFor(() => expect(set).toHaveBeenCalledWith(info.connectionId, "latin1"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(qc.getQueryState(["rows", info.connectionId, "t"])?.isInvalidated).toBe(true);
    expect(await ipc.connections.getEncoding(info.connectionId)).toBe("latin1");
    // Tabler reads and writes UTF-8 only: the toast says the connection is read-only now.
    expect(toast.success).toHaveBeenLastCalledWith(
      expect.stringMatching(/latin1 and is read-only/),
    );
  });

  it("warns that a session with an encoding other than UTF-8 is read-only", async () => {
    const info = await ipc.connections.connect(MY.id);
    useConnectionsStore.getState().add(info);
    const user = userEvent.setup();
    renderHost();
    await waitFor(() => expect(actions.has("connection.encoding")).toBe(true));
    void actions.run("connection.encoding");
    const dialog = await screen.findByRole("dialog", { name: "View Using Encoding" });
    const field = within(dialog).getByLabelText("Encoding");
    await waitFor(() => expect(field).toHaveValue("utf8mb4"));
    // The connection's own encoding: nothing to warn about.
    expect(within(dialog).queryByRole("note")).toBeNull();
    for (const utf8 of ["UTF8", "utf-8", "utf8mb3"]) {
      await user.clear(field);
      await user.type(field, utf8);
      expect(within(dialog).queryByRole("note")).toBeNull();
    }
    for (const other of ["latin1", "sjis", "WIN1252"]) {
      await user.clear(field);
      await user.type(field, other);
      expect(within(dialog).getByRole("note")).toHaveTextContent(
        "Tabler reads and writes text as UTF-8. With another encoding the session is read-only, so edits cannot be stored mis-encoded. Use Reset to leave it.",
      );
    }
    // An unfinished (invalid) name warns about nothing.
    await user.clear(field);
    await user.type(field, "lat in1");
    expect(within(dialog).queryByRole("note")).toBeNull();
  });

  it("does not call a UTF-8 session read-only", async () => {
    const info = await ipc.connections.connect(MY.id);
    useConnectionsStore.getState().add(info);
    const user = userEvent.setup();
    renderHost();
    await waitFor(() => expect(actions.has("connection.encoding")).toBe(true));
    void actions.run("connection.encoding");
    const dialog = await screen.findByRole("dialog", { name: "View Using Encoding" });
    const field = within(dialog).getByLabelText("Encoding");
    await waitFor(() => expect(field).toHaveValue("utf8mb4"));
    await user.clear(field);
    await user.type(field, "utf8mb3{Enter}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(toast.success).toHaveBeenLastCalledWith(expect.stringMatching(/now reads utf8mb3$/));
  });

  it("knows the UTF-8 names", () => {
    for (const utf8 of [null, undefined, "", " ", "utf8mb4", "UTF8", "Utf-8", "utf8mb3", "UNICODE"])
      expect(isUtf8Encoding(utf8), String(utf8)).toBe(true);
    for (const other of ["latin1", "LATIN1", "sjis", "gbk", "WIN1252", "utf16"])
      expect(isUtf8Encoding(other), other).toBe(false);
  });

  it("is not offered for SQLite", async () => {
    const info = await ipc.connections.connect(LITE.id);
    useConnectionsStore.getState().add(info);
    renderHost();
    await new Promise((r) => setTimeout(r, 0));
    expect(actions.has("connection.encoding")).toBe(false);
  });
});
