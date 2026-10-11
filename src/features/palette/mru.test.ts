import { describe, expect, it } from "vitest";
import { MRU_MAX, MRU_STORAGE_KEY, pushMru, readMru, recordMru } from "./mru";

function memoryStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, v),
    data,
  };
}

describe("palette MRU", () => {
  it("moves a key to the front without duplicates and caps the list", () => {
    expect(pushMru(["a", "b", "c"], "b")).toEqual(["b", "a", "c"]);
    expect(pushMru(["a", "b"], "c", 2)).toEqual(["c", "a"]);
  });

  it("persists under tabler.palette.mru", () => {
    const storage = memoryStorage();
    recordMru("action:ui.toggle-sidebar", storage);
    recordMru("table:p1:public:users", storage);
    recordMru("action:ui.toggle-sidebar", storage);
    expect(JSON.parse(storage.data.get(MRU_STORAGE_KEY)!)).toEqual([
      "action:ui.toggle-sidebar",
      "table:p1:public:users",
    ]);
    expect(readMru(storage)).toEqual(["action:ui.toggle-sidebar", "table:p1:public:users"]);
  });

  it("tolerates corrupt data and failing storage", () => {
    expect(readMru(memoryStorage({ [MRU_STORAGE_KEY]: "{nope" }))).toEqual([]);
    expect(readMru(memoryStorage({ [MRU_STORAGE_KEY]: '{"a":1}' }))).toEqual([]);
    expect(readMru(memoryStorage({ [MRU_STORAGE_KEY]: '["a",2,"b"]' }))).toEqual(["a", "b"]);
    const throwing = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    expect(readMru(throwing)).toEqual([]);
    expect(recordMru("x", throwing)).toEqual(["x"]);
    expect(readMru(null)).toEqual([]);
  });

  it("caps what it reads", () => {
    const many = Array.from({ length: MRU_MAX + 10 }, (_, i) => `k${i}`);
    expect(readMru(memoryStorage({ [MRU_STORAGE_KEY]: JSON.stringify(many) }))).toHaveLength(
      MRU_MAX,
    );
  });
});
