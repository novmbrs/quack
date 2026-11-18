import { describe, expect, it } from "vite-plus/test";

import { memberAppealLine, memberEventTitle, notAppealableLine } from "./copy";

const appeal = (
  state: "pending" | "accepted" | "rejected" | "closed" | "needs_information",
  closedByVoid = false,
) => ({
  id: "a",
  state,
  version: 1,
  statement: "I was set up.",
  created_at: "2026-11-01T00:00:00.000Z",
  can_reply: state === "needs_information",
  events:
    state === "closed"
      ? [
          {
            kind: "closed" as const,
            text: "",
            at: "2026-11-02T00:00:00.000Z",
            by_you: false,
            closed_by_void: closedByVoid,
          },
        ]
      : [],
});

describe("memberAppealLine", () => {
  it("uses the appeal DM wording", () => {
    expect(memberAppealLine({ number: 12 }, appeal("rejected"))).toBe(
      "The moderators rejected your appeal. Case #12 still counts.",
    );
    expect(memberAppealLine({ number: 12 }, appeal("accepted"))).toBe(
      "The moderators accepted your appeal. Case #12 no longer counts.",
    );
  });
  it("tells a void close from a staff close by what the API says", () => {
    expect(memberAppealLine({ number: 3 }, appeal("closed", true))).toMatch(
      /closed because case #3 was voided/,
    );
    expect(memberAppealLine({ number: 3 }, appeal("closed"))).toBe(
      "The moderators closed your appeal. Case #3 still counts.",
    );
  });
});

describe("memberEventTitle", () => {
  it("never names a moderator", () => {
    for (const kind of [
      "information_requested",
      "accepted",
      "rejected",
      "closed",
      "reopened",
    ] as const) {
      expect(memberEventTitle({ kind, closed_by_void: false })).toMatch(/^The moderators /);
    }
    expect(memberEventTitle({ kind: "information_provided", closed_by_void: false })).toBe(
      "You replied to the appeal.",
    );
  });
  it("explains a close by void", () => {
    expect(memberEventTitle({ kind: "closed", closed_by_void: true })).toBe(
      "The appeal was closed because the case was voided.",
    );
  });
});

describe("notAppealableLine", () => {
  it("says why", () => {
    expect(notAppealableLine({ number: 4, voided_at: "x" })).toBe(
      "Case #4 was voided and no longer counts.",
    );
    expect(notAppealableLine({ number: 4, voided_at: null })).toBe("This case can't be appealed.");
  });
});
