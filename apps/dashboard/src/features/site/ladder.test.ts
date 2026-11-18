import { describe, expect, it } from "vite-plus/test";

import { selectLevel } from "~/lib/escalation";

import { ordinal, starterSteps, stepLabel } from "./ladder";

describe("starterSteps", () => {
  it("escalates like the starter rule: timeout at 3, ban at 5", () => {
    const labels = [1, 2, 3, 4, 5, 9].map((n) => stepLabel(selectLevel(starterSteps, n)!));
    const ban = "Ban and delete the last day of messages";
    expect(labels).toEqual([
      "Warning",
      "Warning",
      "Timeout for 1 day",
      "Timeout for 1 day",
      ban,
      ban,
    ]);
  });
});

describe("ordinal", () => {
  it("handles the irregular endings", () => {
    expect([1, 2, 3, 4, 11, 12, 13, 21, 22, 101].map(ordinal)).toEqual([
      "1st",
      "2nd",
      "3rd",
      "4th",
      "11th",
      "12th",
      "13th",
      "21st",
      "22nd",
      "101st",
    ]);
  });
});
