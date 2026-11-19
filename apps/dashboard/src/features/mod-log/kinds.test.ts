import { describe, expect, it } from "vite-plus/test";

import type { ModLogKind } from "~/api/types";

import { filtersOf, iconFor, kindGroups, parseKind, parseResult } from "./kinds";

describe("kindGroups", () => {
  it("names every kind once", () => {
    const kinds = kindGroups.flatMap((g) => g.kinds.map((k) => k.value));
    expect(new Set(kinds).size).toBe(kinds.length);
    expect(kinds).toContain("case_created");
    expect(kinds).toContain("legacy");
    expect(kinds).toHaveLength(37);
  });
  it("never uses banned words in labels", () => {
    for (const g of kindGroups)
      for (const k of g.kinds) expect(k.label).not.toMatch(/audit|notification|template/i);
  });
});

describe("search params", () => {
  it("drops unknown kinds and results", () => {
    expect(parseKind("case_voided")).toBe("case_voided");
    expect(parseKind("template_created")).toBeUndefined();
    expect(parseResult("refused")).toBe("refused");
    expect(parseResult("success")).toBeUndefined();
  });
  it("turns days into a half-open range", () => {
    expect(filtersOf({ from: "2026-11-01", to: "2026-11-30", kind: "case_created" })).toMatchObject(
      {
        kind: ["case_created"],
        created_after: "2026-11-01T00:00:00.000Z",
        created_before: "2026-12-01T00:00:00.000Z",
      },
    );
    expect(filtersOf({}).kind).toBeUndefined();
  });
});

describe("iconFor", () => {
  it("shows failures first", () => {
    expect(iconFor("action_failed", "failed")).toBe("error");
    expect(iconFor("case_create_refused", "refused")).toBe("warn");
    expect(iconFor("case_voided" as ModLogKind, "ok")).toBe("case_void");
  });
});
