import { beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockSavedQueries } from "@/lib/ipc/mock/queries";
import {
  inputOf,
  moveSavedQuery,
  putSavedQuery,
  renameSavedQuery,
  resetSavedQueriesStore,
  useSavedQueriesStore,
} from "./store";

beforeEach(() => {
  vi.restoreAllMocks();
  resetMockSavedQueries();
  resetSavedQueriesStore();
});

const recent = async () => {
  const lib = await ipc.savedQueries.list();
  return lib.queries.find((q) => q.id === "q-recent-users")!;
};

describe("saved query rename and move", () => {
  it("send keepSql, so the file's own SQL is kept (the copy held here may be stale or lossy)", async () => {
    const save = vi.spyOn(ipc.savedQueries, "save");
    const query = await recent();
    await renameSavedQuery(query, "Newest users");
    await moveSavedQuery(query, "f-reports");
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[0]![0]).toEqual({
      ...inputOf(query),
      name: "Newest users",
      keepSql: true,
    });
    expect(save.mock.calls[1]![0]).toEqual({
      ...inputOf(query),
      folderId: "f-reports",
      keepSql: true,
    });
  });

  it("leave the stored SQL alone even when the copy held here is stale", async () => {
    const query = await recent();
    const stale = { ...query, sql: "select 'stale copy';" };
    const renamed = await renameSavedQuery(stale, "Newest users");
    expect(renamed.name).toBe("Newest users");
    expect(renamed.sql).toBe(query.sql);
    const moved = await moveSavedQuery(stale, "f-reports");
    expect(moved.folderId).toBe("f-reports");
    expect(moved.sql).toBe(query.sql);
    expect(useSavedQueriesStore.getState().queries.find((q) => q.id === query.id)?.sql).toBe(
      query.sql,
    );
  });

  it("do not change what a plain save does: it writes the SQL it is given", async () => {
    const query = await recent();
    const saved = await putSavedQuery({ ...inputOf(query), sql: "select 'edited';" });
    expect(saved.sql).toBe("select 'edited';");
    expect(inputOf(query)).not.toHaveProperty("keepSql");
  });
});
