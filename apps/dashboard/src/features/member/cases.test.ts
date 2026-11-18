import { describe, expect, it } from "vite-plus/test";

import type { MemberCaseSummary } from "~/api/types";

import { groupByServer } from "./cases";

const kase = (id: string, guildId: string): MemberCaseSummary => ({
  id,
  number: 1,
  guild: { guild_id: guildId, name: `Server ${guildId}`, icon_url: null },
  rule_name: "Spam",
  reason: "Spam.",
  created_at: "2026-11-01T00:00:00.000Z",
  voided_at: null,
  action_text: "This case is a warning.",
  can_appeal: true,
  appeal_state: null,
});

describe("groupByServer", () => {
  it("keeps the newest server first and the order inside each", () => {
    const groups = groupByServer([kase("a", "1"), kase("b", "2"), kase("c", "1")]);
    expect(groups.map((g) => g.guild.guild_id)).toEqual(["1", "2"]);
    expect(groups[0]!.cases.map((c) => c.id)).toEqual(["a", "c"]);
  });
});
