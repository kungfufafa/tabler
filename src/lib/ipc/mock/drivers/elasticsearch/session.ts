import { classifyRequest } from "@/features/drivers/elasticsearch/classify";
import { fromPlain, parseStrict, toText, type Json } from "@/features/drivers/elasticsearch/json";
import { consoleText, type EsRequest } from "@/features/drivers/elasticsearch/request";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { FilterNode } from "../../../bindings/FilterNode";
import type { FilterOp } from "../../../bindings/FilterOp";
import type { KillOutcome } from "../../../bindings/KillOutcome";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { SortSpec } from "../../../bindings/SortSpec";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableRef } from "../../../bindings/TableRef";
import type { TableSummary } from "../../../bindings/TableSummary";
import type { Value } from "../../../bindings/Value";
import { matchesGroup, sortRows } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import { runConsole } from "./console";
import { at, cellOf, kindOf } from "./values";
import {
  indicesOf,
  mergedFields,
  seedCluster,
  type Cluster,
  type Doc,
  type Field,
  type Index,
} from "./data";

/**
 * A mock Elasticsearch or OpenSearch session, mirroring the backend's
 * (`drivers/elasticsearch/session.rs`): the same caps, the cluster as the one database with its
 * indices, aliases and data streams, the flattened mapping as columns with the hit's metadata as
 * its key, cursor pages, costly counts, fetch by `_id`, a guarded `_bulk` per commit whose changes
 * apply one by one (a version conflict or a mapping error is that change's own), the console
 * (`console.ts`), the session's calls, the cluster's tasks, and every refusal of a read-only
 * session.
 */

/** What the cluster can do (`info::Features`). */
export interface EsFeatures {
  seqNoGuards: boolean;
  tasks: boolean;
  caseInsensitive: boolean;
}

/** The backend's read-only refusal of a write path (`EsSession::writable`). */
export const readOnlyMessage = (what: string) => `this connection is read-only: ${what} is refused`;

const CASE_INSENSITIVE: FilterOp[] = [
  "containsCase",
  "notContainsCase",
  "hasPrefixCase",
  "hasSuffixCase",
  "ilike",
];

/** Every filter operator but JSON containment (`session_caps`). */
export const FILTER_OPS: FilterOp[] = [
  "eq",
  "ne",
  "gt",
  "lt",
  "ge",
  "le",
  "like",
  "notLike",
  "in",
  "notIn",
  "isNull",
  "isNotNull",
  "between",
  "notBetween",
  "contains",
  "notContains",
  "containsCase",
  "notContainsCase",
  "hasPrefix",
  "hasSuffix",
  "hasPrefixCase",
  "hasSuffixCase",
  "ilike",
];

/** `session::session_caps`. */
export function esCaps(f: EsFeatures): SessionCaps {
  // In `Feature`'s order, as the backend sorts them.
  const features: SessionCaps["features"] = ["editor", "cancelOnServer"];
  if (f.seqNoGuards) features.push("editRows");
  features.push("insertRows");
  if (f.seqNoGuards) features.push("deleteRows");
  features.push("returning", "exactCount", "structureView", "viewDefinition");
  if (f.tasks) features.push("processList", "killProcess");
  return {
    driver: "elasticsearch",
    flavor: null,
    dialect: null,
    language: "esConsole",
    features,
    namespaces: {
      databases: false,
      schemas: "single",
      defaultSchema: null,
      labels: { database: "Cluster", schema: "Schema", table: "Index", tables: "Indices" },
    },
    data: {
      paging: "cursor",
      count: "costly",
      sort: true,
      filterOps: FILTER_OPS.filter((op) => f.caseInsensitive || !CASE_INSENSITIVE.includes(op)),
      rawFilter: "queryString",
      transactions: "batch",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      serverExprs: [],
      maxParams: 0,
    },
    safety: { readOnly: "classifierOnly", hardened: false, confirm: "passcode" },
    session: { keepaliveSecs: 60, pingBilled: false },
  };
}

// ---- values ----------------------------------------------------------------------------------

const ANALYZED = ["text", "match_only_text", "search_as_you_type", "annotated_text"];

/** The field a sort uses (`Field::sort_field`), if any. */
const sortField = (f: Field): string | null =>
  ANALYZED.includes(f.type) || kindOf(f.type) === "json" ? (f.keyword ?? null) : f.path;

/** A staged cell as the `_source` value written (`values::to_json`), or the refusal. */
function writtenValue(v: Value, field: string, type: string | null): unknown {
  const refuse = (why: string): never => mockError("INVALID_INPUT", `${field}: ${why}`);
  switch (v.t) {
    case "null":
      return null;
    case "default":
      return refuse("Elasticsearch has no column defaults; leave the field out instead");
    case "bool":
      return v.v;
    case "int":
    case "float":
      return v.v;
    case "bigInt":
    case "decimal":
      return /^-?\d+(\.\d+)?$/.test(v.v)
        ? BigInt(v.v.split(".")[0]!)
        : refuse(`“${v.v}” is not a number`);
    case "json":
      try {
        return JSON.parse(v.v) as unknown;
      } catch {
        return refuse("not valid JSON");
      }
    case "other":
      if (v.v.ty === "missing") return refuse("a missing field has no value to write");
      return textAs(v.v.text, type);
    case "text":
      return textAs(v.v, type);
    case "array":
      return v.v.map((x) => writtenValue(x, field, type));
    case "bytes":
      return v.v.base64;
    default:
      return v.v;
  }
}

/** Text in the field's own JSON type when it reads as one (`values::text_as`). */
function textAs(s: string, type: string | null): unknown {
  const t = s.trim();
  const kind = type === null ? null : kindOf(type);
  if (
    (kind === "int" || kind === "float" || kind === "bigInt") &&
    /^-?\d+(\.\d+)?([eE][+-]?\d+)?$/.test(t)
  )
    return Number(t);
  if (kind === "bool" && (t === "true" || t === "false")) return t === "true";
  return s;
}

/** A value a field of `type` refuses at index time (the cluster's `document_parsing_exception`). */
function unparsable(v: unknown, type: string): boolean {
  const kind = kindOf(type);
  if (kind === "int" || kind === "bigInt")
    return typeof v === "string" && !/^-?\d+$/.test(v.trim());
  if (kind === "float") return typeof v === "string" && Number.isNaN(Number(v));
  if (kind === "bool") return typeof v === "string" && v !== "true" && v !== "false";
  return false;
}

// ---- the catalog -----------------------------------------------------------------------------

/** The metadata columns of a hit (`catalog::META_COLUMNS`). */
export const META = ["_id", "_index", "_routing", "_seq_no", "_primary_term", "_score"];

function column(name: string, type: string, ordinal: number): ColumnMeta {
  return {
    name,
    ordinal,
    dataType: type,
    kind: kindOf(type),
    nullable: !META.includes(name),
    defaultExpr: null,
    isPk: false,
    pkOrdinal: null,
    isAutoIncrement: false,
    isGenerated: false,
    isUnsigned: type === "unsigned_long",
    comment: null,
    enumValues: null,
    charMaxLen: null,
    numericPrecision: null,
    numericScale: null,
    collation: null,
    arrayElemType: null,
    extra: null,
    baseType: null,
  };
}

/** A target's columns (`catalog::columns`): the key, then the mapped fields. */
export function columnsOf(fields: readonly Field[], spans: boolean, guards: boolean): ColumnMeta[] {
  const out: ColumnMeta[] = [];
  let pk = 0;
  const push = (c: ColumnMeta, key: boolean, patch: Partial<ColumnMeta> = {}) => {
    const col = { ...c, ...patch, ordinal: out.length + 1 };
    if (key) {
      pk += 1;
      col.isPk = true;
      col.pkOrdinal = pk;
    }
    out.push(col);
  };
  if (spans)
    push(column("_index", "keyword", 0), true, {
      isGenerated: true,
      comment: "the index the document is in",
    });
  push(column("_id", "keyword", 0), true, {
    isAutoIncrement: true,
    comment: "document id; generated when an insert leaves it empty",
  });
  if (guards)
    for (const name of ["_seq_no", "_primary_term"])
      push(column(name, "long", 0), true, {
        isGenerated: true,
        comment: "concurrency guard sent back with each edit",
      });
  for (const f of fields)
    push(column(f.path, f.type, 0), false, {
      extra: f.keyword ? `sort: ${f.keyword}` : sortField(f) === null ? "not sortable" : null,
    });
  return out;
}

const failure = (message: string, extra: Partial<ErrorPayload> = {}): ErrorPayload => ({
  code: "DB_ERROR",
  message,
  detail: null,
  hint: null,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex: null,
  data: null,
  ...extra,
});

/** A target as the sidebar names one (`check_target`). */
function checkTarget(name: string): string {
  if (
    name === "" ||
    name === "." ||
    name === ".." ||
    /^[_+-]/.test(name) ||
    /[\s/\\?#"<>|]/u.test(name) ||
    /\p{Cc}/u.test(name)
  )
    mockError("INVALID_INPUT", `“${name}” is not an index, alias or data stream name`);
  return name;
}

/** One index an object-menu operation acts on (`check_index`). */
function checkIndex(name: string): string {
  if (/[*,:]/.test(checkTarget(name)))
    mockError("INVALID_INPUT", `“${name}” names several indices: the operation takes one`);
  return name;
}

interface Hit {
  index: Index;
  doc: Doc;
}

/** Options of the mock profiles (the profile's `driverParams` and the server it stands for). */
export interface SessionOptions {
  flavor: "elasticsearch" | "opensearch";
  version: string;
  features: EsFeatures;
}

/** The mock server a profile reaches: OpenSearch when its host or name says so. */
export function optionsOf(p: ConnectionProfile): SessionOptions {
  const os = /opensearch/i.test(`${p.host ?? ""} ${p.name}`);
  return os
    ? {
        flavor: "opensearch",
        version: "2.19.1",
        features: { seqNoGuards: true, tasks: true, caseInsensitive: true },
      }
    : {
        flavor: "elasticsearch",
        version: "9.1.5",
        features: { seqNoGuards: true, tasks: true, caseInsensitive: true },
      };
}

let sessions = 0;
const clusters = new Map<string, Cluster>();

/** Tests only: every mock cluster back to its seed. */
export function resetClusters(): void {
  clusters.clear();
}

export function esSession(profile: ConnectionProfile): MockSession {
  const opts = optionsOf(profile);
  const f = opts.features;
  const caps = esCaps(f);
  const readOnly = profile.options.readOnly;
  const showSystem =
    (profile.options.driverParams as { showSystemIndices?: unknown } | null)?.showSystemIndices ===
    true;
  const clusterName = opts.flavor === "opensearch" ? "opensearch-cluster" : "docker-cluster";
  const cluster = clusters.get(clusterName) ?? seedCluster(clusterName);
  clusters.set(clusterName, cluster);
  sessions += 1;
  const opaque = `tabler-mock${String(sessions).padStart(4, "0")}`;

  const writable = (what: string) => {
    if (readOnly) mockError("INVALID_INPUT", readOnlyMessage(what));
  };

  /** The indices behind a sidebar name, refused when there are none. */
  const targetOf = (name: string): Index[] => {
    const found = indicesOf(cluster, checkTarget(name));
    if (found.length === 0 && !name.includes("*"))
      mockError("NOT_FOUND", `not found: no such index [${name}]`);
    return found;
  };
  const spans = (name: string) => !cluster.indices.has(name);

  const meta = (table: TableRef): TableMeta => {
    const indices = targetOf(table.name);
    const fields = mergedFields(indices);
    const columns = columnsOf(fields, spans(table.name), f.seqNoGuards);
    const kind = cluster.aliases.has(table.name)
      ? "view"
      : cluster.streams.has(table.name)
        ? "partitioned"
        : table.name.startsWith(".")
          ? "systemView"
          : "table";
    const behind = cluster.aliases.get(table.name) ?? cluster.streams.get(table.name);
    return {
      table,
      kind,
      columns,
      primaryKey: columns.filter((c) => c.isPk).map((c) => c.name),
      indexes: [],
      foreignKeys: [],
      triggers: [],
      comment: behind ? `over ${behind.join(", ")}` : null,
      rowEstimate: indices.reduce((n, i) => n + i.docs.length, 0),
      ddl: null,
      checks: [],
      options: null,
    };
  };

  const fieldType = (fields: readonly Field[], column: string): string | null =>
    fields.find((x) => x.path === column)?.type ?? null;

  const rowOf = (hit: Hit, columns: readonly ColumnMeta[]): Value[] =>
    columns.map((c) => {
      switch (c.name) {
        case "_index":
          return { t: "text", v: hit.index.name };
        case "_id":
          return { t: "text", v: hit.doc.id };
        case "_seq_no":
          return { t: "int", v: hit.doc.seqNo };
        case "_primary_term":
          return { t: "int", v: hit.doc.primaryTerm };
        default:
          return cellOf(at(hit.doc.source, c.name), c.dataType);
      }
    });

  /** The hits of a target, filtered, in `_doc` order (or sorted). */
  const search = (table: TableRef, filter: FilterGroup | null, sort: readonly SortSpec[]) => {
    const m = meta(table);
    const indices = targetOf(table.name);
    const fields = mergedFields(indices);
    for (const s of sort) {
      const field = fields.find((x) => x.path === s.column);
      if (field && sortField(field) === null)
        mockError(
          "INVALID_INPUT",
          `${s.column}: a ${field.type} field without a keyword multi-field cannot be sorted`,
        );
    }
    if (filter) checkRaw(filter);
    const hits: Hit[] = indices.flatMap((index) => index.docs.map((doc) => ({ index, doc })));
    let rows = hits.map((h) => ({ h, row: rowOf(h, m.columns) }));
    if (filter) {
      const plain = stripRaw(filter);
      rows = rows.filter(
        (r) => matchesGroup(plain, r.row, m.columns) && rawMatches(filter, r.h, fields),
      );
    }
    const sorted = sortRows(
      rows.map((r) => r.row),
      [...sort],
      m.columns,
    );
    return { m, fields, rows: sorted };
  };

  const preview = (req: ApplyRequest): string[] => {
    writable("editing rows");
    return plan(req).map((op) => consoleText(op.request));
  };

  /** One `_bulk` action per change, each previewed as a one-item `_bulk` (`plan_edits`). */
  const plan = (req: ApplyRequest) => {
    const name = checkTarget(req.table.name);
    const fields = mergedFields(targetOf(name));
    return req.changes.map((change, i) => planOne(change, i, name, fields));
  };

  const planOne = (change: StagedChange, i: number, target: string, fields: Field[]) => {
    if ("exprs" in change && change.exprs && Object.keys(change.exprs).length > 0)
      mockError("UNSUPPORTED", "unsupported: Elasticsearch has no server expressions");
    const lines: Json[] = [];
    let source: Record<string, unknown> | null = null;
    let action: Record<string, unknown>;
    if (change.op === "insert") {
      const values = change.values;
      const index = textOf(values._index) || target;
      const id = textOf(values._id);
      action = { _index: index, ...(id ? { _id: id } : {}) };
      source = {};
      for (const [col, v] of Object.entries(values)) {
        if (META.includes(col) || v.t === "default" || (v.t === "other" && v.v.ty === "missing"))
          continue;
        insertPath(source, col, writtenValue(v, col, fieldType(fields, col)));
      }
      lines.push(fromPlain({ create: action }), fromPlain(source));
    } else {
      action = guarded(change.key, target);
      if (change.op === "update") {
        source = {};
        for (const [col, v] of Object.entries(change.changes)) {
          if (META.includes(col))
            mockError(
              "INVALID_INPUT",
              `${col}: document metadata cannot be edited (duplicate the row to give it another id)`,
            );
          if (v.t === "other" && v.v.ty === "missing")
            mockError(
              "INVALID_INPUT",
              `${col}: removing a field needs a script; use the console (\`ctx._source.remove\`)`,
            );
          insertPath(source, col, writtenValue(v, col, fieldType(fields, col)));
        }
        if (Object.keys(source).length === 0)
          mockError("INVALID_INPUT", "an update with no changes");
        lines.push(fromPlain({ update: action }), fromPlain({ doc: source }));
      } else lines.push(fromPlain({ delete: action }));
    }
    const request: EsRequest = {
      method: "POST",
      segments: ["_bulk"],
      query: [["refresh", "wait_for"]],
      body: { kind: "ndjson", values: lines },
    };
    return { change, i, action, source, request };
  };

  const guarded = (key: RowKey, target: string): Record<string, unknown> => {
    if (key.kind !== "pk")
      mockError("UNSUPPORTED", "unsupported: Elasticsearch edits need the document id (_id)");
    if (!f.seqNoGuards)
      mockError(
        "UNSUPPORTED",
        "unsupported: editing needs Elasticsearch 6.7 or later (if_seq_no guards)",
      );
    const id = textOf(key.value._id);
    if (!id) mockError("INVALID_INPUT", "the row has no _id");
    const seq = key.value._seq_no;
    const term = key.value._primary_term;
    if (seq?.t !== "int" || term?.t !== "int")
      mockError(
        "INVALID_INPUT",
        "the row was read without _seq_no / _primary_term; reload it before editing",
      );
    return {
      _index: textOf(key.value._index) || target,
      _id: id,
      if_seq_no: seq.v,
      if_primary_term: term.v,
    };
  };

  /** Applies one planned change to the cluster, as the cluster's `_bulk` item would. */
  const applyOne = (op: ReturnType<typeof planOne>): RowResult => {
    const result: RowResult = {
      rowId: op.change.rowId,
      op: op.change.op,
      affected: 0,
      returned: null,
      error: null,
    };
    const indexName = String(op.action._index);
    const index = cluster.indices.get(indexName) ?? targetOf(indexName)[0];
    if (!index)
      return {
        ...result,
        error: failure(`no such index [${indexName}]`, {
          vendorCode: 404,
          detail: "index_not_found_exception",
        }),
      };
    const conflict = (doc: Doc | undefined) =>
      doc === undefined ||
      doc.seqNo !== op.action.if_seq_no ||
      doc.primaryTerm !== op.action.if_primary_term;
    const mapping = (src: Record<string, unknown>): ErrorPayload | null => {
      for (const field of index.fields) {
        const v = at(src, field.path);
        if (v !== undefined && unparsable(v, field.type))
          return failure(
            `[1:2] failed to parse field [${field.path}] of type [${field.type}] in document with id '${String(op.action._id ?? "")}'`,
            { vendorCode: 400, detail: "document_parsing_exception" },
          );
      }
      return null;
    };
    const done = (doc: Doc): RowResult => ({
      ...result,
      affected: 1,
      returned: {
        _index: { t: "text", v: index.name },
        _id: { t: "text", v: doc.id },
        _seq_no: { t: "int", v: doc.seqNo },
        _primary_term: { t: "int", v: doc.primaryTerm },
      },
    });
    if (op.change.op === "insert") {
      const id = (op.action._id as string | undefined) ?? `mock${index.nextSeqNo}`;
      if (index.docs.some((d) => d.id === id))
        return {
          ...result,
          error: failure(
            `[${id}]: version conflict, document already exists (current version [1])`,
            {
              vendorCode: 409,
              detail: "version_conflict_engine_exception",
              hint: "The document changed since it was read (or was deleted). Reload and edit again.",
            },
          ),
        };
      const bad = mapping(op.source ?? {});
      if (bad) return { ...result, error: bad };
      const doc: Doc = { id, seqNo: index.nextSeqNo++, primaryTerm: 1, source: op.source ?? {} };
      index.docs.push(doc);
      return done(doc);
    }
    const at_ = index.docs.findIndex((d) => d.id === op.action._id);
    const doc = index.docs[at_];
    if (!doc)
      return {
        ...result,
        error: failure("the document no longer exists", { vendorCode: 404 }),
      };
    if (conflict(doc))
      return {
        ...result,
        error: failure(
          `[${doc.id}]: version conflict, required seqNo [${String(op.action.if_seq_no)}], primary term [${String(op.action.if_primary_term)}]. current document has seqNo [${doc.seqNo}] and primary term [${doc.primaryTerm}]`,
          {
            vendorCode: 409,
            detail: "version_conflict_engine_exception",
            hint: "The document changed since it was read (or was deleted). Reload and edit again.",
          },
        ),
      };
    if (op.change.op === "delete") {
      index.docs.splice(at_, 1);
      index.nextSeqNo++;
      return { ...done(doc), returned: done({ ...doc, seqNo: index.nextSeqNo - 1 }).returned };
    }
    const merged = mergeDeep(structuredClone(doc.source), op.source ?? {});
    const bad = mapping(merged);
    if (bad) return { ...result, error: bad };
    doc.source = merged;
    doc.seqNo = index.nextSeqNo++;
    return done(doc);
  };

  return {
    caps,
    listDatabases: () => [{ name: cluster.name, owner: null, encoding: null, isCurrent: true }],
    listSchemas: () => [],
    listTables: (schema): TableSummary[] => {
      const backing = new Set([...cluster.streams.values()].flat());
      const sch = schema.trim() === "" ? null : schema;
      const out: TableSummary[] = [];
      for (const i of cluster.indices.values()) {
        if (backing.has(i.name) || (!showSystem && (i.hidden || i.name.startsWith(".")))) continue;
        out.push({
          name: i.name,
          schema: sch,
          kind: i.name.startsWith(".") ? "systemView" : "table",
          rowEstimate: i.docs.length,
          sizeBytes: 4096 + i.docs.length * 310,
          comment: `green, ${i.status}, 1 primary, 1 replica`,
        });
      }
      for (const [alias, targets] of cluster.aliases)
        out.push({
          name: alias,
          schema: sch,
          kind: "view",
          rowEstimate: null,
          sizeBytes: null,
          comment: `alias of ${targets.join(", ")}`,
        });
      for (const [stream, targets] of cluster.streams)
        out.push({
          name: stream,
          schema: sch,
          kind: "partitioned",
          rowEstimate: targets.reduce((n, t) => n + (cluster.indices.get(t)?.docs.length ?? 0), 0),
          sizeBytes: null,
          comment: `data stream, ${targets.length} backing ind${targets.length === 1 ? "ex" : "ices"}, green`,
        });
      return out;
    },
    tableMeta: meta,
    browse: (req): BrowsePage => {
      const { m, rows } = search(req.table, req.filter, req.sort);
      const columns = req.columns
        ? m.columns.filter((c) => c.isPk || req.columns!.includes(c.name))
        : m.columns;
      const idx = columns.map((c) => m.columns.indexOf(c));
      const from = req.cursor ? positionOf(req.cursor) : 0;
      const limit = Math.max(1, req.limit);
      const page = rows.slice(from, from + limit);
      const body = fromPlain({
        from,
        size: limit,
        query: dslOf(req.filter),
        sort:
          req.sort.length > 0 ? req.sort.map((s) => ({ [s.column]: { order: s.dir } })) : ["_doc"],
        track_total_hits: false,
        seq_no_primary_term: f.seqNoGuards,
        _source: true,
      });
      return {
        columns,
        rows: page.map((r) => idx.map((i) => r[i] ?? { t: "null" })),
        pkColumns: m.primaryKey,
        identity: "pk",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql: consoleText({
          method: "POST",
          segments: [req.table.name, "_search"],
          query: [["preference", opaque]],
          body: { kind: "json", value: body },
        }),
        durationMs: 3,
        // Every full page hands out a cursor, as the backend's walk does.
        nextCursor: page.length === limit ? `pos:${from + limit}` : null,
      };
    },
    count: (table, filter) => {
      const total = meta(table).rowEstimate ?? 0;
      // Unfiltered, the indices' document count (not exact); filtered, `_count`.
      if (filter === null) return { count: total, exact: false };
      return { count: search(table, filter, []).rows.length, exact: true };
    },
    fetchCell: (table, key, field) => {
      if (key.kind !== "pk")
        mockError("UNSUPPORTED", "unsupported: Elasticsearch reads a document by its _id");
      const id = textOf(key.value._id);
      if (!id) mockError("INVALID_INPUT", "the row has no _id");
      const index = textOf(key.value._index) || table.name;
      const hit = targetOf(index)
        .flatMap((i) => i.docs.map((doc) => ({ index: i, doc })))
        .find((h) => h.doc.id === id);
      if (!hit) mockError("NOT_FOUND", "not found: the document no longer exists");
      // A metadata column from the hit, a field from its `_source` (as `_mget` limits it).
      const type = META.includes(field)
        ? "keyword"
        : (fieldType(mergedFields(targetOf(table.name)), field) ?? "object");
      return rowOf(hit, [column(field, type, 0)])[0]!;
    },
    preview,
    apply: (req): ApplyResult => {
      let planned: ReturnType<typeof plan>;
      const failedAll = (error: ErrorPayload, sql: string[]): ApplyResult => ({
        success: false,
        results: req.changes.map((c) => ({
          rowId: c.rowId,
          op: c.op,
          affected: 0,
          returned: null,
          error,
        })),
        sql,
        error,
        durationMs: 1,
      });
      try {
        writable("editing rows");
        planned = plan(req);
      } catch (e) {
        const { code, message } = e as { code: string; message: string };
        return failedAll(failure(message, { code }), []);
      }
      const results = planned.map((op) => {
        const r = applyOne(op);
        return req.returning ? r : { ...r, returned: null };
      });
      const error = results.find((r) => r.error)?.error ?? null;
      return {
        success: error === null,
        results,
        sql: planned.map((op) => consoleText(op.request)),
        error,
        durationMs: 4,
      };
    },
    run: (statement, emit) =>
      runConsole({ cluster, readOnly, opaque, features: f, flavor: opts.flavor }, statement, emit),
    call: (method, args) => {
      const a = (args ?? {}) as Record<string, unknown>;
      const arg = (name: string): string => {
        const v = a[name];
        if (typeof v !== "string" || v.trim() === "")
          mockError("INVALID_INPUT", `Elasticsearch: ${name} is required`);
        return v.trim();
      };
      const reads = ["clusterHealth", "indexSettings", "document"];
      const writes = ["indexAction", "deleteIndex", "truncate", "createIndex"];
      if (!reads.includes(method) && !writes.includes(method))
        mockError("UNSUPPORTED", `unsupported: Elasticsearch: unknown call “${method}”`);
      if (writes.includes(method)) writable(method);
      const write = (req: EsRequest, reply: unknown) => ({
        request: consoleText(req),
        reply: toText(fromPlain(reply), 2),
      });
      switch (method) {
        case "clusterHealth":
          return toText(
            fromPlain({
              cluster_name: cluster.name,
              status: "green",
              number_of_nodes: 1,
              active_primary_shards: cluster.indices.size,
            }),
            2,
          );
        case "indexSettings": {
          const name = checkTarget(arg("index"));
          const out: Record<string, unknown> = {};
          for (const i of targetOf(name))
            out[i.name] = {
              settings: {
                index: {
                  number_of_shards: "1",
                  number_of_replicas: "1",
                  max_result_window: String(i.window),
                  ...(i.hidden ? { hidden: "true" } : {}),
                },
              },
            };
          return toText(fromPlain(out), 2);
        }
        case "document": {
          const name = checkTarget(arg("index"));
          const id = arg("id");
          const hit = targetOf(name)
            .flatMap((i) => i.docs.map((doc) => ({ i, doc })))
            .find((h) => h.doc.id === id);
          return hit
            ? { found: true, index: hit.i.name, id, source: toText(fromPlain(hit.doc.source), 2) }
            : { found: false };
        }
        case "indexAction": {
          const name = checkIndex(arg("index"));
          const action = arg("action");
          const segs: Record<string, string[]> = {
            open: ["_open"],
            close: ["_close"],
            refresh: ["_refresh"],
            flush: ["_flush"],
            forcemerge: ["_forcemerge"],
            clear_cache: ["_cache", "clear"],
          };
          const tail = segs[action];
          if (!tail) mockError("INVALID_INPUT", `unknown index action “${action}”`);
          const indices = targetOf(name);
          if (action === "open" || action === "close") for (const i of indices) i.status = action;
          const reply =
            action === "open" || action === "close"
              ? { acknowledged: true, shards_acknowledged: true }
              : { _shards: { total: 2, successful: 1, failed: 0 } };
          return write({ method: "POST", segments: [name, ...tail], query: [], body: null }, reply);
        }
        case "deleteIndex": {
          const name = checkIndex(arg("index"));
          if (!cluster.indices.delete(name))
            mockError("NOT_FOUND", `not found: no such index [${name}]`);
          for (const [alias, targets] of cluster.aliases) {
            const left = targets.filter((t) => t !== name);
            if (left.length === 0) cluster.aliases.delete(alias);
            else cluster.aliases.set(alias, left);
          }
          return write(
            { method: "DELETE", segments: [name], query: [], body: null },
            { acknowledged: true },
          );
        }
        case "truncate": {
          const name = checkIndex(arg("index"));
          const indices = targetOf(name);
          const deleted = indices.reduce((n, i) => n + i.docs.length, 0);
          for (const i of indices) i.docs = [];
          return write(
            {
              method: "POST",
              segments: [name, "_delete_by_query"],
              query: [
                ["conflicts", "proceed"],
                ["refresh", "true"],
              ],
              body: { kind: "json", value: fromPlain({ query: { match_all: {} } }) },
            },
            { took: 12, timed_out: false, total: deleted, deleted, failures: [] },
          );
        }
        default: {
          const name = checkIndex(arg("name"));
          const text = typeof a.body === "string" ? a.body.trim() : "";
          let body: Json | null = null;
          if (text !== "")
            try {
              body = parseStrict(text);
            } catch (e) {
              mockError("INVALID_INPUT", `index body: ${(e as Error).message}`);
            }
          if (cluster.indices.has(name))
            mockError("DB_ERROR", `index [${name}/abc123] already exists`);
          cluster.indices.set(name, {
            name,
            fields: fieldsOfBody(body),
            docs: [],
            hidden: false,
            status: "open",
            window: 10_000,
            nextSeqNo: 0,
          });
          return write(
            {
              method: "PUT",
              segments: [name],
              query: [],
              body: body ? { kind: "json", value: body } : null,
            },
            { acknowledged: true, shards_acknowledged: true, index: name },
          );
        }
      }
    },
    listProcesses: f.tasks
      ? (): ProcessRow[] => {
          const now = Date.now();
          const row = (t: Cluster["tasks"][number], current: boolean): ProcessRow => ({
            id: t.id,
            user: null,
            database: null,
            application: t.opaqueId,
            client: t.id.split(":")[0] ?? null,
            state: t.cancellable ? "running" : "running (not cancellable)",
            command: t.action,
            startedAt: new Date(t.startedAt).toISOString(),
            durationMs: now - t.startedAt,
            waitEvent: null,
            query: t.description,
            current,
            own: t.opaqueId?.startsWith("tabler-") ?? false,
          });
          const listing = {
            id: "oTUltX4IQMOUUVeiohTt8A:12400",
            action: "cluster:monitor/tasks/lists",
            description: "",
            startedAt: now,
            cancellable: false,
            opaqueId: opaque,
          };
          return [
            ...cluster.tasks.map((t) => row(t, false)),
            { ...row(listing, true), query: null },
          ];
        }
      : undefined,
    killProcesses: f.tasks
      ? (ids): KillOutcome[] =>
          ids.map((id) => {
            if (readOnly) return { id, ok: false, message: readOnlyMessage("cancelling tasks") };
            if (!/^[^:\s/]+:\d+$/.test(id))
              return { id, ok: false, message: `“${id}” is not a task id (node:number)` };
            const at_ = cluster.tasks.findIndex((t) => t.id === id);
            if (at_ < 0) return { id, ok: false, message: `task [${id}] is missing` };
            if (!cluster.tasks[at_]!.cancellable)
              return { id, ok: false, message: `task [${id}] is not cancellable` };
            cluster.tasks.splice(at_, 1);
            return { id, ok: true, message: null };
          })
      : undefined,
  };
}

// ---- helpers ---------------------------------------------------------------------------------

const textOf = (v: Value | undefined): string =>
  v?.t === "text" ? v.v : v?.t === "int" ? String(v.v) : "";

function positionOf(cursor: string): number {
  const m = /^pos:(\d+)$/.exec(cursor);
  if (!m) mockError("INVALID_INPUT", "the page cursor is not one this session handed out");
  return Number(m[1]);
}

/** `{"user": {"name": v}}` from `user.name` (`insert_path`). */
function insertPath(doc: Record<string, unknown>, path: string, value: unknown) {
  const dot = path.indexOf(".");
  if (dot <= 0 || dot === path.length - 1) {
    if (path in doc)
      mockError("INVALID_INPUT", `${path} is written twice (as a field and as an object)`);
    doc[path] = value;
    return;
  }
  const head = path.slice(0, dot);
  const slot = (doc[head] ??= {});
  if (slot === null || typeof slot !== "object" || Array.isArray(slot))
    mockError("INVALID_INPUT", `${path}: ${head} is written as a value and as an object`);
  insertPath(slot as Record<string, unknown>, path.slice(dot + 1), value);
}

/** A partial document merged into a `_source`, as `_update` with `doc` does. */
function mergeDeep(
  into: Record<string, unknown>,
  from: Record<string, unknown>,
): Record<string, unknown> {
  for (const [k, v] of Object.entries(from)) {
    const cur = into[k];
    if (
      v !== null &&
      typeof v === "object" &&
      !Array.isArray(v) &&
      cur !== null &&
      typeof cur === "object" &&
      !Array.isArray(cur)
    )
      mergeDeep(cur as Record<string, unknown>, v as Record<string, unknown>);
    else into[k] = v;
  }
  return into;
}

/** The fields of a new index's `mappings.properties`, flattened. */
function fieldsOfBody(body: Json | null): Field[] {
  const out: Field[] = [];
  const walk = (props: Json | undefined, prefix: string) => {
    if (props?.k !== "obj") return;
    for (const [name, def] of props.v) {
      if (def.k !== "obj") continue;
      const type = def.v.find(([k]) => k === "type")?.[1];
      const inner = def.v.find(([k]) => k === "properties")?.[1];
      if (inner) walk(inner, `${prefix}${name}.`);
      else out.push({ path: `${prefix}${name}`, type: type?.k === "str" ? type.v : "object" });
    }
  };
  const mappings = body?.k === "obj" ? body.v.find(([k]) => k === "mappings")?.[1] : undefined;
  walk(mappings?.k === "obj" ? mappings.v.find(([k]) => k === "properties")?.[1] : undefined, "");
  return out.sort((a, b) => a.path.localeCompare(b.path));
}

/** The raw filters of a group (`RawFilter::QueryString`): DSL that calls a model is refused. */
function checkRaw(group: FilterGroup) {
  for (const node of group.items) {
    if (node.kind === "group") checkRaw(node);
    if (node.kind !== "raw" || !node.sql.trim().startsWith("{")) continue;
    let q: Json;
    try {
      q = parseStrict(node.sql.trim());
    } catch (e) {
      mockError("INVALID_INPUT", `raw filter: ${(e as Error).message}`);
    }
    const probe: EsRequest = {
      method: "GET",
      segments: ["_search"],
      query: [],
      body: { kind: "json", value: q },
    };
    if (classifyRequest(probe) !== "read")
      mockError(
        "INVALID_INPUT",
        "raw filter: this query calls a model or another service; run it in the console",
      );
  }
}

/** The group without its raw filters (`rawMatches` judges those). */
function stripRaw(group: FilterGroup): FilterGroup {
  return {
    ...group,
    items: group.items
      .filter((n) => n.kind !== "raw")
      .map((n): FilterNode => (n.kind === "group" ? { ...stripRaw(n), kind: "group" } : n)),
  };
}

/**
 * A raw filter over a hit, roughly as the cluster reads it: `field:value` and bare words of a
 * Lucene query string, or a `term` / `match` of Query DSL; anything else matches every hit.
 */
function rawMatches(group: FilterGroup, hit: Hit, fields: readonly Field[]): boolean {
  const one = (node: FilterNode): boolean => {
    if (node.kind === "group") return rawMatches(node, hit, fields);
    if (node.kind !== "raw") return true;
    const text = node.sql.trim();
    if (text === "" || text.startsWith("{")) return true;
    const value = (path: string) => {
      const v = at(hit.doc.source, path);
      return v === undefined ? "" : typeof v === "object" ? JSON.stringify(v) : String(v);
    };
    return text.split(/\s+AND\s+|\s+/).every((term) => {
      const colon = term.indexOf(":");
      if (colon > 0)
        return (
          value(term.slice(0, colon)).toLowerCase() ===
          term
            .slice(colon + 1)
            .replace(/^"|"$/g, "")
            .toLowerCase()
        );
      return fields.some((f) => value(f.path).toLowerCase().includes(term.toLowerCase()));
    });
  };
  const raws = group.items.filter((n) => n.kind === "raw" || n.kind === "group");
  if (raws.length === 0) return true;
  return group.conjunction === "and" ? raws.every(one) : raws.some(one);
}

/** The filter bar as Query DSL, for the page's request text (a sketch of `build::query`). */
function dslOf(filter: FilterGroup | null): unknown {
  if (!filter || filter.items.length === 0) return { match_all: {} };
  const plain = (v: Value | undefined): unknown =>
    v === undefined
      ? null
      : v.t === "null" || v.t === "default"
        ? null
        : v.t === "other"
          ? v.v.text
          : v.t === "bytes"
            ? v.v.base64
            : v.t === "array"
              ? v.v.map(plain)
              : v.v;
  const node = (n: FilterNode): unknown => {
    if (n.kind === "group") return dslOf(n);
    if (n.kind === "raw")
      return n.sql.trim().startsWith("{")
        ? (JSON.parse(n.sql) as unknown)
        : { query_string: { query: n.sql } };
    if (n.kind === "anyColumn")
      return { multi_match: { query: plain(n.values[0]), lenient: true } };
    const [a, b] = n.values;
    switch (n.op) {
      case "eq":
        return { term: { [n.column]: plain(a) } };
      case "ne":
        return { bool: { must_not: [{ term: { [n.column]: plain(a) } }] } };
      case "gt":
      case "ge":
      case "lt":
      case "le":
        return {
          range: {
            [n.column]: { [n.op === "ge" ? "gte" : n.op === "le" ? "lte" : n.op]: plain(a) },
          },
        };
      case "between":
        return { range: { [n.column]: { gte: plain(a), lte: plain(b) } } };
      case "isNull":
        return { bool: { must_not: [{ exists: { field: n.column } }] } };
      case "isNotNull":
        return { exists: { field: n.column } };
      case "in":
        return { terms: { [n.column]: n.values.map(plain) } };
      default:
        return { wildcard: { [n.column]: { value: `*${String(plain(a))}*` } } };
    }
  };
  const items = filter.items.map(node);
  return {
    bool:
      filter.conjunction === "and" ? { filter: items } : { should: items, minimum_should_match: 1 },
  };
}
