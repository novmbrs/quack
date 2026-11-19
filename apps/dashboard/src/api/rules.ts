import { api, unwrap, write } from "./client";
import type { Rule, RuleDefinition, RuleFile, Settings } from "./types";

// Writes for rules. Reads live in queries.ts (rulesQuery, ruleQuery,
// ruleVersionQuery); callers invalidate keys.rules(guildId) after any of
// these succeed.

const rulePath = (guildId: string, ruleId: string) => ({
  path: { guild_id: guildId, rule_id: ruleId },
});

/** createRule saves a new rule. It can be applied as soon as it exists. */
export function createRule(guildId: string, definition: RuleDefinition): Promise<Rule> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/rules", {
      params: { path: { guild_id: guildId } },
      body: definition,
      ...init,
    }),
  );
}

/**
 * updateRule saves a new version. It fails with 409 rule_changed when
 * someone else saved a version after expectedVersion.
 */
export function updateRule(
  guildId: string,
  ruleId: string,
  expectedVersion: number,
  definition: RuleDefinition,
): Promise<Rule> {
  return write((init) =>
    api.PUT("/api/guilds/{guild_id}/rules/{rule_id}", {
      params: rulePath(guildId, ruleId),
      body: { expected_version: expectedVersion, definition },
      ...init,
    }),
  );
}

/** archiveRule stops a rule from being applied. Its cases stay and keep counting. */
export function archiveRule(guildId: string, ruleId: string): Promise<Rule> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/rules/{rule_id}/archive", {
      params: rulePath(guildId, ruleId),
      ...init,
    }),
  );
}

/** restoreRule makes an archived rule available again. */
export function restoreRule(guildId: string, ruleId: string): Promise<Rule> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/rules/{rule_id}/restore", {
      params: rulePath(guildId, ruleId),
      ...init,
    }),
  );
}

/** exportRule fetches a rule as a rule file, without anything server-specific. */
export function exportRule(guildId: string, ruleId: string): Promise<RuleFile> {
  return unwrap(
    api.GET("/api/guilds/{guild_id}/rules/{rule_id}/export", { params: rulePath(guildId, ruleId) }),
  );
}

/** importRule creates a new rule from a rule file, optionally under another name. */
export function importRule(guildId: string, file: RuleFile, name?: string): Promise<Rule> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/rules/import", {
      params: { path: { guild_id: guildId } },
      body: { confirm: true, file, name: name || undefined },
      ...init,
    }),
  );
}

/** acknowledgeStarterNotice dismisses the one-time "review your starter rule" reminder. */
export function acknowledgeStarterNotice(guildId: string): Promise<Settings> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/settings/starter-notice/acknowledge", {
      params: { path: { guild_id: guildId } },
      ...init,
    }),
  );
}

/** fileName is the download name for a rule: "spam.quack-rule.json". */
export function fileName(ruleName: string): string {
  const base = ruleName
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60);
  return `${base || "rule"}.quack-rule.json`;
}

/** downloadRuleFile saves a rule file as "<name>.quack-rule.json". */
export function downloadRuleFile(file: RuleFile) {
  const blob = new Blob([`${JSON.stringify(file, null, 2)}\n`], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = fileName(file.rule.name);
  document.body.append(a);
  a.click();
  a.remove();
  // Give the browser a moment to start the download before revoking.
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}
