import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GRID } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc, type BrowsePage, type ColumnMeta, type TableMeta, type Value } from "@/lib/ipc";
import type { ObjectMenuContext, SidebarItemRef } from "../types";
import lane from ".";
import { dynamoConsole } from "./console";
import { DynamoInspector } from "./DynamoInspector";
import { dynamoUi } from "./form";
import { tableFacts } from "./info";
import { dynamoObjectMenu, indexChoices } from "./menu";
import { formParams, dynamoFormOf } from "./params";

// The lane's views: the console, the item inspector, a table's menu with its info panel and
// index browser, and the form's own rows.

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("the lane module", () => {
  it("gives the DynamoDB form, console, inspector and menu, and no SQL dialect", () => {
    expect(Object.keys(lane.kinds ?? {})).toEqual(["dynamodb"]);
    const ui = lane.drivers?.dynamodb;
    expect(ui?.console).toBe(dynamoConsole);
    expect(ui?.Inspector).toBe(DynamoInspector);
    expect(ui?.objectMenu).toBe(dynamoObjectMenu);
    expect(ui?.TableView).toBeUndefined();
    expect(ui?.StructureView).toBeUndefined();
    expect(lane.dialects).toBeUndefined();
  });
});

describe("the console", () => {
  it("splits, classifies and highlights PartiQL and API commands", () => {
    const text = `SELECT * FROM "Music" WHERE "Artist" = 'a;b';\nScan {"TableName": "t;x"}\n;DELETE FROM t WHERE pk = 'a'`;
    expect(dynamoConsole.split(text).map((r) => text.slice(r.start, r.end))).toEqual([
      `SELECT * FROM "Music" WHERE "Artist" = 'a;b'`,
      'Scan {"TableName": "t;x"}',
      "DELETE FROM t WHERE pk = 'a'",
    ]);
    expect(dynamoConsole.classify('Query {"TableName": "t"}')).toBe("read");
    expect(dynamoConsole.classify('PutItem {"TableName": "t", "Item": {}}')).toBe("write");
    expect(dynamoConsole.classify("frobnicate")).toBe("write");
    expect(dynamoConsole.fileExtension).toBe("partiql");
    expect(dynamoConsole.language()).toBeTruthy();
  });
});

const col = (name: string, isPk: boolean): ColumnMeta =>
  ({ name, isPk, pkOrdinal: isPk ? 1 : null }) as ColumnMeta;

describe("the inspector", () => {
  const row: Record<string, Value> = {
    pk: { t: "text", v: "u1" },
    zip: { t: "text", v: "02134" },
    n: { t: "decimal", v: "12345678901234567890.5" },
    tags: { t: "array", v: [{ t: "text", v: "a" }] },
    gone: { t: "other", v: { ty: "missing", text: "" } },
  };
  const table = { schema: "us-east-1", name: "Users" };
  const columns = [col("pk", true), col("zip", false), col("n", false)];

  it("shows the item as DynamoDB JSON and stages only what changed, a removal as missing", () => {
    const onStage = vi.fn();
    render(
      <DynamoInspector
        connectionId="c"
        table={table}
        columns={columns}
        row={row}
        onStage={onStage}
      />,
    );
    const area = screen.getByLabelText("Item") as HTMLTextAreaElement;
    expect(area.value).toContain('"S": "02134"');
    expect(area.value).toContain('"N": "12345678901234567890.5"');
    expect(area.value).toContain('"SS"');
    expect(area.value).not.toContain("gone");
    fireEvent.change(area, {
      target: {
        value: area.value.replace('"02134"', '"02135"').replace(/,\s*"tags": \{[^}]*\}/, ""),
      },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Stage/ }));
    expect(onStage).toHaveBeenCalledWith({
      zip: { t: "text", v: "02135" },
      tags: { t: "other", v: { ty: "missing", text: "" } },
    });
  });

  it("toggles to plain JSON, keeps the key, and is read-only without onStage", () => {
    const onStage = vi.fn();
    const { unmount } = render(
      <DynamoInspector
        connectionId="c"
        table={table}
        columns={columns}
        row={row}
        onStage={onStage}
      />,
    );
    fireEvent.click(screen.getByRole("radio", { name: "JSON" }));
    const area = screen.getByLabelText("Item") as HTMLTextAreaElement;
    expect(area.value).toContain('"zip": "02134"');
    expect(area.value).toContain("12345678901234567890.5");
    fireEvent.change(area, { target: { value: area.value.replace('"u1"', '"u2"') } });
    expect(screen.getByRole("button", { name: /^Stage/ })).toBeDisabled();
    unmount();
    render(<DynamoInspector connectionId="c" table={table} columns={columns} row={row} />);
    expect(screen.queryByRole("button", { name: /^Stage/ })).toBeNull();
    render(<DynamoInspector connectionId="c" table={table} columns={columns} row={null} />);
    expect(screen.getByTestId("dynamo-no-item")).toBeInTheDocument();
  });
});

describe("the table info panel", () => {
  it("reads DescribeTable, TTL and point-in-time recovery, leniently", () => {
    const facts = tableFacts({
      table: {
        TableStatus: "ACTIVE",
        BillingModeSummary: { BillingMode: "PROVISIONED" },
        ProvisionedThroughput: { ReadCapacityUnits: 5, WriteCapacityUnits: 2 },
        ItemCount: 1234,
        TableSizeBytes: 56789,
        KeySchema: [
          { AttributeName: "Artist", KeyType: "HASH" },
          { AttributeName: "SongTitle", KeyType: "RANGE" },
        ],
        GlobalSecondaryIndexes: [{}],
        StreamSpecification: { StreamEnabled: true, StreamViewType: "NEW_AND_OLD_IMAGES" },
        DeletionProtectionEnabled: true,
        TableClassSummary: { TableClass: "STANDARD_INFREQUENT_ACCESS" },
        CreationDateTime: 1767225600,
        TableArn: "arn:aws:dynamodb:us-east-1:1:table/Music",
        ItemCountX: "ignored",
      },
      timeToLive: { TimeToLiveStatus: "ENABLED", AttributeName: "expiresAt" },
      continuousBackups: {
        PointInTimeRecoveryDescription: { PointInTimeRecoveryStatus: "ENABLED" },
      },
    });
    expect(Object.fromEntries(facts)).toEqual({
      Status: "ACTIVE",
      Billing: "provisioned, 5 read and 2 write capacity units",
      Items: "about 1,234 (refreshed about every six hours)",
      Size: "about 55.5 KB",
      Key: "Artist (partition), SongTitle (sort)",
      Indexes: "1 global, 0 local",
      Stream: "new and old images",
      "Time to live": "enabled on expiresAt",
      "Point-in-time recovery": "enabled",
      "Deletion protection": "on",
      "Table class": "standard infrequent access",
      Created: "2026-01-01T00:00:00Z",
      ARN: "arn:aws:dynamodb:us-east-1:1:table/Music",
    });
    // An emulator without TTL or backups, a hostile reply's odd types.
    expect(
      Object.fromEntries(
        tableFacts({
          table: { ItemCount: "many", KeySchema: "x" },
          timeToLive: null,
          continuousBackups: null,
        }),
      ),
    ).toEqual({
      Billing: "on-demand",
      Indexes: "none",
      Stream: "off",
      "Time to live": "not reported by this server",
      "Point-in-time recovery": "not reported by this server",
      "Deletion protection": "off",
      "Table class": "standard",
    });
  });
});

describe("a table's menu", () => {
  const item: SidebarItemRef = {
    table: { schema: "us-east-1", name: "Music" },
    kind: "table",
    database: null,
  };
  const call = vi.fn();
  const ctx = {
    connectionId: "c1",
    info: {},
    caps: {},
    call,
    refresh: vi.fn(),
  } as unknown as ObjectMenuContext;
  const meta = {
    indexes: [
      { name: "PRIMARY", method: "primary key", columns: ["Artist", "SongTitle"] },
      { name: "ByGenre", method: "GSI", columns: ["Genre", "Year"] },
      { name: "ByYear", method: "LSI", columns: ["Artist", "Year"] },
    ],
  } as unknown as TableMeta;

  it("offers the info panel and the index browser on a table only", () => {
    expect(dynamoObjectMenu(item, ctx).map((i) => i.label)).toEqual([
      "Table Info…",
      "Browse Index…",
    ]);
    expect(dynamoObjectMenu({ ...item, kind: "view" }, ctx)).toEqual([]);
    expect(indexChoices(meta)).toEqual([
      { name: "ByGenre", global: true, keys: ["Genre", "Year"] },
      { name: "ByYear", global: false, keys: ["Artist", "Year"] },
    ]);
  });

  it("reads the table info as is (no Safe Mode prompt) and shows it", async () => {
    const read = vi.spyOn(ipc, "sessionCall").mockResolvedValue({
      table: { TableStatus: "ACTIVE", ItemCount: 3 },
      timeToLive: null,
      continuousBackups: null,
    });
    await dynamoObjectMenu(item, ctx)[0]!.run();
    expect(read).toHaveBeenCalledWith("c1", "tableInfo", { table: "Music" });
    expect(call).not.toHaveBeenCalled();
    expect(await screen.findByTestId("dynamo-table-info")).toHaveTextContent("ACTIVE");
  });

  it("pages an index forward and back by its cursors", async () => {
    vi.spyOn(ipc.schema, "table").mockResolvedValue(meta);
    const page = (n: string, next: string | null): BrowsePage =>
      ({
        columns: [{ name: "Genre" }],
        rows: [[{ t: "text", v: n }]],
        nextCursor: next,
      }) as unknown as BrowsePage;
    const browse = vi
      .spyOn(ipc, "sessionCall")
      .mockImplementation((_c, _m, args) =>
        Promise.resolve(
          (args as { request: { cursor: string | null } }).request.cursor === "c2"
            ? page("Jazz", null)
            : page("Rock", "c2"),
        ),
      );
    await dynamoObjectMenu(item, ctx)[1]!.run();
    expect(await screen.findByText("Rock")).toBeInTheDocument();
    expect(browse).toHaveBeenCalledWith("c1", "browseIndex", {
      table: "Music",
      index: "ByGenre",
      request: expect.objectContaining({ cursor: null, limit: 50 }),
    });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Jazz")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Previous" }));
    expect(await screen.findByText("Rock")).toBeInTheDocument();
  });

  it("says when a table has no secondary index", async () => {
    vi.spyOn(ipc.schema, "table").mockResolvedValue({ indexes: [] } as unknown as TableMeta);
    const browse = vi.spyOn(ipc, "sessionCall");
    await dynamoObjectMenu(item, ctx)[1]!.run();
    expect(browse).not.toHaveBeenCalled();
    expect(screen.queryByTestId("dynamo-index-page")).toBeNull();
  });
});

describe("the form's rows", () => {
  let current: ReturnType<typeof useForm<ConnectionFormValues>> | null = null;

  function Harness({ values }: { values: Partial<ConnectionFormValues> }) {
    const form = useForm<ConnectionFormValues>({
      defaultValues: { ...DEFAULT_FORM, driver: "dynamodb", ...dynamoUi.defaults!(), ...values },
    });
    current = form;
    const Fields = dynamoUi.Fields!;
    return (
      <div className={GRID}>
        <Fields form={form} kind="dynamodb" hasStoredPassword={false} />
      </div>
    );
  }

  const params = (p: Record<string, unknown>) => formParams({ ...dynamoFormOf(null), ...p });

  it("show the access key's rows, the regions of the backend's list, and no TLS for AWS", async () => {
    const util = vi
      .spyOn(ipc, "driverUtil")
      .mockImplementation((_input, method) =>
        Promise.resolve(method === "regions" ? ["us-east-1", "eu-west-1"] : []),
      );
    render(<Harness values={{}} />);
    expect(screen.getByLabelText("Access key ID")).toBeInTheDocument();
    expect(screen.getByLabelText("Secret Access Key")).toBeInTheDocument();
    expect(screen.getByLabelText("Region")).toHaveValue("us-east-1");
    await waitFor(() =>
      expect(screen.getByTestId("dynamo-regions").querySelectorAll("option")).toHaveLength(2),
    );
    expect(util).toHaveBeenCalledWith(expect.objectContaining({ password: null }), "regions");
    expect(screen.queryByText("SSL mode")).toBeNull();
    expect(screen.queryByLabelText("Profile")).toBeNull();
  });

  it("show a profile's name with this machine's profiles, and no key or secret", async () => {
    vi.spyOn(ipc, "driverUtil").mockImplementation((_input, method) =>
      Promise.resolve(method === "awsProfiles" ? ["default", "dev"] : []),
    );
    render(<Harness values={{ driverParams: params({ auth: "profile" }) }} />);
    expect(screen.queryByLabelText("Access key ID")).toBeNull();
    expect(screen.queryByLabelText("Secret Access Key")).toBeNull();
    expect(screen.getByLabelText("Profile")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByTestId("dynamo-profiles").querySelectorAll("option")).toHaveLength(2),
    );
    expect(await screen.findByText("2 profiles on this machine")).toBeInTheDocument();
  });

  it("send the form's calls with no secret and no Command Line to run", async () => {
    const util = vi.spyOn(ipc, "driverUtil").mockResolvedValue([]);
    const command = { passwordMode: "command" as const, passwordCommand: "touch /tmp/pwned" };
    const { unmount } = render(
      <Harness values={{ driverParams: params({ auth: "command" }), ...command }} />,
    );
    render(<Harness values={{ driverParams: params({ auth: "profile" }), ...command }} />);
    await waitFor(() =>
      expect(util.mock.calls.map(([, method]) => method).sort()).toEqual([
        "awsProfiles",
        "regions",
        "regions",
      ]),
    );
    for (const [input] of util.mock.calls) {
      expect(input.profile.options.passwordMode).toBe("none");
      expect(input.profile.options.passwordCommand).toBeNull();
      expect([input.password, input.sessionToken, input.keyPassphrase]).toEqual([null, null, null]);
    }
    unmount();
  });

  it("show the SSL mode for an https endpoint of its own", () => {
    vi.spyOn(ipc, "driverUtil").mockResolvedValue([]);
    render(
      <Harness values={{ driverParams: params({ endpoint: "https://ddb.internal:8443" }) }} />,
    );
    expect(screen.getByText("SSL mode")).toBeInTheDocument();
    expect(current!.getValues("ssl")).toBe("verify-full");
  });
});
