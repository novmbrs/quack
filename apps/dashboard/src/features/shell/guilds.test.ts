import { describe, expect, it } from "vite-plus/test";

import type { Capabilities, GuildSummary } from "~/api/types";

import { guildRole, installableGuilds, showsOverview, staffGuilds } from "./guilds";

const none: Capabilities = {
  moderate: false,
  manage_rules: false,
  manage_settings: false,
  manage_staff_roles: false,
};

const guild = (over: Partial<GuildSummary>, caps: Partial<Capabilities> = {}): GuildSummary => ({
  guild_id: "1",
  name: "Pond",
  icon_url: null,
  installed: true,
  can_install: false,
  mfa_required: false,
  mfa_blocked: false,
  ...over,
  capabilities: { ...none, ...caps },
});

describe("staffGuilds", () => {
  it("keeps every kind of staff where Quack is installed", () => {
    const list = [
      guild({ guild_id: "mod" }, { moderate: true }),
      guild({ guild_id: "admin" }, { manage_settings: true, manage_rules: true }),
      guild({ guild_id: "rules" }, { manage_rules: true }),
      guild({ guild_id: "mfa", mfa_required: true, mfa_blocked: true }),
    ];
    expect(staffGuilds(list).map((g) => g.guild_id)).toEqual(["mod", "admin", "rules", "mfa"]);
  });

  it("drops servers without Quack and servers where the account isn't staff", () => {
    const list = [
      guild(
        { guild_id: "missing", installed: false, can_install: true },
        { manage_settings: true },
      ),
      guild({ guild_id: "member" }),
    ];
    expect(staffGuilds(list)).toEqual([]);
    expect(installableGuilds(list).map((g) => g.guild_id)).toEqual(["missing"]);
  });
});

describe("guildRole", () => {
  it("names the staff role", () => {
    expect(guildRole(guild({}, { manage_settings: true, moderate: true }))).toBe("Administrator");
    expect(guildRole(guild({}, { moderate: true }))).toBe("Moderator");
    expect(guildRole(guild({}, { manage_rules: true }))).toBe("Rules manager");
    expect(guildRole(guild({}, { moderate: true, manage_rules: true }))).toBe(
      "Moderator and rules manager",
    );
  });

  it("says when 2FA is blocking access", () => {
    expect(guildRole(guild({ mfa_blocked: true }))).toBe("Needs 2FA");
  });
});

describe("showsOverview", () => {
  const me = (caps: Partial<Capabilities>) => ({ capabilities: { ...none, ...caps } });

  it("is true for moderators and administrators", () => {
    expect(showsOverview(me({ moderate: true }))).toBe(true);
    expect(showsOverview(me({ manage_settings: true }))).toBe(true);
  });

  it("is false for rules managers", () => {
    expect(showsOverview(me({ manage_rules: true }))).toBe(false);
  });
});
