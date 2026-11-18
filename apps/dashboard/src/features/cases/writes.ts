import { api, write } from "~/api/client";
import type { Action, Case, ContextInput } from "~/api/types";

// Writes for cases and their actions (design §14.2 routes 21–25, 29–31).
// Each call is one thing the moderator asked for, with its own
// Idempotency-Key; write() reuses it on automatic retries.

const actionPath = (guildId: string, actionId: string) => ({
  path: { guild_id: guildId, action_id: actionId },
});
const casePath = (guildId: string, caseRef: string) => ({
  path: { guild_id: guildId, case_ref: caseRef },
});

/** retryAction starts a new round of attempts for a failed action. */
export function retryAction(guildId: string, actionId: string): Promise<Action> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/actions/{action_id}/retry", {
      params: actionPath(guildId, actionId),
      ...init,
    }),
  );
}

/** dismissAction takes a failed action off the failed actions list. */
export function dismissAction(guildId: string, actionId: string): Promise<Action> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/actions/{action_id}/dismiss", {
      params: actionPath(guildId, actionId),
      ...init,
    }),
  );
}

/** undoAction queues removing a timeout or unbanning; it returns the undo action. */
export function undoAction(guildId: string, actionId: string): Promise<Action> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/actions/{action_id}/undo", {
      params: actionPath(guildId, actionId),
      ...init,
    }),
  );
}

/** voidCase keeps the case in the history and stops it from counting. */
export function voidCase(guildId: string, caseRef: string, reason: string): Promise<Case> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/cases/{case_ref}/void", {
      params: casePath(guildId, caseRef),
      body: { reason },
      ...init,
    }),
  );
}

/** editContext replaces the case context; a newer revision is 409 context_changed. */
export function editContext(
  guildId: string,
  caseRef: string,
  context: Record<string, ContextInput>,
  expectedRevision: number,
): Promise<Case> {
  return write((init) =>
    api.PUT("/api/guilds/{guild_id}/cases/{case_ref}/context", {
      params: casePath(guildId, caseRef),
      body: { context, expected_context_revision: expectedRevision },
      ...init,
    }),
  );
}

/** addEvidence saves up to 10 more message links as evidence. */
export function addEvidence(guildId: string, caseRef: string, links: string[]): Promise<Case> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/cases/{case_ref}/evidence", {
      params: casePath(guildId, caseRef),
      body: { links },
      ...init,
    }),
  );
}

/** AddCase is the body of a new case. */
export type AddCase = {
  member_id: string;
  rule_id: string;
  rule_version?: number;
  context: Record<string, ContextInput>;
  evidence_links: string[];
  replaces_case_number?: number;
};

/** addCase applies a rule to a member. */
export function addCase(guildId: string, body: AddCase): Promise<Case> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/cases", {
      params: { path: { guild_id: guildId } },
      body,
      ...init,
    }),
  );
}
