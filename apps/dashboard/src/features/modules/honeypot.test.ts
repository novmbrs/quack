import { describe, expect, it } from "vite-plus/test";

import type { Level } from "~/api/types";

import { honeypotCount, honeypotWarning } from "./honeypot";

const tail = "This channel catches spam accounts. Quack ignores posts from staff and bots.";
const rule = (levels: Level[]) => ({ name: "Honeypot", levels });

describe("honeypotWarning", () => {
  it("uses the server's own text", () => {
    expect(honeypotWarning("Stay out!", undefined)).toBe("Stay out!");
  });

  it("words a single outcome", () => {
    expect(
      honeypotWarning(
        "  ",
        rule([
          { from_case_count: 1, action: { kind: "ban", delete_message_seconds: 0 }, send_dm: true },
        ]),
      ),
    ).toBe(
      `### Don't post here\nIf you post here, Quack can add a case under Honeypot. This rule can ban you.\n${tail}`,
    );
  });

  it("lists every outcome once, in level order", () => {
    const escalating = rule([
      { from_case_count: 5, action: { kind: "ban", delete_message_seconds: 86400 }, send_dm: true },
      { from_case_count: 1, action: null, send_dm: true },
      { from_case_count: 3, action: { kind: "timeout", duration_seconds: 3600 }, send_dm: true },
      { from_case_count: 4, action: { kind: "kick" }, send_dm: true },
      { from_case_count: 9, action: { kind: "ban", delete_message_seconds: 0 }, send_dm: true },
    ]);
    expect(honeypotWarning("", escalating)).toBe(
      `### Don't post here\nIf you post here, Quack can add a case under Honeypot. This rule can give you a warning, time you out for 1 hour, remove you from the server, or ban you. What happens depends on how many of your cases count under it.\n${tail}`,
    );
  });

  it("says a warning-only rule gives a warning", () => {
    expect(
      honeypotWarning("", rule([{ from_case_count: 1, action: null, send_dm: true }])),
    ).toContain("This rule can give you a warning.\n");
  });

  it("gives up without a rule", () => {
    expect(honeypotWarning("", undefined)).toBeNull();
  });
});

it("honeypotCount counts like the warning post", () => {
  expect(honeypotCount(0)).toBe("No honeypot cases yet.");
  expect(honeypotCount(1)).toBe("1 honeypot case so far.");
  expect(honeypotCount(1234)).toBe("1,234 honeypot cases so far.");
});
