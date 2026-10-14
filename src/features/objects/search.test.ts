import { describe, expect, it } from "vitest";
import { compileSearch, isSearching, type MatchStrategy } from "./search";

const NAMES = [
  "users",
  "user_stats",
  "posts",
  "post_tags",
  "audit_log",
  "Order Items",
  "CamelCase",
];
const find = (q: string, strategy?: MatchStrategy) => NAMES.filter(compileSearch(q, strategy));

describe("compileSearch", () => {
  it("matches everything when empty", () => {
    expect(find("")).toEqual(NAMES);
    expect(find("   ")).toEqual(NAMES);
    expect(find(" | , ")).toEqual(NAMES);
    expect(isSearching("  ")).toBe(false);
    expect(isSearching("u")).toBe(true);
  });

  it("matches fuzzily, ignoring case", () => {
    expect(find("usr")).toEqual(["users", "user_stats"]);
    expect(find("USER")).toEqual(["users", "user_stats"]);
    expect(find("pt")).toEqual(["posts", "post_tags"]);
    expect(find("cc")).toEqual(["CamelCase"]);
    expect(find("zzz")).toEqual([]);
  });

  it("anchors with ^ and $", () => {
    expect(find("^post")).toEqual(["posts", "post_tags"]);
    expect(find("s$")).toEqual(["users", "user_stats", "posts", "post_tags", "Order Items"]);
    expect(find("^users$")).toEqual(["users"]);
    expect(find("^user$")).toEqual([]);
    expect(find("_log$")).toEqual(["audit_log"]);
  });

  it("separates alternatives with | or ,", () => {
    expect(find("^users$|^posts$")).toEqual(["users", "posts"]);
    expect(find("audit, ^camel")).toEqual(["audit_log", "CamelCase"]);
    expect(find("^users$ | ")).toEqual(["users"]);
  });

  it("requires every whitespace-separated term of an alternative", () => {
    expect(find("user stat")).toEqual(["user_stats"]);
    expect(find("^post tags")).toEqual(["post_tags"]);
  });

  it("matches double-quoted text exactly (as a substring)", () => {
    expect(find('"ser"')).toEqual(["users", "user_stats"]);
    expect(find('"usr"')).toEqual([]);
    expect(find('"order items"')).toEqual(["Order Items"]);
    expect(find('"a|b"')).toEqual([]);
    expect(find('"post_')).toEqual(["post_tags"]);
  });

  it("matches plain terms as substrings or prefixes under those strategies", () => {
    expect(find("usr", "contains")).toEqual([]);
    expect(find("ser", "contains")).toEqual(["users", "user_stats"]);
    expect(find("tag", "contains")).toEqual(["post_tags"]);
    expect(find("ser", "prefix")).toEqual([]);
    expect(find("POST", "prefix")).toEqual(["posts", "post_tags"]);
    // Anchors, quotes and alternatives keep their meaning.
    expect(find("_log$", "prefix")).toEqual(["audit_log"]);
    expect(find("post | audit", "prefix")).toEqual(["posts", "post_tags", "audit_log"]);
    expect(find('"r i"', "prefix")).toEqual(["Order Items"]);
    expect(isSearching("  ", "contains")).toBe(false);
  });

  it("tests the whole query as one regular expression under Regex", () => {
    expect(find("^post", "regex")).toEqual(["posts", "post_tags"]);
    expect(find("^(users|posts)$", "regex")).toEqual(["users", "posts"]);
    expect(find("_\\w+s$", "regex")).toEqual(["user_stats", "post_tags"]);
    expect(find("CAMEL", "regex")).toEqual(["CamelCase"]);
    // Not a usable pattern (yet): matched as plain text instead of hiding everything.
    expect(find("user_(", "regex")).toEqual([]);
    expect(["a(b", "ab"].filter(compileSearch("a(", "regex"))).toEqual(["a(b"]);
    // A runaway pattern is not run.
    expect(["aaaa"].filter(compileSearch("(a+)+$", "regex"))).toEqual([]);
    expect(isSearching(" ", "regex")).toBe(false);
    expect(isSearching("x", "regex")).toBe(true);
  });
});
