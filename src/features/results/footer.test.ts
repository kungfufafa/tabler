import { describe, expect, it } from "vitest";
import {
  appendedHeader,
  autoLimitLabel,
  durationLabel,
  pinnedLabel,
  resultTabText,
} from "./footer";

describe("results footer labels", () => {
  it("say when the row limit cut the result", () => {
    const n = (1000).toLocaleString();
    expect(autoLimitLabel(1000)).toBe(`${n} rows (auto limited ${n} rows)`);
    expect(autoLimitLabel(1)).toBe("1 row (auto limited 1 row)");
  });

  it("show the execution time once the statement finished", () => {
    expect(durationLabel(6)).toBe("6 ms");
    expect(durationLabel(1234)).toBe("1.23 s");
    expect(durationLabel(null)).toBe("");
  });

  it("name result tabs, pinned results and stacked results", () => {
    expect(resultTabText("Result 2", 1, 23)).toBe("Result 2 · 1 row · 23 ms");
    expect(resultTabText("Result 1", 0, null)).toBe("Result 1 · 0 rows");
    expect(pinnedLabel("Result 2")).toBe("Result 2 (pinned)");
    expect(appendedHeader(3, 2, 5)).toBe("Result 3 · 2 rows · 5 ms");
  });
});
