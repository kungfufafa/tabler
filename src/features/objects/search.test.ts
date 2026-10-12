import { describe, expect, it } from "vitest";
import { compileSearch, isSearching } from "./search";

const NAMES = [
  "users",
  "user_stats",
  "posts",
  "post_tags",
  "audit_log",
  "Order Items",
  "CamelCase",
];
const find = (q: string) => NAMES.filter(compileSearch(q));

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
});
