import type { Health, ModuleName, Permission } from "~/api/types";

/** HealthCheck is one line of the server health checklist. */
export type HealthCheck = {
  key: string;
  label: string;
  /** ok passes, problem needs fixing, and unset is fine but off. */
  state: "ok" | "problem" | "unset";
  detail: string;
};

/** permissionNames are Discord's own names for each permission (design §12.0). */
export const permissionNames: Record<Permission, string> = {
  view_channel: "View Channel",
  send_messages: "Send Messages",
  send_messages_in_threads: "Send Messages in Threads",
  embed_links: "Embed Links",
  attach_files: "Attach Files",
  read_message_history: "Read Message History",
  moderate_members: "Timeout Members",
  kick_members: "Kick Members",
  ban_members: "Ban Members",
  manage_messages: "Manage Messages",
  manage_channels: "Manage Channels",
  manage_threads: "Manage Threads",
  create_public_threads: "Create Public Threads",
  create_private_threads: "Create Private Threads",
  view_audit_log: "View Audit Log",
  administrator: "Administrator",
  manage_guild: "Manage Server",
};

/** permissionList joins permission names: "A", "A and B", "A, B, and C". */
export function permissionList(permissions: readonly Permission[]): string {
  const names = permissions.map((p) => permissionNames[p] ?? p);
  if (names.length <= 2) return names.join(" and ");
  return `${names.slice(0, -1).join(", ")}, and ${names.at(-1)}`;
}

/** settingNames names the channel settings health reports on. */
export const settingNames: Record<string, string> = {
  log_channel_id: "Log channel",
  appeal_channel_id: "Appeal channel",
  evidence_channel_id: "Evidence channel",
  "tickets.queue_channel_id": "Ticket queue channel",
  "tickets.entry_channel_id": "Ticket entry channel",
  "activity_log.routes": "Server activity log channel",
  "honeypot.channel_id": "Honeypot channel",
};

const moduleNames: Record<ModuleName, string> = {
  tickets: "Tickets",
  activity_log: "Server activity log",
  honeypot: "Honeypot",
};

/**
 * healthChecklist turns GET …/health into a checklist: Quack's place in the
 * server, the permissions its role is missing, each configured staff channel,
 * and each module that is on. Problem messages come from the API as is.
 */
export function healthChecklist(health: Health): HealthCheck[] {
  const checks: HealthCheck[] = [];
  checks.push({
    key: "quack",
    label: "Quack is in the server",
    state: health.quack_in_server ? "ok" : "problem",
    detail: health.quack_in_server
      ? "Quack is a member of this server."
      : "Quack isn't in this server. Add Quack again to moderate here.",
  });
  if (!health.quack_in_server) return checks;

  const missing = health.missing_permissions;
  checks.push({
    key: "permissions",
    label: "Permissions",
    state: missing.length ? "problem" : "ok",
    detail: missing.length
      ? `Quack's role is missing ${permissionList(missing)}. In Server Settings → Roles, give Quack's role ${missing.length === 1 ? "this permission" : "these permissions"}.`
      : "Quack's role has every permission Quack asks for.",
  });

  for (const channel of health.channels) {
    checks.push({
      key: `channel:${channel.setting}:${channel.channel_id}`,
      label: settingNames[channel.setting] ?? channel.setting,
      state: channel.problems.length ? "problem" : "ok",
      detail: channel.problems.length
        ? channel.problems.map((p) => p.message).join(" ")
        : "Only staff can see it, and Quack can post there.",
    });
  }

  for (const name of Object.keys(moduleNames) as ModuleName[]) {
    const module = health.modules[name];
    if (!module.enabled && module.problems.length === 0) continue;
    checks.push({
      key: `module:${name}`,
      label: moduleNames[name],
      state: module.problems.length ? "problem" : module.enabled ? "ok" : "unset",
      detail: module.problems.length
        ? module.problems.map((p) => p.message).join(" ")
        : "On and working.",
    });
  }
  return checks;
}

/** healthy reports whether every check passed. */
export function healthy(checks: readonly HealthCheck[]): boolean {
  return checks.every((c) => c.state !== "problem");
}
