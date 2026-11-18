import { describe, expect, it } from "vite-plus/test";

import { ApiError } from "~/api/client";
import type { RuleDefinition } from "~/api/types";

import {
  apiIssues,
  emptyDraft,
  fingerprint,
  fromDefinition,
  ladder,
  newField,
  newLevel,
  outcome,
  parseRuleFile,
  range,
  slugify,
  summarize,
  toDefinition,
  uniqueName,
  validate,
  windowLabel,
  type Draft,
} from "./draft";

// The starter rule from design §3.7, with a field added.
const starter: RuleDefinition = {
  name: "General rule violation",
  description: "A starter rule for general violations. Review it and change it for this server.",
  reason: "General rule violation",
  appealable: true,
  counting_window_days: null,
  fields: [{ key: "link", label: "Message link", kind: "message_link", required: true }],
  levels: [
    { from_case_count: 5, action: { kind: "ban", delete_message_seconds: 86400 }, send_dm: true },
    { from_case_count: 1, action: null, send_dm: true },
    { from_case_count: 3, action: { kind: "timeout", duration_seconds: 86400 }, send_dm: false },
  ],
};

describe("fromDefinition and toDefinition", () => {
  it("round-trips a definition with levels in order", () => {
    const def = toDefinition(fromDefinition(starter));
    expect(def.levels.map((l) => l.from_case_count)).toEqual([1, 3, 5]);
    expect(def.levels[1]).toEqual({
      from_case_count: 3,
      action: { kind: "timeout", duration_seconds: 86400 },
      send_dm: false,
    });
    expect(def.levels[2]?.action).toEqual({ kind: "ban", delete_message_seconds: 86400 });
    expect(def.fields).toEqual(starter.fields);
    expect(def.counting_window_days).toBeNull();
  });

  it("sends the counting window only when windowed", () => {
    const draft = fromDefinition({ ...starter, counting_window_days: 30 });
    expect(draft.windowed).toBe(true);
    expect(toDefinition(draft).counting_window_days).toBe(30);
    expect(toDefinition({ ...draft, windowed: false }).counting_window_days).toBeNull();
  });

  it("trims text and ignores editor-only state in the fingerprint", () => {
    const draft = fromDefinition(starter);
    expect(fingerprint({ ...draft, name: ` ${draft.name} ` })).toBe(fingerprint(draft));
    expect(fingerprint({ ...draft, reason: "Other" })).not.toBe(fingerprint(draft));
  });
});

describe("validate", () => {
  const valid = (): Draft => ({ ...fromDefinition(starter) });

  it("accepts the starter rule", () => {
    expect(validate(valid())).toEqual({});
  });

  it("needs a name and a reason", () => {
    const issues = validate({ ...emptyDraft() });
    expect(issues.name).toBeTruthy();
    expect(issues.reason).toBeTruthy();
  });

  it("checks lengths in characters", () => {
    expect(validate({ ...valid(), name: "🦆".repeat(100) }).name).toBeUndefined();
    expect(validate({ ...valid(), name: "a".repeat(101) }).name).toBeTruthy();
    expect(validate({ ...valid(), description: "a".repeat(501) }).description).toBeTruthy();
  });

  it("makes the first level start at 1 and later ones strictly higher", () => {
    const d = valid();
    const [first, second, third] = d.levels;
    expect(validate({ ...d, levels: [{ ...first!, from: 2 }, second!, third!] })).toHaveProperty(
      `levels.${first!.id}.from`,
    );
    const dup = { ...third!, from: 3 };
    expect(validate({ ...d, levels: [first!, second!, dup] })).toHaveProperty(
      `levels.${dup.id}.from`,
    );
  });

  it("limits timeouts to 28 days and ban deletion to 7 days", () => {
    const d = valid();
    const t = { ...newLevel(2), action: "timeout" as const, timeoutSeconds: 29 * 86400 };
    const b = { ...newLevel(9), action: "ban" as const, deleteSeconds: 8 * 86400 };
    const issues = validate({ ...d, levels: [...d.levels, t, b] });
    expect(issues[`levels.${t.id}.timeout`]).toBeTruthy();
    expect(issues[`levels.${b.id}.delete`]).toBeTruthy();
  });

  it("checks field keys and the counting window", () => {
    const a = { ...newField(), label: "Where", key: "where" };
    const b = { ...newField(), label: "Where again", key: "where" };
    const c = { ...newField(), label: "Bad", key: "A" };
    const issues = validate({ ...valid(), fields: [a, b, c] });
    expect(issues[`fields.${a.id}.key`]).toBeUndefined();
    expect(issues[`fields.${b.id}.key`]).toBeTruthy();
    expect(issues[`fields.${c.id}.key`]).toBeTruthy();
    expect(validate({ ...valid(), windowed: true, windowDays: 0 }).window).toBeTruthy();
    expect(validate({ ...valid(), windowed: true, windowDays: 30 }).window).toBeUndefined();
  });
});

describe("apiIssues", () => {
  it("maps API field paths onto the editor", () => {
    const draft = fromDefinition(starter);
    const error = new ApiError(
      400,
      "validation_failed",
      "Check the marked fields and save again.",
      {
        fields: [
          { field: "name", code: "name_length", message: "Too long." },
          { field: "levels[1].from_case_count", code: "level_order", message: "Out of order." },
          {
            field: "levels[2].action.delete_message_seconds",
            code: "ban_delete_window",
            message: "7 days.",
          },
          { field: "fields[0].key", code: "field_key", message: "Bad key." },
          { field: "counting_window_days", code: "counting_window", message: "Window." },
          { field: "something_else", code: "x", message: "Other." },
        ],
      },
    );
    const issues = apiIssues(error, draft);
    const [, second, third] = draft.levels;
    expect(issues.name).toBe("Too long.");
    expect(issues[`levels.${second!.id}.from`]).toBe("Out of order.");
    expect(issues[`levels.${third!.id}.delete`]).toBe("7 days.");
    expect(issues[`fields.${draft.fields[0]!.id}.key`]).toBe("Bad key.");
    expect(issues.window).toBe("Window.");
    expect(issues.form).toBe("Other.");
  });
});

describe("ladder", () => {
  it("covers case counts from each level to the next", () => {
    const steps = ladder(fromDefinition(starter).levels);
    expect(steps.map((s) => range(s))).toEqual([
      "Case count 1–2",
      "Case count 3–4",
      "Case count 5+",
    ]);
    expect(steps.map(outcome)).toEqual([
      "Warning",
      "Timeout for 1 day",
      "Ban and delete the last day of messages",
    ]);
    expect(steps.map((s) => s.index)).toEqual([1, 2, 3]);
    expect(summarize(steps)).toBe("1–2 · Warning → 3–4 · Timeout for 1 day → 5+ · Ban");
  });

  it("marks a level that shares a case count as never reached", () => {
    const a = newLevel(1);
    const b = newLevel(3);
    const c = { ...newLevel(3), action: "kick" as const };
    const steps = ladder([a, b, c]);
    expect(steps[1]?.unreachable).toBe(true);
    expect(range(steps[1]!)).toBe("Never reached");
    expect(range(steps[0]!)).toBe("Case count 1–2");
  });
});

describe("parseRuleFile", () => {
  it("reads a rule file", () => {
    const text = JSON.stringify({
      format: "quack-rule",
      format_version: 1,
      exported_at: "2026-11-17T18:00:00Z",
      rule: starter,
    });
    expect(parseRuleFile(text).rule.name).toBe("General rule violation");
  });

  it("converts a file from the earlier dashboard", () => {
    const legacy = {
      schema_version: 1,
      slug: "spam",
      name: "Spam",
      description: "",
      official_reason: "Spamming.",
      case_decay_days: 30,
      appealable: true,
      context_fields: [
        { key: "where", label: "Where", type: "short_text", position: 2, required: false },
        { key: "link", label: "Link", type: "discord_message_link", position: 1, required: true },
      ],
      levels: [
        {
          name: "Ban",
          is_default: false,
          trigger_case_count: 5,
          notify_user: true,
          actions: [{ action_type: "ban_user", delete_message_seconds: 3600, max_retries: 0 }],
        },
        {
          name: "Default",
          is_default: true,
          trigger_case_count: 0,
          notify_user: false,
          actions: [],
        },
      ],
    };
    const file = parseRuleFile(
      JSON.stringify({ policy: legacy }),
      new Date("2026-11-18T00:00:00Z"),
    );
    expect(file.format).toBe("quack-rule");
    expect(file.rule.reason).toBe("Spamming.");
    expect(file.rule.counting_window_days).toBe(30);
    expect(file.rule.fields.map((f) => [f.key, f.kind])).toEqual([
      ["link", "message_link"],
      ["where", "short_text"],
    ]);
    expect(file.rule.levels).toEqual([
      { from_case_count: 1, action: null, send_dm: false },
      { from_case_count: 5, action: { kind: "ban", delete_message_seconds: 3600 }, send_dm: true },
    ]);
  });

  it("explains what's wrong with other text", () => {
    expect(() => parseRuleFile("not json")).toThrow(/rule file/);
    expect(() => parseRuleFile('{"hello":1}')).toThrow(/rule file/);
    expect(() => parseRuleFile('{"format":"quack-rule","format_version":2,"rule":{}}')).toThrow(
      /newer version/,
    );
  });
});

describe("helpers", () => {
  it("slugifies labels into keys", () => {
    expect(slugify("Message link")).toBe("message_link");
    expect(slugify("  Où?  ")).toBe("ou");
  });

  it("finds a free rule name, ignoring case", () => {
    expect(uniqueName("Spam", ["Harassment"])).toBe("Spam");
    expect(uniqueName("Spam", ["spam", "Spam (2)"])).toBe("Spam (3)");
  });

  it("labels the counting window", () => {
    expect(windowLabel(null)).toBe("Counts every case");
    expect(windowLabel(1)).toBe("Counts the last day");
    expect(windowLabel(90)).toBe("Counts the last 90 days");
  });
});
