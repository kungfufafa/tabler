import { leadingKeyword } from "@/lib/sql/statement";

/** First keywords of statements that can change the schema (mirrors the backend's `changes_schema`). */
const SCHEMA_KEYWORDS = new Set(["CREATE", "ALTER", "DROP", "RENAME", "TRUNCATE", "COMMENT"]);

/**
 * Whether `sql` (one statement) can change the schema, so cached table lists, metadata and
 * completion are stale once it ran: its first word, after whitespace, `--` / `#` line comments,
 * `/* … *\/` block comments and opening parentheses, is CREATE, ALTER, DROP, RENAME, TRUNCATE or
 * COMMENT.
 */
export function changesSchema(sql: string): boolean {
  const word = leadingKeyword(sql);
  return word !== null && SCHEMA_KEYWORDS.has(word);
}
