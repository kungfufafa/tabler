import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Value } from "@/lib/ipc";
import {
  DYNAMODB_JSON,
  DYNAMODB_PLAIN_JSON,
  EXTENDED_JSON,
  MISSING,
  type DocumentFields,
} from "./formats";
import { JsonDocumentEditor } from "./JsonDocumentEditor";

const text = (v: string): Value => ({ t: "text", v });

const doc: DocumentFields = {
  _id: { t: "other", v: { ty: "objectId", text: "65f1a2b3c4d5e6f708192a3b" } },
  name: text("Ada"),
  age: { t: "int", v: 36 },
  city: MISSING,
};

const editor = () => screen.getByRole("textbox", { name: "Document" }) as HTMLTextAreaElement;
const type = (value: string) => fireEvent.change(editor(), { target: { value } });

describe("JsonDocumentEditor", () => {
  it("shows the document without its missing fields and saves only what changed", () => {
    const onSave = vi.fn();
    render(<JsonDocumentEditor document={doc} formats={[EXTENDED_JSON]} onSave={onSave} />);
    expect(editor().value).toContain('"$oid": "65f1a2b3c4d5e6f708192a3b"');
    expect(editor().value).not.toContain("city");
    const save = screen.getByRole("button", { name: /Save/ });
    expect(save).toBeDisabled();
    expect(screen.getByText("No changes")).toBeInTheDocument();

    type('{"_id": {"$oid": "65f1a2b3c4d5e6f708192a3b"}, "name": null, "age": 37, "city": "Oslo"}');
    expect(screen.getByText("3 changed fields")).toBeInTheDocument();
    fireEvent.click(save);
    expect(onSave).toHaveBeenCalledWith({
      name: { t: "null" },
      age: { t: "int", v: 37 },
      city: text("Oslo"),
    });
  });

  it("refuses an invalid text, a changed read-only field and a removed field", () => {
    render(
      <JsonDocumentEditor
        document={doc}
        formats={[EXTENDED_JSON]}
        readOnlyFields={["_id"]}
        onSave={() => {}}
      />,
    );
    const save = screen.getByRole("button", { name: /Save/ });
    type('{"name": ');
    expect(screen.getByRole("alert")).toHaveTextContent(/Unexpected end/);
    expect(save).toBeDisabled();
    type('{"_id": {"$oid": "65f1a2b3c4d5e6f708192a3c"}, "name": "Ada", "age": 36}');
    expect(screen.getByRole("alert")).toHaveTextContent('"_id" cannot be changed');
    type('{"_id": {"$oid": "65f1a2b3c4d5e6f708192a3b"}, "name": "Ada"}');
    expect(screen.getByRole("alert")).toHaveTextContent('Removing a field ("age")');
    expect(save).toBeDisabled();
  });

  it("unsets a removed field where the lane can", () => {
    const onSave = vi.fn();
    render(
      <JsonDocumentEditor
        document={doc}
        formats={[EXTENDED_JSON]}
        canRemoveFields
        onSave={onSave}
      />,
    );
    type('{"_id": {"$oid": "65f1a2b3c4d5e6f708192a3b"}, "name": "Ada"}');
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onSave).toHaveBeenCalledWith({ age: MISSING });
  });

  it("toggles between DynamoDB JSON and plain JSON, keeping the edit", () => {
    const onSave = vi.fn();
    const item: DocumentFields = {
      pk: text("user#1"),
      profile: { t: "json", v: '{"city":{"S":"Oslo"}}' },
      n: { t: "decimal", v: "1.50" },
    };
    render(
      <JsonDocumentEditor
        document={item}
        formats={[DYNAMODB_JSON, DYNAMODB_PLAIN_JSON]}
        onSave={onSave}
      />,
    );
    expect(JSON.parse(editor().value)).toEqual({
      pk: { S: "user#1" },
      profile: { M: { city: { S: "Oslo" } } },
      n: { N: "1.50" },
    });
    type('{"pk": {"S": "user#2"}, "profile": {"M": {"city": {"S": "Oslo"}}}, "n": {"N": "1.50"}}');
    fireEvent.click(screen.getByRole("radio", { name: "JSON" }));
    expect(JSON.parse(editor().value)).toEqual({ pk: "user#2", profile: { city: "Oslo" }, n: 1.5 });
    expect(editor().value).toContain('"n": 1.50');
    fireEvent.click(screen.getByRole("radio", { name: "DynamoDB JSON" }));
    expect(JSON.parse(editor().value).pk).toEqual({ S: "user#2" });
    fireEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(onSave).toHaveBeenCalledWith({ pk: text("user#2") });
  });

  it("shows a read-only document without its buttons, with the lane's slots", () => {
    render(
      <JsonDocumentEditor
        document={doc}
        formats={[EXTENDED_JSON]}
        toolbar={<button type="button">Copy as shell</button>}
        footer={<p>Loaded from the replica</p>}
      />,
    );
    expect(editor()).toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: /Save/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Copy as shell" })).toBeInTheDocument();
    expect(screen.getByText("Loaded from the replica")).toBeInTheDocument();
  });
});
