import { describe, expect, it } from "vite-plus/test";

import { fillDays, total } from "./days";

describe("fillDays", () => {
  it("adds a zero for every day without cases", () => {
    const from = new Date("2026-10-01T00:00:00Z");
    expect(fillDays([{ key: "2026-10-02", count: 4 }], from, 3)).toEqual([
      { day: "2026-10-01", count: 0 },
      { day: "2026-10-02", count: 4 },
      { day: "2026-10-03", count: 0 },
    ]);
  });

  it("accepts day keys with a time", () => {
    const from = new Date("2026-10-01T00:00:00Z");
    expect(fillDays([{ key: "2026-10-01T00:00:00Z", count: 2 }], from, 1)).toEqual([
      { day: "2026-10-01", count: 2 },
    ]);
  });
});

describe("total", () => {
  it("adds the counts", () => {
    expect(total([{ count: 2 }, { count: 3 }])).toBe(5);
    expect(total(undefined)).toBe(0);
  });
});
