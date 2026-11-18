import { describe, expect, it } from "vite-plus/test";

import {
  dismissDone,
  dismissLabel,
  mayHaveApplied,
  retryDone,
  retryLabel,
  undoConfirm,
  undoKind,
  undoQueued,
} from "./controls";

describe("action control words", () => {
  it("labels retry and dismiss like the bot", () => {
    expect(retryLabel("ban")).toBe("Retry ban");
    expect(retryLabel("remove_timeout")).toBe("Retry removing timeout");
    expect(dismissLabel("timeout")).toBe("Dismiss failed timeout");
    expect(dismissLabel("remove_timeout")).toBe("Dismiss failed timeout removal");
  });

  it("offers the undo that matches the action", () => {
    expect(undoKind({ kind: "timeout" })).toBe("remove_timeout");
    expect(undoKind({ kind: "ban" })).toBe("unban");
    expect(undoKind({ kind: "kick" })).toBeNull();
  });

  it("says what happens next", () => {
    expect(retryDone("ban", 12)).toBe(
      "Quack will try again to ban the member for case #12. Open the case to see the outcome.",
    );
    expect(retryDone("kick", 3, true)).toBe(
      "Quack will try again to remove the member from the server for case #3.",
    );
    expect(dismissDone("ban", 12)).toBe(
      "The ban failure for case #12 was dismissed. The case keeps its history.",
    );
    expect(dismissDone("unban", 12)).toBe(
      "The failed attempt to unban the member for case #12 was dismissed. The case keeps its history.",
    );
    expect(undoConfirm("unban", "Sam", 12)).toBe("Unban Sam for case #12?");
    expect(undoQueued("remove_timeout", "Sam", 12)).toBe(
      "Quack will check whether it can remove Sam's timeout. Open case #12 to see the outcome.",
    );
  });

  it("asks before retrying an action that may have applied", () => {
    expect(mayHaveApplied({ state: "failed", failure_reason: "unconfirmed" })).toBe(true);
    expect(mayHaveApplied({ state: "failed", failure_reason: "discord_refused" })).toBe(false);
  });
});
