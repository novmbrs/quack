import { describe, expect, it } from "vite-plus/test";

import type { Settings } from "~/api/types";

import {
  changes,
  formFromSettings,
  inviteProblem,
  maxStaffRoles,
  type SettingsForm,
  settingsPatch,
  staffRolesProblem,
  textProblem,
} from "./form";

const saved: SettingsForm = {
  logChannel: "",
  appealChannel: "111111111111111111",
  evidenceChannel: "",
  appealInvite: "https://discord.gg/quack",
  dmIntro: "Hi from the mods.",
  dmFooter: "",
  moderatorRoles: ["444444444444444444", "555555555555555555"],
  rulesManagerRoles: [],
};

const off = { enabled: false, problems: [] };

describe("formFromSettings", () => {
  it("fills missing fields with empty values", () => {
    const settings: Settings = {
      version: 3,
      moderator_role_ids: [],
      rules_manager_role_ids: [],
      modules: { tickets: off, activity_log: off, honeypot: off },
    };
    expect(formFromSettings(settings)).toEqual({
      logChannel: "",
      appealChannel: "",
      evidenceChannel: "",
      appealInvite: "",
      dmIntro: "",
      dmFooter: "",
      moderatorRoles: [],
      rulesManagerRoles: [],
    });
  });
});

describe("settingsPatch", () => {
  it("is empty when nothing changed", () => {
    expect(settingsPatch(saved, { ...saved }, 3)).toEqual({});
  });

  it("ignores whitespace-only edits", () => {
    expect(settingsPatch(saved, { ...saved, dmIntro: "  Hi from the mods.\n" }, 3)).toEqual({});
  });

  it("sends only the changed fields, without expected_version", () => {
    const patch = settingsPatch(
      saved,
      {
        ...saved,
        logChannel: "222222222222222222",
        evidenceChannel: "333333333333333333",
        dmFooter: " Be kind. ",
      },
      3,
    );
    expect(patch).toEqual({
      log_channel_id: "222222222222222222",
      evidence_channel_id: "333333333333333333",
      dm_footer: "Be kind.",
    });
    expect(changes(patch)).toBe(true);
  });

  it("clears a field with null", () => {
    expect(settingsPatch(saved, { ...saved, appealChannel: "", appealInvite: " " }, 3)).toEqual({
      appeal_channel_id: null,
      appeal_invite_url: null,
    });
  });
});

describe("settingsPatch staff roles", () => {
  it("ignores a reordered list", () => {
    expect(
      settingsPatch(
        saved,
        { ...saved, moderatorRoles: ["555555555555555555", "444444444444444444"] },
        3,
      ),
    ).toEqual({});
  });

  it("sends a changed list whole, with expected_version", () => {
    expect(
      settingsPatch(
        saved,
        {
          ...saved,
          moderatorRoles: ["444444444444444444"],
          rulesManagerRoles: ["666666666666666666"],
        },
        7,
      ),
    ).toEqual({
      moderator_role_ids: ["444444444444444444"],
      rules_manager_role_ids: ["666666666666666666"],
      expected_version: 7,
    });
  });

  it("clears a list with an empty array", () => {
    expect(settingsPatch(saved, { ...saved, moderatorRoles: [] }, 2)).toEqual({
      moderator_role_ids: [],
      expected_version: 2,
    });
  });

  it("doesn't count expected_version as a change", () => {
    expect(changes({ expected_version: 1 })).toBe(false);
  });
});

describe("staffRolesProblem", () => {
  const known = new Set(["1", "2"]);

  it("accepts current roles, and anything while roles load", () => {
    expect(staffRolesProblem([], [], known)).toBeNull();
    expect(staffRolesProblem(["1", "2"], [], known)).toBeNull();
    expect(staffRolesProblem(["3"], [], undefined)).toBeNull();
  });

  it("lets saved roles deleted in Discord stay", () => {
    expect(staffRolesProblem(["1", "3"], ["3"], known)).toBeNull();
  });

  it("flags newly added deleted roles and too many roles", () => {
    expect(staffRolesProblem(["1", "3"], [], known)).not.toBeNull();
    const many = Array.from({ length: maxStaffRoles + 1 }, (_, i) => String(i + 1));
    expect(staffRolesProblem(many, [], undefined)).not.toBeNull();
  });
});

describe("inviteProblem", () => {
  it("accepts empty and Discord invites in every form the API takes", () => {
    for (const ok of [
      "",
      "https://discord.gg/abc-DEF_1",
      "discord.gg/abc",
      "http://discord.gg/abc",
      "https://discord.com/invite/abc",
      " https://www.discord.com/invite/abc ",
      "discordapp.com/invite/abc",
    ]) {
      expect(inviteProblem(ok), ok).toBeNull();
    }
  });

  it("rejects anything else", () => {
    for (const bad of [
      "https://discord.gg/",
      "https://discord.gg/a",
      "https://discord.gg/abc?x=1",
      "https://discord.com/channels/1/2",
      "https://example.com/abc",
      "https://discord.gg/abc/def",
    ]) {
      expect(inviteProblem(bad), bad).not.toBeNull();
    }
  });
});

describe("textProblem", () => {
  it("allows up to the limit after trimming, in characters", () => {
    expect(textProblem(`  ${"a".repeat(1000)}  `, 1000)).toBeNull();
    expect(textProblem("🦆".repeat(1000), 1000)).toBeNull();
    expect(textProblem("a".repeat(1001), 1000)).not.toBeNull();
  });
});
