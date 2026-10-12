import { describe, expect, it } from "vitest";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import {
  addGroupName,
  groupNameError,
  idsInGroup,
  knownGroups,
  removeGroupName,
  renameGroupName,
  sanitizeGroups,
} from "./groups";

describe("connection groups", () => {
  it("reads stored names defensively", () => {
    expect(sanitizeGroups(["A", " A ", "", 3, null, "B"])).toEqual(["A", "B"]);
    expect(sanitizeGroups("A")).toEqual([]);
    expect(sanitizeGroups(undefined)).toEqual([]);
  });

  it("lists saved and used groups alphabetically, once", () => {
    expect(knownGroups(["Archive", "Local"], SEED_PROFILES)).toEqual([
      "Archive",
      "Local",
      "Production",
      "Staging",
    ]);
  });

  it("adds, renames and removes names", () => {
    expect(addGroupName(["A"], " B ")).toEqual(["A", "B"]);
    expect(addGroupName(["A"], "A")).toEqual(["A"]);
    expect(addGroupName(["A"], "  ")).toEqual(["A"]);
    expect(renameGroupName(["A", "B"], "A", "C")).toEqual(["B", "C"]);
    // A group only used by profiles (not saved yet) is saved under its new name.
    expect(renameGroupName(["B"], "A", "C")).toEqual(["B", "C"]);
    expect(removeGroupName(["A", "B"], "A")).toEqual(["B"]);
  });

  it("validates names against the existing groups, ignoring case", () => {
    expect(groupNameError(" ", [])).toBe("Enter a group name");
    expect(groupNameError("staging", ["Staging"])).toBe("“staging” already exists");
    expect(groupNameError("Staging", ["Staging"], "Staging")).toBeNull();
    expect(groupNameError("x".repeat(61), [])).toMatch(/60/);
    expect(groupNameError("Archive", ["Staging"])).toBeNull();
  });

  it("finds the profiles of a group", () => {
    expect(idsInGroup(SEED_PROFILES, "Staging")).toEqual(["p-staging-ssh", "p-rotated-ssh"]);
    expect(idsInGroup(SEED_PROFILES, "")).toEqual(["p-sample-sqlite"]);
    expect(idsInGroup(SEED_PROFILES, "Nope")).toEqual([]);
  });
});
