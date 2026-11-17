import { describe, expect, it } from "vitest";
import { outputText } from "./logLines";
import { basename, dirname } from "./options";

describe("backup paths", () => {
  it("splits on both separators, so Windows paths show their file name", () => {
    expect(basename("/Users/me/Backups/shop.sql.gz")).toBe("shop.sql.gz");
    expect(basename("C:\\Users\\me\\Backups\\shop.sql.gz")).toBe("shop.sql.gz");
    expect(basename("C:/Users/me/dump\\shop.dump")).toBe("shop.dump");
    expect(dirname("/Users/me/Backups/shop.sql")).toBe("/Users/me/Backups");
    expect(dirname("C:\\Users\\me\\shop.sql")).toBe("C:\\Users\\me");
    expect(dirname("C:\\shop.sql")).toBe("C:\\");
    expect(dirname("shop.sql")).toBeNull();
  });
});

describe("outputText", () => {
  it("is the command line, then every output line", () => {
    expect(
      outputText({
        command: "pg_dump --dbname=shop",
        log: [{ text: "pg_dump: dumping" }, { text: "ERROR: denied" }],
      }),
    ).toBe("$ pg_dump --dbname=shop\npg_dump: dumping\nERROR: denied");
    expect(outputText({ command: null, log: [{ text: "only" }] })).toBe("only");
  });
});
