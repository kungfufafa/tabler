import { describe, expect, it } from "vitest";
import { fuzzyMatch, normalizeQuery } from "./fuzzy";

function rank(query: string, texts: string[]): string[] {
  return texts
    .map((text) => ({ text, m: fuzzyMatch(query, text) }))
    .filter((x) => x.m !== null)
    .sort((a, b) => b.m!.score - a.m!.score)
    .map((x) => x.text);
}

describe("fuzzyMatch", () => {
  it("matches everything with an empty query", () => {
    expect(fuzzyMatch("", "users")).toEqual({ score: 0, ranges: [] });
    expect(fuzzyMatch("   ", "users")).toEqual({ score: 0, ranges: [] });
  });

  it("returns null when the query is not a subsequence", () => {
    expect(fuzzyMatch("xyz", "users")).toBeNull();
    expect(fuzzyMatch("sru", "users")).toBeNull();
    expect(fuzzyMatch("userss", "users")).toBeNull();
  });

  it("is case-insensitive and ignores whitespace in the query", () => {
    expect(fuzzyMatch("USR", "users")).not.toBeNull();
    expect(fuzzyMatch("usr", "USERS")).not.toBeNull();
    expect(fuzzyMatch("user id", "user_id")?.ranges).toEqual([
      [0, 4],
      [5, 7],
    ]);
    expect(normalizeQuery(" Theme Dark ")).toBe("themedark");
  });

  it("returns merged highlight ranges", () => {
    expect(fuzzyMatch("usr", "users")?.ranges).toEqual([
      [0, 2],
      [3, 4],
    ]);
    expect(fuzzyMatch("ord", "orders")?.ranges).toEqual([[0, 3]]);
  });

  it("picks the best alignment rather than the first occurrence", () => {
    // Greedy matching would take the "o" of "customer"; the word "order" is the better hit.
    expect(fuzzyMatch("ord", "customer_order")?.ranges).toEqual([[9, 12]]);
    // camelCase boundaries count as word starts.
    expect(fuzzyMatch("uid", "userId")?.ranges).toEqual([
      [0, 1],
      [4, 6],
    ]);
  });

  it("ranks exact prefix, then word starts, then scattered matches", () => {
    expect(
      rank("ord", ["product_reviews_old", "customer_order_items", "order_items", "orders"]),
    ).toEqual(["orders", "order_items", "customer_order_items", "product_reviews_old"]);
  });

  it("prefers word-start and consecutive characters", () => {
    expect(rank("ui", ["build", "user_items"])).toEqual(["user_items", "build"]);
    expect(rank("abc", ["xaxbxcx", "xabcx"])).toEqual(["xabcx", "xaxbxcx"]);
  });

  it("ranks an exact match above longer prefixes and shorter texts above longer ones", () => {
    expect(rank("users", ["users_archive", "users", "app_users"])).toEqual([
      "users",
      "users_archive",
      "app_users",
    ]);
    expect(rank("use", ["user_sessions", "users"])).toEqual(["users", "user_sessions"]);
  });
});
