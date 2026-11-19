import { describe, expect, it } from "vite-plus/test";

import type { Field } from "~/api/types";

import { contextFields, contextPayload, evidenceLinks, missingRequired } from "./context";

const fields: Field[] = [
  { key: "what", label: "What happened?", kind: "long_text", required: true },
  { key: "count", label: "How many?", kind: "number", required: false },
  { key: "repeat", label: "Repeat?", kind: "boolean", required: true },
];

describe("case context", () => {
  it("asks for Details when the rule has no fields", () => {
    expect(contextFields([]).map((f) => f.key)).toEqual(["details"]);
    expect(contextFields(fields)).toBe(fields);
  });

  it("sends trimmed answers and leaves blanks out", () => {
    expect(contextPayload(fields, { what: "  spam ", count: " 2.50 ", repeat: false })).toEqual({
      what: "spam",
      count: "2.50",
      repeat: false,
    });
    expect(contextPayload(fields, { what: " ", count: "" })).toEqual({ repeat: false });
  });

  it("finds required fields without an answer", () => {
    expect(missingRequired(fields, {}).map((f) => f.key)).toEqual(["what"]);
    expect(missingRequired(fields, { what: "x" })).toEqual([]);
  });

  it("splits pasted links", () => {
    expect(evidenceLinks("a\n b  c\n")).toEqual(["a", "b", "c"]);
  });
});
