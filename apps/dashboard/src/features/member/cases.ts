import type { MemberCaseSummary } from "~/api/types";

/** ServerCases is one server's cases on the "Your cases" page. */
export type ServerCases = {
  guild: MemberCaseSummary["guild"];
  cases: MemberCaseSummary[];
};

/**
 * groupByServer groups a member's cases by server, keeping the API's order
 * (newest first) inside each server and ordering servers by their newest case.
 */
export function groupByServer(cases: MemberCaseSummary[]): ServerCases[] {
  const groups = new Map<string, ServerCases>();
  for (const c of cases) {
    const group = groups.get(c.guild.guild_id);
    if (group) group.cases.push(c);
    else groups.set(c.guild.guild_id, { guild: c.guild, cases: [c] });
  }
  return [...groups.values()];
}
