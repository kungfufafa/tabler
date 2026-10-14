/**
 * A tiny Markdown parser for assistant replies: paragraphs, headings, fenced code blocks (an
 * unclosed fence while streaming is still a code block), bullet / numbered lists (nested), block
 * quotes, GitHub tables, rules; inline code, bold, italic, links and bare URLs. It builds a tree
 * that `Markdown.tsx` renders as React elements — no HTML is ever injected.
 */

export type Inline =
  | { t: "text"; text: string }
  | { t: "code"; text: string }
  | { t: "strong"; children: Inline[] }
  | { t: "em"; children: Inline[] }
  | { t: "link"; href: string; children: Inline[] };

export type Align = "left" | "center" | "right" | null;

export type Block =
  | { t: "p"; text: string }
  | { t: "h"; level: number; text: string }
  | { t: "code"; lang: string; text: string; closed: boolean }
  | { t: "list"; ordered: boolean; start: number; items: Block[][] }
  | { t: "quote"; blocks: Block[] }
  | { t: "table"; header: string[]; align: Align[]; rows: string[][] }
  | { t: "hr" };

const FENCE = /^( {0,3})(`{3,}|~{3,})[ \t]*([^`\s]*)[^`]*$/;
const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*#*[ \t]*$/;
const HR = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const QUOTE = /^ {0,3}> ?/;
const LIST_ITEM = /^( {0,3})([-*+]|\d{1,9}[.)])([ \t]+|$)(.*)$/;
const TABLE_SEP = /^ *\|? *:?-+:? *(\| *:?-+:? *)*\|? *$/;

const isBlank = (line: string) => line.trim() === "";
const indentOf = (line: string) => line.length - line.trimStart().length;

function startsBlock(line: string, next: string | undefined): boolean {
  return (
    FENCE.test(line) ||
    HEADING.test(line) ||
    HR.test(line) ||
    QUOTE.test(line) ||
    LIST_ITEM.test(line) ||
    isTableStart(line, next)
  );
}

function isTableStart(line: string, next: string | undefined): boolean {
  return line.includes("|") && next !== undefined && next.includes("-") && TABLE_SEP.test(next);
}

function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|") && !s.endsWith("\\|")) s = s.slice(0, -1);
  const cells: string[] = [];
  let cur = "";
  for (let i = 0; i < s.length; i++) {
    const ch = s[i]!;
    if (ch === "\\" && s[i + 1] === "|") {
      cur += "|";
      i++;
    } else if (ch === "|") {
      cells.push(cur.trim());
      cur = "";
    } else cur += ch;
  }
  cells.push(cur.trim());
  return cells;
}

function alignOf(cell: string): Align {
  const c = cell.trim();
  const left = c.startsWith(":");
  const right = c.endsWith(":");
  return left && right ? "center" : right ? "right" : left ? "left" : null;
}

/** Parses Markdown text into blocks. */
export function parseMarkdown(src: string): Block[] {
  return parseLines(src.replace(/\r\n?/g, "\n").split("\n"));
}

function parseLines(lines: string[]): Block[] {
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    if (isBlank(line)) {
      i++;
      continue;
    }
    const fence = FENCE.exec(line);
    if (fence) {
      const indent = fence[1]!.length;
      const marker = fence[2]!;
      const lang = fence[3] ?? "";
      const body: string[] = [];
      let closed = false;
      i++;
      while (i < lines.length) {
        const l = lines[i]!;
        const t = l.trim();
        if (
          indentOf(l) <= 3 + indent &&
          t.startsWith(marker[0]!.repeat(marker.length)) &&
          new RegExp(`^\\${marker[0]}+$`).test(t)
        ) {
          closed = true;
          i++;
          break;
        }
        body.push(l.slice(Math.min(indent, indentOf(l))));
        i++;
      }
      out.push({ t: "code", lang: lang.toLowerCase(), text: body.join("\n"), closed });
      continue;
    }
    if (HR.test(line)) {
      out.push({ t: "hr" });
      i++;
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      out.push({ t: "h", level: heading[1]!.length, text: heading[2] ?? "" });
      i++;
      continue;
    }
    if (QUOTE.test(line)) {
      const inner: string[] = [];
      while (i < lines.length && !isBlank(lines[i]!)) {
        const l = lines[i]!;
        inner.push(QUOTE.test(l) ? l.replace(QUOTE, "") : l);
        i++;
      }
      out.push({ t: "quote", blocks: parseLines(inner) });
      continue;
    }
    if (isTableStart(line, lines[i + 1])) {
      const header = splitRow(line);
      const align = splitRow(lines[i + 1]!).map(alignOf);
      const rows: string[][] = [];
      i += 2;
      while (i < lines.length && !isBlank(lines[i]!) && lines[i]!.includes("|")) {
        rows.push(splitRow(lines[i]!));
        i++;
      }
      out.push({ t: "table", header, align, rows });
      continue;
    }
    const item = LIST_ITEM.exec(line);
    if (item) {
      const [list, next] = parseList(lines, i);
      out.push(list);
      i = next;
      continue;
    }
    // Paragraph: until a blank line or the start of another block.
    const para: string[] = [line.trim()];
    i++;
    while (i < lines.length && !isBlank(lines[i]!) && !startsBlock(lines[i]!, lines[i + 1])) {
      para.push(lines[i]!.trim());
      i++;
    }
    out.push({ t: "p", text: para.join("\n") });
  }
  return out;
}

/** A list starting at `lines[start]`; returns it and the index after it. */
function parseList(lines: string[], start: number): [Block, number] {
  const first = LIST_ITEM.exec(lines[start]!)!;
  const ordered = /\d/.test(first[2]!);
  const startNum = ordered ? Number.parseInt(first[2]!, 10) : 1;
  const items: Block[][] = [];
  let i = start;
  while (i < lines.length) {
    const m = LIST_ITEM.exec(lines[i]!);
    if (!m || /\d/.test(m[2]!) !== ordered) break;
    const contentIndent = m[1]!.length + m[2]!.length + Math.max(1, Math.min(m[3]!.length, 4));
    const body: string[] = [m[4] ?? ""];
    i++;
    while (i < lines.length) {
      const l = lines[i]!;
      if (isBlank(l)) {
        // A blank line continues the item only when indented content follows.
        let j = i + 1;
        while (j < lines.length && isBlank(lines[j]!)) j++;
        if (j < lines.length && indentOf(lines[j]!) >= contentIndent) {
          for (; i < j; i++) body.push("");
          continue;
        }
        break;
      }
      if (indentOf(l) >= contentIndent) {
        body.push(l.slice(contentIndent));
        i++;
        continue;
      }
      if (LIST_ITEM.test(l) || startsBlock(l, lines[i + 1])) break;
      // Lazy continuation of the item's paragraph.
      body.push(l.trim());
      i++;
    }
    items.push(parseLines(body));
    // Blank lines between items of the same list.
    let j = i;
    while (j < lines.length && isBlank(lines[j]!)) j++;
    const nextItem = j < lines.length ? LIST_ITEM.exec(lines[j]!) : null;
    if (nextItem && /\d/.test(nextItem[2]!) === ordered) i = j;
    else break;
  }
  return [{ t: "list", ordered, start: startNum, items }, i];
}

const URL_RE = /^https?:\/\/[^\s<>()]*[^\s<>().,;:!?'"\]]/;
const PUNCT = /[!-/:-@[-`{-~]/;

/** Parses the inline Markdown of a paragraph, heading or table cell. */
export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let text = "";
  const flush = () => {
    if (text) out.push({ t: "text", text });
    text = "";
  };
  let i = 0;
  while (i < src.length) {
    const ch = src[i]!;
    if (ch === "\\" && i + 1 < src.length && PUNCT.test(src[i + 1]!)) {
      text += src[i + 1];
      i += 2;
      continue;
    }
    if (ch === "`") {
      let n = 0;
      while (src[i + n] === "`") n++;
      const ticks = "`".repeat(n);
      const end = src.indexOf(ticks, i + n);
      if (end > 0 && src[end + n] !== "`") {
        flush();
        let code = src.slice(i + n, end).replace(/\n/g, " ");
        if (code.length > 2 && code.startsWith(" ") && code.endsWith(" ")) code = code.slice(1, -1);
        out.push({ t: "code", text: code });
        i = end + n;
        continue;
      }
      text += ticks;
      i += n;
      continue;
    }
    if ((ch === "*" || ch === "_") && src[i + 1] === ch) {
      const marker = ch + ch;
      const end = src.indexOf(marker, i + 2);
      if (end > i + 2 && src[i + 2] !== " ") {
        flush();
        out.push({ t: "strong", children: parseInline(src.slice(i + 2, end)) });
        i = end + 2;
        continue;
      }
    } else if (ch === "*" || ch === "_") {
      const prev = src[i - 1];
      const wordBefore = prev !== undefined && /[\p{L}\p{N}]/u.test(prev);
      if (!(ch === "_" && wordBefore) && src[i + 1] !== undefined && src[i + 1] !== " ") {
        let end = i + 1;
        // The closer: the same character, not doubled, not after a space.
        while ((end = src.indexOf(ch, end)) > 0) {
          const after = src[end + 1];
          if (
            src[end - 1] !== " " &&
            src[end + 1] !== ch &&
            !(ch === "_" && after !== undefined && /[\p{L}\p{N}]/u.test(after))
          )
            break;
          end++;
        }
        if (end > i + 1) {
          flush();
          out.push({ t: "em", children: parseInline(src.slice(i + 1, end)) });
          i = end + 1;
          continue;
        }
      }
    } else if (ch === "[") {
      const close = src.indexOf("]", i + 1);
      if (close > i && src[close + 1] === "(") {
        const end = src.indexOf(")", close + 2);
        const href = end > 0 ? src.slice(close + 2, end).trim() : "";
        if (end > 0 && /^https?:\/\/\S+$/.test(href)) {
          flush();
          out.push({ t: "link", href, children: parseInline(src.slice(i + 1, close)) });
          i = end + 1;
          continue;
        }
      }
    } else if (ch === "h" && (i === 0 || !/[\p{L}\p{N}]/u.test(src[i - 1]!))) {
      const m = URL_RE.exec(src.slice(i));
      if (m) {
        flush();
        out.push({ t: "link", href: m[0], children: [{ t: "text", text: m[0] }] });
        i += m[0].length;
        continue;
      }
    }
    text += ch;
    i++;
  }
  flush();
  return out;
}

const SQL_LANGS = new Set([
  "sql",
  "pgsql",
  "plpgsql",
  "postgres",
  "postgresql",
  "psql",
  "mysql",
  "mariadb",
  "sqlite",
  "sqlite3",
  "tsql",
  "plsql",
]);

const SQL_START =
  /^\s*(select|with|insert|update|delete|create|alter|drop|explain|show|describe|truncate|grant|revoke|begin|set|pragma|vacuum|analyze|call|merge|replace)\b/i;

/** A code block holding SQL: tagged with a SQL language, or untagged and starting like SQL. */
export function isSqlCode(lang: string, text: string): boolean {
  if (SQL_LANGS.has(lang.toLowerCase())) return true;
  return lang === "" && SQL_START.test(text);
}

/** The SQL code blocks of a reply, in order (nested ones included). */
export function sqlBlocks(src: string): string[] {
  const out: string[] = [];
  const walk = (blocks: Block[]) => {
    for (const b of blocks) {
      if (b.t === "code" && isSqlCode(b.lang, b.text)) out.push(b.text);
      else if (b.t === "list") b.items.forEach(walk);
      else if (b.t === "quote") walk(b.blocks);
    }
  };
  walk(parseMarkdown(src));
  return out;
}
