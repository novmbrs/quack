import { describe, expect, it } from "vite-plus/test";

import { ApiError } from "~/api/client";

import { disabledReasonText, enableProblem } from "./enable";

describe("enableProblem", () => {
  it("lists every problem of a module that isn't ready", () => {
    const error = new ApiError(409, "module_not_ready", "Tickets are still off.", {
      problems: [
        { code: "ticket_entry_missing", message: "Choose where members open tickets." },
        { code: "ticket_queue_missing", message: "Choose a staff channel for new tickets." },
      ],
    });
    expect(enableProblem(error)).toEqual({
      message: "Tickets are still off.",
      problems: ["Choose where members open tickets.", "Choose a staff channel for new tickets."],
    });
  });

  it("keeps the API's message for other refusals", () => {
    const error = new ApiError(
      403,
      "missing_capability",
      "To change Quack's settings, you need Manage Server in Discord.",
    );
    expect(enableProblem(error)).toEqual({
      message: "To change Quack's settings, you need Manage Server in Discord.",
      problems: [],
    });
  });

  it("has a sentence for anything else", () => {
    expect(enableProblem(new Error("boom")).message).toMatch(/^Quack couldn't/);
  });
});

describe("disabledReasonText", () => {
  it("explains why Quack turned a module off", () => {
    expect(
      disabledReasonText("honeypot", { enabled: false, disabled_reason: "channel_deleted" }),
    ).toMatch(/^The honeypot was turned off because a channel it uses was deleted\./);
    expect(
      disabledReasonText("tickets", { enabled: false, disabled_reason: "quack_left" }),
    ).toMatch(/^Tickets were turned off because Quack left the server\./);
  });

  it("is quiet when staff turned it off or it's on", () => {
    expect(
      disabledReasonText("activity_log", { enabled: false, disabled_reason: null }),
    ).toBeNull();
    expect(
      disabledReasonText("activity_log", { enabled: true, disabled_reason: "channel_deleted" }),
    ).toBeNull();
  });
});
