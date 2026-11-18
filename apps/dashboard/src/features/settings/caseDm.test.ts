import { describe, expect, it } from "vite-plus/test";

import { buildCaseDm, type CaseDmInput } from "./caseDm";

const base: CaseDmInput = {
  guildName: "Duck Pond",
  ruleName: "Spam",
  reason: "Posting the same link in every channel.",
  action: "warning",
  introduction: "",
  footer: "",
  caseNumber: 42,
  addedAt: "17 November 2026 at 14:05",
  appealable: false,
};

const text = (segments: { text: string }[]) => segments.map((s) => s.text).join("");

describe("buildCaseDm", () => {
  it("leads with the case number, server, rule, and reason", () => {
    const dm = buildCaseDm(base);
    expect(dm.heading).toBe("Case #42 in Duck Pond");
    expect(text(dm.rule)).toBe("Rule: Spam · 17 November 2026 at 14:05");
    expect(dm.quote).toBe(base.reason);
    expect(dm.paragraphs.map(text)).toEqual([
      "Action for this case: This case is a warning.",
      "This case can't be appealed.",
    ]);
    expect(dm.buttons).toEqual(["Open your case"]);
  });

  it("says what the rule calls for, not what Discord did", () => {
    expect(text(buildCaseDm({ ...base, action: "ban" }).paragraphs[0]!)).toBe(
      "Action for this case: The rule calls for banning you from the server. Open your case on the dashboard to check its outcome.",
    );
    expect(
      text(
        buildCaseDm({
          ...base,
          action: "timeout",
          timeoutEnds: { full: "18 November 2026 at 14:05", relative: "in a day" },
        }).paragraphs[0]!,
      ),
    ).toBe(
      "Action for this case: The rule calls for a timeout until 18 November 2026 at 14:05 (in a day). Open your case on the dashboard to check its outcome.",
    );
  });

  it("orders appeal line, introduction, details, and footer", () => {
    const dm = buildCaseDm({
      ...base,
      appealable: true,
      introduction: "  Hello from the mods. ",
      footer: "Questions? Open a ticket.",
      details: [{ label: "Where", value: "#general" }],
    });
    expect(dm.paragraphs.map(text)).toEqual([
      "Action for this case: This case is a warning.",
      "To ask the moderators to review this case, select Appeal case. You can appeal once.",
      "Hello from the mods.",
      "Details",
      "Where: #general",
      "Questions? Open a ticket.",
    ]);
    expect(dm.buttons).toEqual(["Appeal case", "Open your case"]);
  });

  it("falls back to this server without a name", () => {
    expect(buildCaseDm({ ...base, guildName: " " }).heading).toBe("Case #42 in this server");
  });
});
