import { isPunct, scan, text, upper, type Token } from "@/features/drivers/portable/scanner";
import { LEX } from "./classify";

/**
 * `sql::redact` on the Vertica dialect (`portable/redact.rs` with the dialect's `redact` and
 * `redact_calls` knobs), for the mock's error messages: the string literal after a password
 * keyword, or in a function's secret argument, becomes the mask, under every reading.
 */

/** `sqlgen::admin::PASSWORD_MASK`. */
const MASK = "'********'";

/** `SPEC.knobs.redact` of `sqlgen/dialect/vertica.rs`. */
const KEYWORDS = [
  "PASSWORD",
  "IDENTIFIED",
  "REPLACE",
  "AWSAUTH",
  "AWS_SECRET_ACCESS_KEY",
  "AWS_SESSION_TOKEN",
  "AWSSESSIONTOKEN",
  "GCSAUTH",
  "S3BUCKETCREDENTIALS",
  "AZURESTORAGECREDENTIALS",
  "LDAPLINKBINDPSWD",
  "BIND_PASSWORD",
  "CLIENT_SECRET",
  "SHARED_SECRET",
  "KAFKA_CONF_SECRET",
  "KAFKA_CONF",
  "PARAMETERS",
];

/** `SPEC.knobs.redact_calls`: the function and its secret argument (1-based). */
const CALLS = new Map<string, number>([
  ["AWS_SET_CONFIG", 2],
  ["SET_CONFIG_PARAMETER", 2],
  ["LDAP_LINK_DRYRUN_CONNECT", 3],
  ["LDAP_LINK_DRYRUN_SEARCH", 3],
  ["LDAP_LINK_DRYRUN_SYNC", 3],
]);

/** Words that may stand between a password keyword and its literal. */
const LINKS = ["BY", "AS", "WITH", "USING"];

/** `sql` with its secrets masked. */
export function redact(sql: string): string {
  const spans: [number, number][] = [];
  let chars: readonly string[] = Array.from(sql);
  for (const reading of LEX.lex.readings) {
    const s = scan(LEX.lex, reading, sql);
    chars = s.chars;
    const code = s.tokens.filter((t) => t.kind !== "space" && t.kind !== "comment");
    const keep = (t: Token) => {
      if (text(s, t) !== MASK) spans.push([t.start, t.end]);
    };
    code.forEach((t, k) => {
      if (t.kind !== "word") return;
      const word = upper(s, t);
      const argument = CALLS.get(word);
      const open = code[k + 1];
      if (argument !== undefined && open !== undefined && isPunct(s, open, "(")) {
        let depth = 0;
        let at = 1;
        for (const a of code.slice(k + 2)) {
          if (isPunct(s, a, "(")) depth++;
          else if (isPunct(s, a, ")")) {
            if (depth === 0) break;
            depth--;
          } else if (depth === 0 && isPunct(s, a, ",")) at++;
          else if (at === argument && a.kind === "str") keep(a);
        }
      }
      if (!KEYWORDS.includes(word)) return;
      let otherWords = 0;
      for (const n of code.slice(k + 1)) {
        if (n.kind === "str") {
          keep(n);
          break;
        }
        // `=`, `(`, and the named-argument arrows `:=` and `=>`.
        if (n.kind === "punct" && ["=", "(", ":", ">"].some((c) => isPunct(s, n, c))) continue;
        if (n.kind === "word" && LINKS.includes(upper(s, n))) continue;
        if ((n.kind === "word" || n.kind === "ident") && otherWords === 0) {
          otherWords++;
          continue;
        }
        break;
      }
    });
  }
  if (spans.length === 0) return sql;
  // One mask over the literals the readings found, overlapping ones merged.
  spans.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged: [number, number][] = [];
  for (const [start, end] of spans) {
    const last = merged[merged.length - 1];
    if (last && start < last[1]) last[1] = Math.max(last[1], end);
    else merged.push([start, end]);
  }
  let out = "";
  let at = 0;
  for (const [start, end] of merged) {
    out += chars.slice(at, start).join("") + MASK;
    at = end;
  }
  return out + chars.slice(at).join("");
}
