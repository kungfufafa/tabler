import type { StatementClass } from "@/features/safety/classify";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import { classifyDialect } from "./classify";
import { split } from "./scanner";

/**
 * Cases of the portable classifier and splitter in the shared format, which the backend's tests
 * run too: the hand-written `src-tauri/tests/fixtures/portable/*.cases.json` of the foundation's
 * test dialects, each dialect's generated `src-tauri/tests/fixtures/classify-generated/<d>.json`,
 * and a lane's own `classify.cases.json`. A failure names the text, what it got and what the case
 * wants.
 */
export interface ClassifyCase {
  sql: string;
  class: StatementClass;
}

export interface SplitCase {
  sql: string;
  /** The statements' text, in order. */
  statements: string[];
}

export interface SharedCases {
  classify: ClassifyCase[];
  split?: SplitCase[];
}

/** Where `dialect` disagrees with `cases` (empty when it agrees on every one). */
export function caseFailures(dialect: DialectLex, cases: SharedCases): string[] {
  const failures: string[] = [];
  for (const c of cases.classify) {
    const got = classifyDialect(dialect, c.sql);
    if (got !== c.class)
      failures.push(`classify ${JSON.stringify(c.sql)}: ${got}, want ${c.class}`);
  }
  for (const c of cases.split ?? []) {
    const got = split(dialect.lex, c.sql).map((s) => s.sql);
    if (JSON.stringify(got) !== JSON.stringify(c.statements))
      failures.push(
        `split ${JSON.stringify(c.sql)}: ${JSON.stringify(got)}, want ${JSON.stringify(c.statements)}`,
      );
  }
  return failures;
}
