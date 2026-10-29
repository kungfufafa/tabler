import { describe, expect, it } from "vitest";
import type { Value } from "@/lib/ipc";
import {
  documentChanges,
  DYNAMODB_JSON,
  DYNAMODB_PLAIN_JSON,
  EXTENDED_JSON,
  MISSING,
  parseJson,
  PLAIN_JSON,
  stringifyJson,
  type DocumentFields,
  type DocumentFormat,
} from "./formats";

const text = (v: string): Value => ({ t: "text", v });
const int = (v: number): Value => ({ t: "int", v });

const fieldsOf = (format: DocumentFormat, json: string, original: DocumentFields = {}) => {
  const read = format.read(json, original);
  if ("error" in read) throw new Error(read.error);
  return read.fields;
};

describe("parseJson", () => {
  it("keeps numbers' digits and refuses duplicate fields", () => {
    const node = parseJson('{"big": 12345678901234567890, "d": 0.1000, "e": -1e-7}');
    expect(stringifyJson(node)).toBe('{"big":12345678901234567890,"d":0.1000,"e":-1e-7}');
    expect(() => parseJson('{"a": 1, "a": 2}')).toThrow(/Duplicate field "a"/);
    expect(() => parseJson('{"a": }')).toThrow(/line 1, column 7/);
    expect(() => parseJson("[1, 2] x")).toThrow(/after the document/);
  });

  it("indents by two spaces", () => {
    expect(stringifyJson(parseJson('{"a":[1,{"b":null}],"c":{}}'), 2)).toBe(
      '{\n  "a": [\n    1,\n    {\n      "b": null\n    }\n  ],\n  "c": {}\n}',
    );
  });
});

describe("the document formats", () => {
  const doc: DocumentFields = {
    _id: { t: "other", v: { ty: "objectId", text: "65f1a2b3c4d5e6f708192a3b" } },
    name: text("Ada"),
    status: MISSING,
    note: { t: "null" },
    seen: { t: "timestampTz", v: "2024-05-01T10:00:00.000Z" },
    views: { t: "bigInt", v: "9007199254740993" },
    price: { t: "decimal", v: "19.90" },
    tags: { t: "json", v: '["a",{"b":1}]' },
    re: {
      t: "other",
      v: { ty: "regex", text: '{"$regularExpression":{"pattern":"^a","options":"i"}}' },
    },
  };

  it("leave missing fields out and read every untouched field back as it was", () => {
    for (const format of [PLAIN_JSON, EXTENDED_JSON]) {
      const written = format.write(doc);
      expect(written).not.toContain("status");
      expect(written).toContain('"note": null');
      const { status: _missing, ...present } = doc;
      expect(fieldsOf(format, written, doc), format.id).toEqual(present);
    }
  });

  it("write MongoDB's types as relaxed Extended JSON and read them back", () => {
    const written = EXTENDED_JSON.write(doc);
    expect(written).toContain('"_id": {\n    "$oid": "65f1a2b3c4d5e6f708192a3b"\n  }');
    expect(written).toContain('"$numberLong": "9007199254740993"');
    const fresh = fieldsOf(
      EXTENDED_JSON,
      '{"_id": {"$oid": "65f1a2b3c4d5e6f708192a3c"}, "at": {"$date": "2025-01-01T00:00:00Z"}, ' +
        '"n": {"$numberLong": "12"}, "d": {"$numberDecimal": "1.10"}, "u": {"$uuid": "0e1f"}, ' +
        '"big": 18446744073709551616, "f": 1.5, "sub": {"x": 1}, ' +
        '"ts": {"$timestamp": {"t": 1, "i": 2}}}',
    );
    expect(fresh).toEqual({
      _id: { t: "other", v: { ty: "objectId", text: "65f1a2b3c4d5e6f708192a3c" } },
      at: { t: "timestampTz", v: "2025-01-01T00:00:00Z" },
      n: int(12),
      d: { t: "decimal", v: "1.10" },
      u: { t: "uuid", v: "0e1f" },
      big: { t: "bigInt", v: "18446744073709551616" },
      f: { t: "float", v: 1.5 },
      sub: { t: "json", v: '{"x":1}' },
      ts: { t: "other", v: { ty: "timestamp", text: '{"$timestamp":{"t":1,"i":2}}' } },
    });
  });

  it("keep a field's loaded type in plain JSON", () => {
    const fields = fieldsOf(
      PLAIN_JSON,
      '{"seen": "2025-02-02T00:00:00.000Z", "price": 20, "views": 1, "name": 5}',
      doc,
    );
    expect(fields.seen).toEqual({ t: "timestampTz", v: "2025-02-02T00:00:00.000Z" });
    expect(fields.price).toEqual({ t: "decimal", v: "20" });
    expect(fields.views).toEqual({ t: "bigInt", v: "1" });
    expect(fields.name).toEqual(int(5));
  });

  describe("DynamoDB", () => {
    const item: DocumentFields = {
      pk: text("user#1"),
      age: int(37),
      score: { t: "decimal", v: "0.10" },
      ok: { t: "bool", v: true },
      none: { t: "null" },
      gone: MISSING,
      profile: { t: "json", v: '{"city":{"S":"Oslo"},"zip":{"N":"0150"}}' },
      list: { t: "json", v: '[{"N":"1"},{"S":"x"}]' },
      colors: { t: "array", v: [text("red"), text("blue")] },
      nums: { t: "array", v: [int(1), { t: "decimal", v: "2.5" }] },
    };

    it("writes typed JSON and reads every untouched attribute back", () => {
      const written = DYNAMODB_JSON.write(item);
      expect(JSON.parse(written)).toEqual({
        pk: { S: "user#1" },
        age: { N: "37" },
        score: { N: "0.10" },
        ok: { BOOL: true },
        none: { NULL: true },
        profile: { M: { city: { S: "Oslo" }, zip: { N: "0150" } } },
        list: { L: [{ N: "1" }, { S: "x" }] },
        colors: { SS: ["red", "blue"] },
        nums: { NS: ["1", "2.5"] },
      });
      const { gone: _missing, ...present } = item;
      expect(fieldsOf(DYNAMODB_JSON, written, item)).toEqual(present);
    });

    it("toggles to plain JSON with maps unmarshalled and numbers exact", () => {
      const plain = DYNAMODB_PLAIN_JSON.write(item);
      expect(JSON.parse(plain)).toEqual({
        pk: "user#1",
        age: 37,
        score: 0.1,
        ok: true,
        none: null,
        profile: { city: "Oslo", zip: 150 },
        list: [1, "x"],
        colors: ["red", "blue"],
        nums: [1, 2.5],
      });
      expect(plain).toContain('"score": 0.10');
      // DynamoDB's own number text, where JSON refuses it, without its leading zeros.
      expect(plain).toContain('"zip": 150');
      const back = fieldsOf(DYNAMODB_PLAIN_JSON, plain, item);
      // A map edited in plain JSON is plain JSON content, which the backend reads too.
      expect(back.profile).toEqual(item.profile);
      expect(fieldsOf(DYNAMODB_PLAIN_JSON, '{"price": 1.5}', {}).price).toEqual({
        t: "decimal",
        v: "1.5",
      });
    });

    it("refuses an attribute that is not typed", () => {
      expect(DYNAMODB_JSON.read('{"a": "x"}', {})).toEqual({
        error: expect.stringContaining('Field "a" is not DynamoDB JSON'),
      });
    });
  });
});

describe("documentChanges", () => {
  const original: DocumentFields = {
    _id: text("a"),
    name: text("Ada"),
    age: int(3),
    city: MISSING,
  };

  it("stages only what changed, a missing field kept apart from NULL", () => {
    expect(documentChanges(original, { _id: text("a"), name: text("Ada"), age: int(3) })).toEqual({
      changes: {},
    });
    expect(
      documentChanges(original, {
        _id: text("a"),
        name: { t: "null" },
        age: int(3),
        city: text("Oslo"),
      }),
    ).toEqual({ changes: { name: { t: "null" }, city: text("Oslo") } });
  });

  it("unsets a removed field only where the lane can", () => {
    const edited = { _id: text("a"), name: text("Ada") };
    expect(documentChanges(original, edited)).toEqual({
      error: expect.stringContaining('Removing a field ("age")'),
    });
    expect(documentChanges(original, edited, { canRemoveFields: true })).toEqual({
      changes: { age: MISSING },
    });
  });

  it("refuses a change to a read-only field", () => {
    expect(
      documentChanges(original, { ...original, _id: text("b") }, { readOnlyFields: ["_id"] }),
    ).toEqual({
      error: '"_id" cannot be changed',
    });
  });

  it("compares JSON without its whitespace", () => {
    const a = { j: { t: "json", v: '{"a": [1, 2]}' } } as const;
    expect(documentChanges(a, { j: { t: "json", v: '{"a":[1,2]}' } })).toEqual({ changes: {} });
  });
});
