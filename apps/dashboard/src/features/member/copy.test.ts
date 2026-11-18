import { describe, expect, it } from "vite-plus/test";

import { memberAppealLine, memberEventTitle, notAppealableLine } from "./copy";

const appeal = (state: "pending" | "accepted" | "rejected" | "closed" | "needs_information") => ({
  id: "a",
  state,
  version: 1,
  statement: "I was set up.",
  created_at: "2026-11-01T00:00:00.000Z",
  can_reply: state === "needs_information",
  events: [],
});

describe("memberAppealLine", () => {
  it("uses the appeal DM wording", () => {
    expect(memberAppealLine({ number: 12, voided_at: null }, appeal("rejected"))).toBe(
      "The moderators rejected your appeal. Case #12 still counts.",
    );
    expect(memberAppealLine({ number: 12, voided_at: "x" }, appeal("accepted"))).toBe(
      "The moderators accepted your appeal. Case #12 no longer counts.",
    );
  });
  it("tells a void close from a staff close", () => {
    expect(memberAppealLine({ number: 3, voided_at: "x" }, appeal("closed"))).toMatch(
      /closed because case #3 was voided/,
    );
    expect(memberAppealLine({ number: 3, voided_at: null }, appeal("closed"))).toBe(
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
      expect(memberEventTitle({ kind, text: "reason" }, { voided_at: null })).toMatch(
        /^The moderators /,
      );
    }
    expect(memberEventTitle({ kind: "information_provided", text: "" }, { voided_at: null })).toBe(
      "You replied to the appeal.",
    );
  });
  it("explains a close by void", () => {
    expect(memberEventTitle({ kind: "closed", text: "" }, { voided_at: "x" })).toBe(
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
