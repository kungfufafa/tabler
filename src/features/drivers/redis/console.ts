import { StreamLanguage, type StringStream } from "@codemirror/language";
import type { ConsoleUi } from "../types";
import { consoleRanges } from "./cli";
import { classifyCommand } from "./classify";

/**
 * The console of a Redis session: redis-cli command lines, split and classified as the backend
 * does (`cli.ts`, `classify.ts`), highlighted by a small tokenizer: the command name, quoted
 * arguments, numbers and the option words most commands take. No formatter: arguments are argv,
 * and re-spacing a glob or a quoted value would change it (TablePlus's Beautify broke globs).
 */

const OPTIONS = new Set([
  "MATCH",
  "COUNT",
  "TYPE",
  "WITHSCORES",
  "LIMIT",
  "EX",
  "PX",
  "EXAT",
  "PXAT",
  "NX",
  "XX",
  "GT",
  "LT",
  "KEEPTTL",
  "GET",
  "BYSCORE",
  "BYLEX",
  "REV",
  "STREAMS",
  "NOVALUES",
  "ASYNC",
  "SYNC",
  "STORE",
  "ALPHA",
  "ASC",
  "DESC",
]);

interface State {
  /** The next word is a command's name (at a line's start or after `;`). */
  head: boolean;
  /** Inside a double- or single-quoted argument that runs over lines. */
  quote: '"' | "'" | null;
}

function quoted(stream: StringStream, state: State): void {
  const q = state.quote!;
  while (!stream.eol()) {
    const c = stream.next();
    if (c === "\\" && (q === '"' || stream.peek() === "'")) stream.next();
    else if (c === q) {
      state.quote = null;
      return;
    }
  }
}

function token(stream: StringStream, state: State): string | null {
  if (stream.sol() && state.quote === null) state.head = true;
  if (state.quote !== null) {
    quoted(stream, state);
    return "string";
  }
  if (stream.eatSpace()) return null;
  const ch = stream.peek();
  if (ch === ";") {
    stream.next();
    state.head = true;
    return "punctuation";
  }
  if (ch === '"' || ch === "'") {
    stream.next();
    state.quote = ch;
    quoted(stream, state);
    state.head = false;
    return "string";
  }
  stream.eatWhile(/[^\s;"']/);
  const word = stream.current();
  if (state.head) {
    state.head = false;
    return "keyword";
  }
  if (/^[-+]?(\d+(\.\d*)?|\.\d+|inf)$/i.test(word)) return "number";
  if (OPTIONS.has(word.toUpperCase())) return "atom";
  return "variableName";
}

const redisLanguage = StreamLanguage.define<State>({
  name: "redis",
  startState: () => ({ head: true, quote: null }),
  token,
});

export const redisConsole: ConsoleUi = {
  language: () => redisLanguage,
  split: consoleRanges,
  classify: classifyCommand,
  fileExtension: "redis",
  placeholder: "SCAN 0 MATCH user:* COUNT 100\nHGETALL user:1\nSET greeting hello",
};
