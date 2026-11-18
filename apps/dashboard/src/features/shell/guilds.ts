import type { Capabilities, GuildMe, GuildSummary } from "~/api/types";

const anyCapability = (c: Capabilities) =>
  c.moderate || c.manage_rules || c.manage_settings || c.manage_staff_roles;

/**
 * staffGuilds keeps the servers where Quack is installed and the account is
 * staff: a moderator, rules manager, or administrator, or someone who would
 * be once Quack confirms their 2FA.
 */
export function staffGuilds(guilds: readonly GuildSummary[]): GuildSummary[] {
  return guilds.filter((g) => g.installed && (anyCapability(g.capabilities) || g.mfa_blocked));
}

/** installableGuilds are servers without Quack where the account can add it. */
export function installableGuilds(guilds: readonly GuildSummary[]): GuildSummary[] {
  return guilds.filter((g) => !g.installed && g.can_install);
}

/** guildRole names the account's staff role in a server in plain words. */
export function guildRole(guild: Pick<GuildSummary, "capabilities" | "mfa_blocked">): string {
  const c = guild.capabilities;
  if (guild.mfa_blocked) return "Needs 2FA";
  if (c.manage_settings || c.manage_staff_roles) return "Administrator";
  if (c.moderate && c.manage_rules) return "Moderator and rules manager";
  if (c.moderate) return "Moderator";
  if (c.manage_rules) return "Rules manager";
  return "Staff";
}

/**
 * showsOverview reports whether a server's overview has anything for the
 * account. Rules managers only work on rules, so they start there instead.
 */
export function showsOverview(me: Pick<GuildMe, "capabilities">): boolean {
  return me.capabilities.moderate || me.capabilities.manage_settings;
}
