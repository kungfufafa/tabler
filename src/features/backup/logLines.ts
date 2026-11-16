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

/** The run's log as plain text for the clipboard: the command line, then every output line. */
export function outputText(run: {
  command: string | null;
  log: readonly { text: string }[];
}): string {
  const lines = run.log.map((l) => l.text);
  if (run.command) lines.unshift(`$ ${run.command}`);
  return lines.join("\n");
}
