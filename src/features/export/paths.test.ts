import { describe, expect, it } from "vitest";
import { basename, dirname } from "./options";

describe("export paths", () => {
  it("splits on both separators, so Windows paths show their file name", () => {
    expect(basename("/Users/me/Downloads/users.csv")).toBe("users.csv");
    expect(basename("C:\\Users\\me\\Downloads\\users.csv")).toBe("users.csv");
    expect(dirname("/Users/me/Downloads/users.csv")).toBe("/Users/me/Downloads");
    expect(dirname("C:\\Users\\me\\Downloads\\users.csv")).toBe("C:\\Users\\me\\Downloads");
    expect(dirname("D:\\users.csv")).toBe("D:\\");
    expect(dirname("users.csv")).toBeNull();
  });
});
