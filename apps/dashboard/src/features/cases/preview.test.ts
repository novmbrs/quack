import { describe, expect, it } from "vite-plus/test";

import { describePreview } from "./preview";

const base = {
  at: "2026-11-18T12:00:00.000Z",
  rule_version: 3,
  counted_cases: 2,
  case_count: 3,
  level_index: 2,
};

describe("describePreview", () => {
  it("says what the level does and where the case count stands", () => {
    expect(
      describePreview({
        ...base,
        counting_window_days: 30,
        level: {
          from_case_count: 3,
          send_dm: true,
          action: { kind: "timeout", duration_seconds: 3600 },
        },
      }),
    ).toEqual({
      title: "Timeout for 1 hour",
      level: "Level 2 (starts at 3 cases)",
      detail:
        "Case count 3 with this case, counting cases from the last 30 days. The member gets a DM.",
    });
  });

  it("calls a level without an action a warning", () => {
    const d = describePreview({
      ...base,
      case_count: 1,
      level_index: 1,
      counting_window_days: null,
      level: { from_case_count: 1, send_dm: false, action: null },
    });
    expect(d.title).toBe("Warning");
    expect(d.detail).toBe("Case count 1 with this case. This level doesn't send a DM.");
  });
});
