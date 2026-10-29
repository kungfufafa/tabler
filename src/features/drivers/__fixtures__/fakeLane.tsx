import type { StatementRange } from "@/features/editor/scanner";
import type { ConnectionProfile, DatabaseKind, Driver } from "@/lib/ipc";
import type { ConsoleUi, DriverUi, KindUi, LaneModule } from "../types";
import { FakeFields, FakeInspector, FakeStructureView, FakeTableView } from "./fakeViews";

/**
 * A lane for the foundation's tests (`registerLaneOverride`), standing in for whichever lane
 * driver a test names, next to the mock's fake document store (`mock/drivers/fake`): a form of
 * host, user, secret and database rows whose database is a driver param (`authSource`), a
 * `fake://` URL, a line console (`find`/`count` read, anything else writes), and a table view,
 * inspector, structure view and object menu that show they were used (`fakeViews.tsx`).
 */

/** One command per non-empty line (UTF-16 offsets, 1-based lines). */
function splitLines(text: string): StatementRange[] {
  const out: StatementRange[] = [];
  let start = 0;
  text.split("\n").forEach((line, i) => {
    const lead = line.length - line.trimStart().length;
    const body = line.trim();
    if (body !== "")
      out.push({ start: start + lead, end: start + lead + body.length, line: i + 1 });
    start += line.length + 1;
  });
  return out;
}

const fakeConsole: ConsoleUi = {
  language: () => [],
  split: splitLines,
  classify: (command) => (/^\s*(find|count)\b/i.test(command) ? "read" : "write"),
  fileExtension: "fake",
  placeholder: "find orders",
};

const authSourceOf = (p: ConnectionProfile): string => {
  const v = p.options.driverParams?.authSource;
  return typeof v === "string" ? v : "";
};

/** The fake lane as the lane of `kind` and `driver` (ids of a real lane: the tests choose one). */
export function fakeLane(kind: DatabaseKind, driver: Driver): LaneModule {
  const form: KindUi = {
    rows: ["hostPort", "user", "secret", "database"],
    labels: { database: "Auth source" },
    sslModes: null,
    defaults: () => ({ port: "27017" }),
    Fields: FakeFields,
    validate: (v) => (v.host === "" ? [{ path: "host", message: "Host is required" }] : []),
    toProfile: (v, base) => ({
      ...base,
      // The database row is the auth source here, and the fake has no users.
      user: null,
      database: null,
      options: { ...base.options, driverParams: { authSource: v.database || "admin" } },
    }),
    fromProfile: (p) => ({ database: authSourceOf(p) }),
    url: {
      schemes: ["fake"],
      parse: (url) => {
        const m = /^fake:\/\/([^:/]+)(?::(\d+))?(?:\/(\w+))?$/.exec(url);
        if (!m) return null;
        return {
          values: { host: m[1], port: m[2] ?? "27017", database: m[3] ?? "" },
          password: null,
        };
      },
      format: (p) => `fake://${p.host ?? ""}:${p.port ?? 27017}/${authSourceOf(p)}`,
    },
    summary: (p) => `${p.host ?? ""}:${p.port ?? 27017}`,
  };
  const views: DriverUi = {
    console: fakeConsole,
    TableView: FakeTableView,
    Inspector: FakeInspector,
    StructureView: FakeStructureView,
    objectMenu: (item, ctx) => [
      {
        id: "stats",
        label: "Collection Stats",
        run: () => ctx.call("stats", { collection: item.table.name }).then(() => undefined),
      },
      {
        id: "compact",
        label: "Compact…",
        destructive: true,
        run: async () => {
          await ctx.call("compact", { collection: item.table.name });
          ctx.refresh();
        },
      },
    ],
  };
  return { kinds: { [kind]: form }, drivers: { [driver]: views } };
}
