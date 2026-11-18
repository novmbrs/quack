import type { Settings, SettingsChange } from "~/api/types";

/** Limits from design §13.1.1. */
export const limits = { dmIntro: 1000, dmFooter: 500, staffRoles: 25 } as const;

/** maxStaffRoles is the most roles a staff role list can hold. */
export const maxStaffRoles = limits.staffRoles;

/** SettingsForm is the settings page's working copy of the editable fields. */
export type SettingsForm = {
  logChannel: string;
  appealChannel: string;
  evidenceChannel: string;
  appealInvite: string;
  dmIntro: string;
  dmFooter: string;
  /** Moderator role IDs, in the order they were picked. */
  moderatorRoles: string[];
  /** Rules manager role IDs, in the order they were picked. */
  rulesManagerRoles: string[];
};

/** formFromSettings starts a form from saved settings. */
export function formFromSettings(s: Settings): SettingsForm {
  return {
    logChannel: s.log_channel_id ?? "",
    appealChannel: s.appeal_channel_id ?? "",
    evidenceChannel: s.evidence_channel_id ?? "",
    appealInvite: s.appeal_invite_url ?? "",
    dmIntro: s.dm_intro ?? "",
    dmFooter: s.dm_footer ?? "",
    moderatorRoles: s.moderator_role_ids,
    rulesManagerRoles: s.rules_manager_role_ids,
  };
}

/**
 * settingsPatch is the PATCH body for what changed between saved and draft
 * (design §13.1.2). Unchanged fields are left out and cleared ones are sent
 * as null. Text is compared trimmed, since the API trims it anyway. A staff
 * role list is sent whole when its set of roles changed, and then
 * expected_version is sent too, so a stale page can't put back roles an
 * administrator just removed.
 */
export function settingsPatch(
  saved: SettingsForm,
  draft: SettingsForm,
  version: number,
): SettingsChange {
  const patch: SettingsChange = {};
  const text = (a: string, b: string) => a.trim() !== b.trim();
  const value = (v: string) => v.trim() || null;
  if (text(saved.logChannel, draft.logChannel)) patch.log_channel_id = value(draft.logChannel);
  if (text(saved.appealChannel, draft.appealChannel))
    patch.appeal_channel_id = value(draft.appealChannel);
  if (text(saved.evidenceChannel, draft.evidenceChannel))
    patch.evidence_channel_id = value(draft.evidenceChannel);
  if (text(saved.appealInvite, draft.appealInvite))
    patch.appeal_invite_url = value(draft.appealInvite);
  if (text(saved.dmIntro, draft.dmIntro)) patch.dm_intro = value(draft.dmIntro);
  if (text(saved.dmFooter, draft.dmFooter)) patch.dm_footer = value(draft.dmFooter);
  if (!sameRoles(saved.moderatorRoles, draft.moderatorRoles))
    patch.moderator_role_ids = [...draft.moderatorRoles];
  if (!sameRoles(saved.rulesManagerRoles, draft.rulesManagerRoles))
    patch.rules_manager_role_ids = [...draft.rulesManagerRoles];
  if (patch.moderator_role_ids || patch.rules_manager_role_ids) patch.expected_version = version;
  return patch;
}

/** changes reports whether a patch changes anything (expected_version alone doesn't). */
export function changes(patch: SettingsChange): boolean {
  return Object.keys(patch).some((k) => k !== "expected_version");
}

function sameRoles(a: readonly string[], b: readonly string[]): boolean {
  const left = new Set(a);
  const right = new Set(b);
  return left.size === right.size && [...left].every((id) => right.has(id));
}

/**
 * staffRolesProblem checks a staff role list the way the API does when it
 * changes: at most 25 roles, and roles not already saved must still exist
 * in the server. Saved roles deleted in Discord don't block saving. known
 * is the server's current role IDs, or undefined while they load. It
 * returns what's wrong, or null.
 */
export function staffRolesProblem(
  roleIds: readonly string[],
  saved: readonly string[],
  known: ReadonlySet<string> | undefined,
): string | null {
  if (roleIds.length > maxStaffRoles) return `Pick at most ${maxStaffRoles} roles.`;
  if (known && roleIds.some((id) => !saved.includes(id) && !known.has(id)))
    return "A role you added was deleted in Discord. Remove it to save.";
  return null;
}

/**
 * inviteProblem checks an appeal invite the way the API does: empty, or
 * discord.gg/<code>, discord.com/invite/<code>, or discordapp.com/invite/<code>,
 * with or without https:// and www., the code 2–64 of A–Z, a–z, 0–9, _ and
 * -. It returns what's wrong, or null.
 */
export function inviteProblem(raw: string): string | null {
  const value = raw.trim();
  if (value === "") return null;
  const match =
    /^(?:https?:\/\/)?(?:www\.)?(?:discord\.gg\/|discord(?:app)?\.com\/invite\/)([A-Za-z0-9_-]{2,64})\/?$/i.exec(
      value,
    );
  return match ? null : "Use a Discord invite link, like https://discord.gg/yourserver.";
}

/** textProblem reports a DM introduction or footer longer than the API allows. */
export function textProblem(value: string, max: number): string | null {
  return Array.from(value.trim()).length > max
    ? `Keep it to ${max.toLocaleString()} characters.`
    : null;
}

/** settingsFields maps API field paths to the form's fields, for field errors. */
export const settingsFields: Record<string, keyof SettingsForm> = {
  log_channel_id: "logChannel",
  appeal_channel_id: "appealChannel",
  evidence_channel_id: "evidenceChannel",
  appeal_invite_url: "appealInvite",
  dm_intro: "dmIntro",
  dm_footer: "dmFooter",
  moderator_role_ids: "moderatorRoles",
  rules_manager_role_ids: "rulesManagerRoles",
};
