import { describe, expect, it } from "vite-plus/test";

import { selectLevel } from "./escalation";

const levels = [
  { id: "warning", from_case_count: 1 },
  { id: "timeout", from_case_count: 3 },
  { id: "ban", from_case_count: 5 },
];

describe("selectLevel", () => {
  it("uses the first level until the next one starts", () => {
    expect(selectLevel(levels, 1)?.id).toBe("warning");
    expect(selectLevel(levels, 2)?.id).toBe("warning");
  });
  it("picks the highest level the case count reached", () => {
    expect(selectLevel(levels, 3)?.id).toBe("timeout");
    expect(selectLevel(levels, 4)?.id).toBe("timeout");
    expect(selectLevel(levels, 5)?.id).toBe("ban");
    expect(selectLevel(levels, 40)?.id).toBe("ban");
  });
  it("does not depend on level order", () => {
    expect(selectLevel([...levels].reverse(), 4)?.id).toBe("timeout");
  });
  it("falls back to the first level below every level", () => {
    expect(selectLevel([{ id: "a", from_case_count: 2 }], 1)?.id).toBe("a");
  });
});
