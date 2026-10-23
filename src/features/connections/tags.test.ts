import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { sortProfiles } from "./list";
import {
  compareTags,
  isBuiltinTag,
  loadCustomTags,
  resetCustomTags,
  sanitizeTags,
  tagColorOf,
  tagLabel,
  tagsError,
  tagShortLabel,
  TAGS_SETTING,
  useCustomTagsStore,
} from "./tags";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});

afterEach(() => {
  resetCustomTags();
  resetMockSettings();
});

describe("tagShortLabel", () => {
  it("abbreviates the environments that matter and leaves local unmarked", () => {
    expect(tagShortLabel("production")).toBe("PROD");
    expect(tagShortLabel("staging")).toBe("STG");
    expect(tagShortLabel("development")).toBe("DEV");
    expect(tagShortLabel("testing")).toBe("TEST");
    expect(tagShortLabel("local")).toBeNull();
    expect(tagShortLabel(null)).toBeNull();
    expect(tagShortLabel("customer-a")).toBe("CUST");
  });
});

describe("custom connection tags (b494)", () => {
  it("keeps valid, unique tags that are not built-in names", () => {
    expect(
      sanitizeTags([
        { name: " QA ", color: "teal" },
        { name: "qa", color: "red" },
        { name: "Production", color: "red" },
        { name: "", color: "red" },
        { name: "Demo", color: "#12ab34" },
        { name: "Odd", color: "not-a-colour" },
        "junk",
        null,
      ]),
    ).toEqual([
      { name: "QA", color: "teal" },
      { name: "Demo", color: "#12ab34" },
      { name: "Odd", color: "gray" },
    ]);
    expect(sanitizeTags("nope")).toEqual([]);
  });

  it("explains what the Edit Tags dialog cannot save", () => {
    expect(tagsError([{ name: "QA", color: "teal" }])).toBeNull();
    expect(tagsError([{ name: " ", color: "teal" }])).toMatch(/needs a name/);
    expect(tagsError([{ name: "staging", color: "teal" }])).toMatch(/built-in/);
    expect(
      tagsError([
        { name: "QA", color: "teal" },
        { name: "qa", color: "red" },
      ]),
    ).toMatch(/two tags named/);
  });

  it("labels, colours and sorts built-in, custom and unknown tags", async () => {
    await ipc.settings.set(TAGS_SETTING, [{ name: "QA", color: "#12ab34" }]);
    await loadCustomTags();
    expect(useCustomTagsStore.getState()).toMatchObject({ loaded: true });
    expect(isBuiltinTag("local")).toBe(true);
    expect(tagLabel("production")).toBe("Production");
    expect(tagLabel("QA")).toBe("QA");
    expect(tagColorOf("production")).toBe("red");
    expect(tagColorOf("qa")).toBe("#12ab34");
    expect(tagColorOf("Gone")).toBe("gray");
    const order = ["Gone", null, "QA", "production", "local"].sort(compareTags);
    expect(order).toEqual(["local", "production", "QA", "Gone", null]);

    const base = SEED_PROFILES[0]!;
    const p = (name: string, tag: string | null) => ({
      ...base,
      id: name,
      name,
      options: { ...base.options, tag },
    });
    expect(
      sortProfiles([p("a", null), p("b", "QA"), p("c", "testing")], "tag").map((x) => x.name),
    ).toEqual(["c", "b", "a"]);
  });
});
