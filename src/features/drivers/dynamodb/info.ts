import { formatBytes } from "@/lib/values";

/**
 * What the table info panel shows, read from the session's `tableInfo` call: DynamoDB's
 * `DescribeTable`, `DescribeTimeToLive` and `DescribeContinuousBackups` replies, each null where
 * the server has none (an emulator). Read leniently: a field of an unexpected type is left out.
 */
export interface TableInfo {
  table: unknown;
  timeToLive: unknown;
  continuousBackups: unknown;
}

type Obj = Record<string, unknown>;

const obj = (v: unknown): Obj | null =>
  typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Obj) : null;
const str = (o: Obj | null, key: string): string | null => {
  const v = o?.[key];
  return typeof v === "string" ? v : null;
};
const num = (o: Obj | null, key: string): number | null => {
  const v = o?.[key];
  return typeof v === "number" && Number.isFinite(v) ? v : null;
};
const list = (o: Obj | null, key: string): unknown[] => {
  const v = o?.[key];
  return Array.isArray(v) ? v : [];
};

/** `PAY_PER_REQUEST` and the like in words. */
const words = (s: string) => s.toLowerCase().replace(/_/g, " ");

/** A creation time as DynamoDB sends it (seconds since the epoch). */
function created(v: unknown): string | null {
  if (typeof v !== "number" || !Number.isFinite(v)) return null;
  const d = new Date(v * 1000);
  return Number.isNaN(d.getTime()) ? null : d.toISOString().replace(".000Z", "Z");
}

/** The panel's lines: (label, value), in order, only those the replies hold. */
export function tableFacts(info: TableInfo): [string, string][] {
  const t = obj(info.table);
  const out: [string, string][] = [];
  const add = (label: string, value: string | null) => {
    if (value !== null && value !== "") out.push([label, value]);
  };
  add("Status", str(t, "TableStatus"));
  const billing = str(obj(t?.BillingModeSummary ?? null), "BillingMode");
  const throughput = obj(t?.ProvisionedThroughput ?? null);
  const rcu = num(throughput, "ReadCapacityUnits");
  const wcu = num(throughput, "WriteCapacityUnits");
  const provisioned = billing === "PROVISIONED" || (billing === null && (rcu ?? 0) > 0);
  if (t)
    add(
      "Billing",
      provisioned
        ? `provisioned, ${rcu ?? 0} read and ${wcu ?? 0} write capacity units`
        : billing === null || billing === "PAY_PER_REQUEST"
          ? "on-demand"
          : words(billing),
    );
  const items = num(t, "ItemCount");
  add(
    "Items",
    items === null
      ? null
      : `about ${items.toLocaleString("en-US")} (refreshed about every six hours)`,
  );
  const size = num(t, "TableSizeBytes");
  add("Size", size === null ? null : `about ${formatBytes(size)}`);
  const keys = list(t, "KeySchema")
    .map((k) => {
      const o = obj(k);
      const name = str(o, "AttributeName");
      const type = str(o, "KeyType");
      return name && type ? `${name} (${type === "HASH" ? "partition" : "sort"})` : null;
    })
    .filter((k): k is string => k !== null);
  add("Key", keys.join(", "));
  const gsi = list(t, "GlobalSecondaryIndexes").length;
  const lsi = list(t, "LocalSecondaryIndexes").length;
  if (t) add("Indexes", gsi + lsi === 0 ? "none" : `${gsi} global, ${lsi} local`);
  const stream = obj(t?.StreamSpecification ?? null);
  if (t)
    add(
      "Stream",
      stream?.StreamEnabled === true ? words(str(stream, "StreamViewType") ?? "on") : "off",
    );
  const ttl = obj(info.timeToLive);
  if (ttl) {
    const status = str(ttl, "TimeToLiveStatus");
    const attribute = str(ttl, "AttributeName");
    add(
      "Time to live",
      status === null ? null : `${words(status)}${attribute ? ` on ${attribute}` : ""}`,
    );
  } else add("Time to live", "not reported by this server");
  const pitr = str(
    obj(obj(info.continuousBackups)?.PointInTimeRecoveryDescription ?? null),
    "PointInTimeRecoveryStatus",
  );
  add(
    "Point-in-time recovery",
    info.continuousBackups === null ? "not reported by this server" : pitr && words(pitr),
  );
  if (t) add("Deletion protection", t.DeletionProtectionEnabled === true ? "on" : "off");
  if (t)
    add("Table class", words(str(obj(t?.TableClassSummary ?? null), "TableClass") ?? "STANDARD"));
  add("Encryption", str(obj(t?.SSEDescription ?? null), "SSEType"));
  add("Created", created(t?.CreationDateTime));
  add("ARN", str(t, "TableArn"));
  return out;
}

/** The secondary indexes of a table's structure (`IndexMeta.method` GSI or LSI). */
export interface IndexChoice {
  name: string;
  global: boolean;
  keys: string[];
}
