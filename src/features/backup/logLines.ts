/**
 * Whether a line of a binary's output reports an error (psql / pg_* / mysql / mysqldump wording).
 * Mirrors `is_error_line` in src-tauri/src/backup/run.rs, which counts them.
 */
export function isErrorLine(line: string): boolean {
  const l = line.trimStart();
  return (
    l.includes("ERROR:") ||
    l.includes(": error:") ||
    l.startsWith("error:") ||
    l.includes("Got error") ||
    /^ERROR \d/.test(l) ||
    l.startsWith("FATAL:")
  );
}
