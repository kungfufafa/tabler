import { describe, expect, it } from "vitest";
import { isSqlCode, parseInline, parseMarkdown, sqlBlocks } from "./markdown";

describe("parseMarkdown", () => {
  it("parses paragraphs, headings, rules and fenced code", () => {
    const blocks = parseMarkdown(
      "## Top customers\n\nUse this query:\nit sums orders.\n\n```sql\nSELECT id\nFROM customers;\n```\n\n---\n~~~\nplain\n~~~",
    );
    expect(blocks).toEqual([
      { t: "h", level: 2, text: "Top customers" },
      { t: "p", text: "Use this query:\nit sums orders." },
      { t: "code", lang: "sql", text: "SELECT id\nFROM customers;", closed: true },
      { t: "hr" },
      { t: "code", lang: "", text: "plain", closed: true },
    ]);
  });

  it("keeps an unclosed fence (a reply still streaming) as code", () => {
    const blocks = parseMarkdown("Here:\n```postgresql\nSELECT 1");
    expect(blocks[1]).toEqual({ t: "code", lang: "postgresql", text: "SELECT 1", closed: false });
  });

  it("does not close a fence on an inner opening fence", () => {
    const blocks = parseMarkdown("````md\n```sql\nx\n```\n````");
    expect(blocks).toEqual([{ t: "code", lang: "md", text: "```sql\nx\n```", closed: true }]);
  });

  it("parses nested lists with code inside items", () => {
    const blocks = parseMarkdown(
      "1. Add an index:\n   ```sql\n   CREATE INDEX i ON t (a);\n   ```\n2. Re-run:\n   - EXPLAIN\n   - compare\n\nDone.",
    );
    expect(blocks[0]).toEqual({
      t: "list",
      ordered: true,
      start: 1,
      items: [
        [
          { t: "p", text: "Add an index:" },
          { t: "code", lang: "sql", text: "CREATE INDEX i ON t (a);", closed: true },
        ],
        [
          { t: "p", text: "Re-run:" },
          {
            t: "list",
            ordered: false,
            start: 1,
            items: [[{ t: "p", text: "EXPLAIN" }], [{ t: "p", text: "compare" }]],
          },
        ],
      ],
    });
    expect(blocks[1]).toEqual({ t: "p", text: "Done." });
  });

  it("parses loose lists, quotes and tables", () => {
    const blocks = parseMarkdown(
      "- a\n\n- b\n\n> note\n> more\n\n| col | type |\n|:---|---:|\n| id | `int` |\n| a\\|b | text |",
    );
    expect(blocks[0]).toMatchObject({ t: "list", items: [[{ t: "p" }], [{ t: "p" }]] });
    expect(blocks[1]).toEqual({ t: "quote", blocks: [{ t: "p", text: "note\nmore" }] });
    expect(blocks[2]).toEqual({
      t: "table",
      header: ["col", "type"],
      align: ["left", "right"],
      rows: [
        ["id", "`int`"],
        ["a|b", "text"],
      ],
    });
  });
});

describe("parseInline", () => {
  it("parses code, bold, italic and links", () => {
    expect(
      parseInline("Use `user_id` with **care** and *joins*, see [docs](https://x.dev/a)."),
    ).toEqual([
      { t: "text", text: "Use " },
      { t: "code", text: "user_id" },
      { t: "text", text: " with " },
      { t: "strong", children: [{ t: "text", text: "care" }] },
      { t: "text", text: " and " },
      { t: "em", children: [{ t: "text", text: "joins" }] },
      { t: "text", text: ", see " },
      { t: "link", href: "https://x.dev/a", children: [{ t: "text", text: "docs" }] },
      { t: "text", text: "." },
    ]);
  });

  it("leaves identifiers, stray markers and unsafe links as text", () => {
    expect(parseInline("snake_case_name and a * b")).toEqual([
      { t: "text", text: "snake_case_name and a * b" },
    ]);
    expect(parseInline("[x](javascript:alert(1))")).toEqual([
      { t: "text", text: "[x](javascript:alert(1))" },
    ]);
    expect(parseInline("\\*not em\\*")).toEqual([{ t: "text", text: "*not em*" }]);
  });

  it("links bare URLs without trailing punctuation", () => {
    expect(parseInline("See https://www.postgresql.org/docs/.")).toEqual([
      { t: "text", text: "See " },
      {
        t: "link",
        href: "https://www.postgresql.org/docs/",
        children: [{ t: "text", text: "https://www.postgresql.org/docs/" }],
      },
      { t: "text", text: "." },
    ]);
  });
});

describe("SQL code blocks", () => {
  it("recognises SQL by language or by its first keyword", () => {
    expect(isSqlCode("sql", "anything")).toBe(true);
    expect(isSqlCode("MySQL", "x")).toBe(true);
    expect(isSqlCode("", "  WITH t AS (SELECT 1) SELECT * FROM t")).toBe(true);
    expect(isSqlCode("", "npm install")).toBe(false);
    expect(isSqlCode("python", "select = 1")).toBe(false);
  });

  it("extracts every SQL block, nested ones included", () => {
    const reply =
      "Try:\n```sql\nSELECT 1;\n```\n1. then\n   ```\n   UPDATE t SET a = 1;\n   ```\n```bash\nls\n```";
    expect(sqlBlocks(reply)).toEqual(["SELECT 1;", "UPDATE t SET a = 1;"]);
  });
});
