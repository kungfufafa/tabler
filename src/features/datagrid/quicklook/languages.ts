import {
  HighlightStyle,
  StreamLanguage,
  syntaxHighlighting,
  type StreamParser,
} from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { highlightTree, tagHighlighter, tags as t } from "@lezer/highlight";

/**
 * JSON and XML syntax colouring for the Quick Look editor and the right sidebar's fields. The app
 * has no `@codemirror/lang-json` / `lang-xml`, so these are small stream tokenizers (keys,
 * strings, numbers, literals; tags, attributes, comments) — colouring only, no folding or linting.
 */
export type ValueLanguage = "json" | "xml";

interface JsonState {
  /** Inside an object, expecting a key (the next string is a property name). */
  stack: Array<"{" | "[">;
  expectKey: boolean;
}

const json: StreamParser<JsonState> = {
  name: "json",
  startState: () => ({ stack: [], expectKey: false }),
  copyState: (s) => ({ stack: [...s.stack], expectKey: s.expectKey }),
  token(stream, state) {
    if (stream.eatSpace()) return null;
    const ch = stream.peek();
    if (ch === '"') {
      stream.next();
      let escaped = false;
      for (;;) {
        const c = stream.next();
        if (c === undefined) break;
        if (escaped) escaped = false;
        else if (c === "\\") escaped = true;
        else if (c === '"') break;
      }
      const key = state.expectKey;
      state.expectKey = false;
      return key ? "propertyName" : "string";
    }
    if (stream.match(/^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/)) return "number";
    if (stream.match(/^(?:true|false)\b/)) return "bool";
    if (stream.match(/^null\b/)) return "null";
    stream.next();
    switch (ch) {
      case "{":
        state.stack.push("{");
        state.expectKey = true;
        return "brace";
      case "[":
        state.stack.push("[");
        state.expectKey = false;
        return "squareBracket";
      case "}":
      case "]":
        state.stack.pop();
        state.expectKey = false;
        return ch === "}" ? "brace" : "squareBracket";
      case ",":
        state.expectKey = state.stack[state.stack.length - 1] === "{";
        return "separator";
      case ":":
        state.expectKey = false;
        return "punctuation";
      default:
        return "invalid";
    }
  },
};

interface XmlState {
  /** Inside `<…>` (a tag), a comment, CDATA or a processing instruction. */
  mode: "text" | "tag" | "comment" | "cdata" | "pi";
  /** The next name in a tag is its tag name. */
  tagName: boolean;
}

const xml: StreamParser<XmlState> = {
  name: "xml",
  startState: () => ({ mode: "text", tagName: false }),
  copyState: (s) => ({ ...s }),
  token(stream, state) {
    switch (state.mode) {
      case "comment":
        if (stream.skipTo("-->")) {
          stream.match("-->");
          state.mode = "text";
        } else stream.skipToEnd();
        return "comment";
      case "cdata":
        if (stream.skipTo("]]>")) {
          stream.match("]]>");
          state.mode = "text";
        } else stream.skipToEnd();
        return "string";
      case "pi":
        if (stream.skipTo("?>")) {
          stream.match("?>");
          state.mode = "text";
        } else stream.skipToEnd();
        return "meta";
      case "tag": {
        if (stream.eatSpace()) return null;
        if (stream.match("/>") || stream.match(">")) {
          state.mode = "text";
          return "angleBracket";
        }
        if (stream.match(/^"[^"]*"?/) || stream.match(/^'[^']*'?/)) return "attributeValue";
        if (stream.match(/^[\w:.-]+/)) {
          const name = state.tagName;
          state.tagName = false;
          return name ? "tagName" : "attributeName";
        }
        stream.next();
        return "operator";
      }
      default: {
        if (stream.match("<!--")) {
          state.mode = "comment";
          return "comment";
        }
        if (stream.match("<![CDATA[")) {
          state.mode = "cdata";
          return "string";
        }
        if (stream.match("<?")) {
          state.mode = "pi";
          return "meta";
        }
        if (stream.match(/^<\/?/)) {
          state.mode = "tag";
          state.tagName = true;
          return "angleBracket";
        }
        if (stream.match(/^&[#\w]+;/)) return "character";
        stream.next();
        while (!stream.eol() && stream.peek() !== "<" && stream.peek() !== "&") stream.next();
        return null;
      }
    }
  },
};

const LANGUAGES = {
  json: StreamLanguage.define(json),
  xml: StreamLanguage.define(xml),
};

/** The value colours (the SQL editor's syntax palette, see `index.css` `--syn-*`). */
export const valueHighlightStyle = HighlightStyle.define([
  { tag: t.propertyName, color: "var(--syn-keyword)" },
  { tag: [t.string, t.attributeValue, t.character], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--syn-number)" },
  { tag: t.tagName, color: "var(--syn-keyword)", fontWeight: "600" },
  { tag: t.attributeName, color: "var(--syn-number)" },
  { tag: [t.comment, t.meta], color: "var(--syn-comment)", fontStyle: "italic" },
  {
    tag: [t.brace, t.squareBracket, t.separator, t.punctuation, t.angleBracket],
    color: "var(--muted-foreground)",
  },
  { tag: t.invalid, color: "var(--error)" },
]);

/** CodeMirror extensions that colour `lang`. */
export function languageExtension(lang: ValueLanguage): Extension {
  return [LANGUAGES[lang], syntaxHighlighting(valueHighlightStyle)];
}

/** The language a value's text reads as: JSON for JSON columns (or text that parses), XML for markup. */
export function detectLanguage(kind: string, dataType: string, text: string): ValueLanguage | null {
  if (kind === "json") return "json";
  if (/^\s*xml\b/i.test(dataType)) return "xml";
  const head = text.trimStart();
  if (head.startsWith("<") && /<\/?[A-Za-z_?!][^>]*>/.test(head.slice(0, 2000))) return "xml";
  if ((head.startsWith("{") || head.startsWith("[")) && text.length < 2_000_000) {
    try {
      JSON.parse(text);
      return "json";
    } catch {
      return null;
    }
  }
  return null;
}

/** Most characters coloured statically (larger values show plain). */
const STATIC_LIMIT = 200_000;

/** The same colours as Tailwind classes, for static highlighting outside CodeMirror. */
const staticHighlighter = tagHighlighter([
  { tag: t.propertyName, class: "text-[color:var(--syn-keyword)]" },
  { tag: [t.string, t.attributeValue, t.character], class: "text-[color:var(--syn-string)]" },
  { tag: [t.number, t.bool, t.null], class: "text-[color:var(--syn-number)]" },
  { tag: t.tagName, class: "text-[color:var(--syn-keyword)] font-semibold" },
  { tag: t.attributeName, class: "text-[color:var(--syn-number)]" },
  { tag: [t.comment, t.meta], class: "text-[color:var(--syn-comment)] italic" },
  {
    tag: [t.brace, t.squareBracket, t.separator, t.punctuation, t.angleBracket],
    class: "text-muted-foreground",
  },
  { tag: t.invalid, class: "text-error" },
]);

/**
 * `text` split into runs with their colour classes (for a highlighted backdrop behind a text
 * area); one plain run when too long.
 */
export function highlightRuns(
  text: string,
  lang: ValueLanguage,
): Array<{ text: string; className?: string }> {
  if (text.length > STATIC_LIMIT) return [{ text }];
  const tree = LANGUAGES[lang].parser.parse(text);
  const out: Array<{ text: string; className?: string }> = [];
  let at = 0;
  highlightTree(tree, staticHighlighter, (from, to, classes) => {
    if (from > at) out.push({ text: text.slice(at, from) });
    out.push({ text: text.slice(from, to), className: classes });
    at = to;
  });
  if (at < text.length) out.push({ text: text.slice(at) });
  return out;
}
