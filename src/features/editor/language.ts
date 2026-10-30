import type { Extension } from "@codemirror/state";
import { useEffect, useState } from "react";
import { loadDialectLex } from "@/features/drivers/portable/classify";
import { split as portableSplit } from "@/features/drivers/portable/scanner";
import {
  laneBuiltFor,
  loadDialectUi,
  loadDriverUi,
  useDialectUi,
  useDriverUi,
} from "@/features/drivers/registry";
import type { ConsoleUi, DialectUi } from "@/features/drivers/types";
import { capsOf, connectionDialect, has, sessionDialect } from "@/lib/caps";
import type { AppInfo, ConnectionInfo, SessionCaps, SqlDialectId } from "@/lib/ipc";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import { useAppInfo, useAppInfoStore } from "@/stores/appInfo";
import { useConnectionsStore } from "@/stores/connections";
import { splitStatements, type SqlDialect, type StatementRange } from "./scanner";

/**
 * What the SQL editor of a connection reads (plan F9b, `languageOf`): a core dialect, which the
 * core scanner splits and the core completion, formatter and generators know, or a lane's
 * language: an extension SQL dialect (split by the portable splitter over its lexer profile,
 * highlighted by its `DialectUi`) or a native store's console (`ConsoleUi`: redis-cli, a mongosh
 * subset). The backend splits a run the same way (`split_console`, the dialect's splitter).
 */
export interface LaneLanguage {
  /** What the editor reads ("T-SQL", "Redis"). */
  label: string;
  /** Highlighting (and an extension dialect's keywords). */
  extension: () => Extension;
  /** The statements or commands of the text, as the backend splits them (UTF-16 offsets). */
  split: (text: string) => StatementRange[];
  /** sql-formatter's language; null: no Format. */
  formatter: string | null;
  /** The extension of the tab's files ("sql" for SQL, a console's own). */
  fileExtension: string;
  placeholder?: string;
}

/** A core dialect, or a lane's language (see `LaneLanguage`). */
export type EditorLanguage = SqlDialect | LaneLanguage;

/**
 * Whether a connection has a query editor (its session offers one: a core dialect, an extension
 * SQL dialect or a console), and the core dialect its text is seeded with (null for a lane's
 * language, which the editor takes when it mounts). Null when it has none.
 */
export function editorOf(info: ConnectionInfo | undefined): { dialect: SqlDialect | null } | null {
  if (!info) return { dialect: "postgres" };
  return has(info, "editor") ? { dialect: connectionDialect(info) } : null;
}

/** The core dialect of a language; null for a lane's. */
export function coreLanguage(language: EditorLanguage): SqlDialect | null {
  return typeof language === "string" ? language : null;
}

/** The statements of `text` (UTF-16 offsets), as the language splits them. */
export function splitWith(language: EditorLanguage, text: string): StatementRange[] {
  return typeof language === "string" ? splitStatements(text, language) : language.split(text);
}

/** The text of each statement of `text`. */
export function statementsWith(language: EditorLanguage, text: string): string[] {
  return splitWith(language, text).map((s) => text.slice(s.start, s.end));
}

// The lexer profiles of the extension dialects, loaded once each (null: the dialect has none).
const lexes = new Map<SqlDialectId, DialectLex | null>();
const loadingLexes = new Map<SqlDialectId, Promise<DialectLex | null>>();

function lexOf(dialect: SqlDialectId): DialectLex | null | undefined {
  if (lexes.has(dialect)) return lexes.get(dialect);
  if (!loadingLexes.has(dialect))
    loadingLexes.set(
      dialect,
      loadDialectLex(dialect)
        .catch(() => null)
        .then((lex) => {
          lexes.set(dialect, lex);
          return lex;
        }),
    );
  return undefined;
}

// One language object per lane module, so the editor reconfigures only when one loads.
const dialectLanguages = new WeakMap<DialectLex, Map<DialectUi | null, LaneLanguage>>();
const consoleLanguages = new WeakMap<ConsoleUi, LaneLanguage>();

function dialectLanguage(
  dialect: SqlDialectId,
  lex: DialectLex | null,
  ui: DialectUi | null,
): LaneLanguage | null {
  // Without a lexer profile nothing can split the text as the backend does.
  if (lex === null) return null;
  const byUi = dialectLanguages.get(lex) ?? new Map<DialectUi | null, LaneLanguage>();
  dialectLanguages.set(lex, byUi);
  let language = byUi.get(ui);
  if (!language) {
    language = {
      label: ui?.label ?? dialect,
      extension: () => (ui ? ui.codemirror().extension : []),
      split: (text) => portableSplit(lex.lex, text),
      formatter: ui?.formatter ?? null,
      fileExtension: "sql",
    };
    byUi.set(ui, language);
  }
  return language;
}

function consoleLanguage(label: string, console: ConsoleUi): LaneLanguage {
  let language = consoleLanguages.get(console);
  if (!language) {
    language = {
      label,
      extension: console.language,
      split: console.split,
      formatter: null,
      fileExtension: console.fileExtension,
      placeholder: console.placeholder,
    };
    consoleLanguages.set(console, language);
  }
  return language;
}

const LABELS: Partial<Record<SessionCaps["language"], string>> = {
  redisCli: "Redis",
  mongoShell: "MongoDB Shell",
  esConsole: "Console",
  partiql: "PartiQL",
  cql: "CQL",
};

/** Whether this build has the lane of the session's dialect or driver. */
function laneBuilt(caps: SessionCaps, info: Pick<AppInfo, "kinds"> | null): boolean {
  return caps.dialect !== null
    ? laneBuiltFor({ dialect: caps.dialect }, info)
    : laneBuiltFor({ driver: caps.driver }, info);
}

/** What a lane's lookups answered: the language, null without one, undefined while loading. */
function laneLanguage(
  caps: SessionCaps,
  dialectUi: DialectUi | null | undefined,
  console: ConsoleUi | null | undefined,
  lex: DialectLex | null | undefined,
): EditorLanguage | null | undefined {
  if (caps.dialect !== null) {
    if (dialectUi === undefined || lex === undefined) return undefined;
    return dialectLanguage(caps.dialect, lex, dialectUi);
  }
  if (console === undefined) return undefined;
  return console ? consoleLanguage(LABELS[caps.language] ?? "Console", console) : null;
}

/**
 * The language of a connection's editor (outside React): PostgreSQL's while the connection is
 * unknown, a core dialect at once, a lane's once it has loaded (undefined until then; the load
 * starts here), null when the session has no editor (or its lane gives no language).
 */
export function languageOf(info: ConnectionInfo | undefined): EditorLanguage | null | undefined {
  if (!info) return "postgres";
  const caps = capsOf(info);
  if (!has(caps, "editor")) return null;
  const core = sessionDialect(caps);
  if (core !== null) return core;
  if (!laneBuilt(caps, useAppInfoStore.getState().info)) return null;
  const dialectUi = caps.dialect !== null ? loadDialectUi(caps.dialect) : null;
  const console = caps.dialect === null ? loadDriverUi(caps.driver)?.console : null;
  const lex = caps.dialect !== null ? lexOf(caps.dialect) : null;
  return laneLanguage(caps, dialectUi, console === undefined ? undefined : (console ?? null), lex);
}

/** `languageOf` as a hook: renders again once a lane's module and lexer profile have loaded. */
export function useEditorLanguage(connectionId: string): EditorLanguage | null | undefined {
  const info = useConnectionsStore((s) => s.byId[connectionId]?.info);
  const appInfo = useAppInfo();
  const caps = info ? capsOf(info) : null;
  // A lane's language, where the session is a lane's and this build has the lane.
  const lane = caps !== null && sessionDialect(caps) === null && laneBuilt(caps, appInfo);
  const extension = lane ? caps.dialect : null;
  const dialectUi = useDialectUi(extension);
  const driverUi = useDriverUi(lane && caps.dialect === null ? caps.driver : null);
  const [lex, setLex] = useState<{ dialect: SqlDialectId; lex: DialectLex | null } | null>(null);
  useEffect(() => {
    if (extension === null || lexes.has(extension)) return;
    let live = true;
    lexOf(extension);
    void loadingLexes.get(extension)?.then((loaded) => {
      if (live) setLex({ dialect: extension, lex: loaded });
    });
    return () => {
      live = false;
    };
  }, [extension]);
  if (!info || !caps) return "postgres";
  if (!has(caps, "editor")) return null;
  const core = sessionDialect(caps);
  if (core !== null) return core;
  if (!lane) return null;
  const loadedLex =
    extension === null
      ? null
      : lexes.has(extension)
        ? lexes.get(extension)!
        : lex?.dialect === extension
          ? lex.lex
          : undefined;
  const console = driverUi === undefined ? undefined : (driverUi?.console ?? null);
  return laneLanguage(caps, dialectUi, console, loadedLex);
}
