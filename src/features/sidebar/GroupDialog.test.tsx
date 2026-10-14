import { fireEvent, render, screen, within } from "@testing-library/react";
import { act } from "react";
import { beforeEach, describe, expect, it } from "vitest";
import { GroupDialogHost } from "./GroupDialog";
import {
  createGroup,
  groupsOf,
  openGroupDialog,
  reloadGroups,
  useGroupDialogStore,
} from "./groups";

const SCOPE = "p1|db";

describe("GroupDialog", () => {
  beforeEach(() => {
    localStorage.clear();
    reloadGroups();
    useGroupDialogStore.setState({ request: null });
  });

  it("creates a group with filter rules (AND), checking each rule", () => {
    render(<GroupDialogHost />);
    act(() => openGroupDialog({ scope: SCOPE }));
    const dialog = screen.getByRole("dialog", { name: "New Group" });
    const create = within(dialog).getByRole("button", { name: "Create" });
    expect(create).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Log views" } });
    // Without rules it is a manual group.
    expect(within(dialog).queryByRole("group", { name: "Filter rules" })).toBeNull();
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Use filter rules" }));
    const rules = within(dialog).getByRole("group", { name: "Filter rules" });
    fireEvent.change(within(rules).getByLabelText("Rule 1 operator"), {
      target: { value: "regex" },
    });
    fireEvent.change(within(rules).getByLabelText("Rule 1 value"), { target: { value: "^(log" } });
    expect(within(dialog).getByRole("alert")).toHaveTextContent("Invalid regex");
    expect(create).toBeDisabled();
    // A pattern that could hang the sidebar is refused too.
    fireEvent.change(within(rules).getByLabelText("Rule 1 value"), {
      target: { value: "^(\\w+)+_log$" },
    });
    expect(within(dialog).getByRole("alert")).toHaveTextContent("can take forever to match");
    expect(create).toBeDisabled();
    fireEvent.change(within(rules).getByLabelText("Rule 1 value"), {
      target: { value: "^log_" },
    });
    fireEvent.click(within(rules).getByRole("button", { name: "Add a rule after rule 1" }));
    fireEvent.change(within(rules).getByLabelText("Rule 2 field"), { target: { value: "kind" } });
    fireEvent.change(within(rules).getByLabelText("Rule 2 kind"), { target: { value: "view" } });
    fireEvent.click(create);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(groupsOf(SCOPE)).toEqual([
      expect.objectContaining({
        name: "Log views",
        rules: [
          { field: "name", op: "regex", value: "^log_" },
          { field: "kind", value: "view" },
        ],
        members: [],
      }),
    ]);
  });

  it("edits a group, and New Group… from a menu keeps the items it was opened for", () => {
    const g = createGroup(SCOPE, "Old", [{ field: "schema", value: "audit" }]);
    render(<GroupDialogHost />);
    act(() => openGroupDialog({ scope: SCOPE, group: g }));
    let dialog = screen.getByRole("dialog", { name: "Edit Group" });
    expect(within(dialog).getByLabelText("Rule 1 value")).toHaveValue("audit");
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Audit" } });
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Use filter rules" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(groupsOf(SCOPE)[0]).toMatchObject({ id: g.id, name: "Audit", rules: null });

    act(() => openGroupDialog({ scope: SCOPE, members: ["t:public.users"] }));
    dialog = screen.getByRole("dialog", { name: "New Group" });
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Mine" } });
    fireEvent.submit(within(dialog).getByLabelText("Name"));
    expect(groupsOf(SCOPE)[1]).toMatchObject({
      name: "Mine",
      rules: null,
      members: ["t:public.users"],
    });
  });
});
