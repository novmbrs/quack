import { describe, expect, it } from "vite-plus/test";

import {
  actionOutcome,
  ago,
  duration,
  humanize,
  isSnowflake,
  last,
  levelName,
  levelOutcome,
  levelSummary,
  ordinal,
  plural,
} from "./format";

describe("duration", () => {
  it("uses the largest unit and the next one", () => {
    expect(duration(86400)).toBe("1 day");
    expect(duration(3 * 3600)).toBe("3 hours");
    expect(duration(5400)).toBe("1 hour 30 minutes");
    expect(duration(604800)).toBe("7 days");
    expect(duration(90061)).toBe("1 day 1 hour");
    expect(duration(45)).toBe("45 seconds");
    expect(duration(0)).toBe("0 seconds");
  });
  it('drops the one after "the last"', () => {
    expect(last(86400)).toBe("day");
    expect(last(7 * 86400)).toBe("7 days");
    expect(last(5400)).toBe("1 hour 30 minutes");
  });
});

describe("ago", () => {
  const now = Date.parse("2026-10-03T12:00:00Z");
  it("says just now for the last few seconds", () => {
    expect(ago("2026-10-03T11:59:50Z", now)).toBe("just now");
  });
  it("counts minutes and hours", () => {
    expect(ago("2026-10-03T11:55:00Z", now)).toBe("5 minutes ago");
    expect(ago("2026-10-03T09:00:00Z", now)).toBe("3 hours ago");
  });
  it("is empty for missing timestamps", () => {
    expect(ago(undefined, now)).toBe("");
  });
});

describe("levels", () => {
  it("names a level by position and start", () => {
    expect(levelName(2, 3)).toBe("Level 2 (starts at 3 cases)");
    expect(levelName(1, 1)).toBe("Level 1 (starts at 1 case)");
  });
  it("says what a level does", () => {
    expect(levelOutcome(null)).toBe("warning");
    expect(levelOutcome({ kind: "timeout", duration_seconds: 3600 })).toBe("timeout for 1 hour");
    expect(levelOutcome({ kind: "ban", delete_message_seconds: 86400 })).toBe(
      "ban and delete the last day of messages",
    );
    expect(levelOutcome({ kind: "ban", delete_message_seconds: 0 })).toBe("ban");
    expect(levelSummary({ from_case_count: 3, action: { kind: "kick" } })).toBe("at 3 cases: kick");
  });
  it("gives outcome words", () => {
    expect(actionOutcome({ state: "failed", failure_reason: "unconfirmed" }).label).toBe(
      "Couldn't confirm",
    );
    expect(actionOutcome({ state: "skipped" }).label).toBe("Left in place");
  });
});

describe("helpers", () => {
  it("formats ordinals and plurals", () => {
    expect(ordinal(1)).toBe("1st");
    expect(ordinal(12)).toBe("12th");
    expect(ordinal(22)).toBe("22nd");
    expect(plural(1, "case")).toBe("1 case");
    expect(plural(2, "case")).toBe("2 cases");
  });
  it("humanizes keys", () => {
    expect(humanize("message_delete")).toBe("Message delete");
  });
  it("recognizes snowflakes", () => {
    expect(isSnowflake("80351110224678912")).toBe(true);
    expect(isSnowflake("abc")).toBe(false);
  });
});
