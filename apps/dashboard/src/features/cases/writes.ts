import { api, upload, type UploadProgress, write } from "~/api/client";
import type {
  Action,
  Case,
  CaseVoided,
  ContextInput,
  EvidenceAdded,
  EvidenceEdited,
} from "~/api/types";

// Writes for cases and their actions (design §14.2 routes 21–25, 29–31,
// 60–63).
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

/**
 * voidCase keeps the case in the history and stops it from counting; the
 * answer says what the void did, in the bot's words.
 */
export function voidCase(guildId: string, caseRef: string, reason: string): Promise<CaseVoided> {
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

/**
 * addEvidence saves up to 10 more message links as evidence, with an
 * optional staff note on each; the answer says how many were new or
 * already saved, in the bot's words.
 */
export function addEvidence(
  guildId: string,
  caseRef: string,
  links: string[],
  note?: string,
): Promise<EvidenceAdded> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/cases/{case_ref}/evidence", {
      params: casePath(guildId, caseRef),
      body: note?.trim() ? { links, note } : { links },
      ...init,
    }),
  );
}

/**
 * uploadEvidence sends 1–10 files (and an optional note) as one evidence
 * item (route 60). Quack copies them to the evidence channel; only staff
 * see them until someone shows them to the member.
 */
export function uploadEvidence(
  guildId: string,
  caseRef: string,
  files: File[],
  note: string,
  onProgress?: (p: UploadProgress) => void,
): Promise<EvidenceAdded> {
  const form = new FormData();
  for (const f of files) form.append("files", f, f.name);
  if (note.trim()) form.append("note", note.trim());
  const url = `/api/guilds/${encodeURIComponent(guildId)}/cases/${encodeURIComponent(caseRef)}/evidence/uploads`;
  return upload<EvidenceAdded>(url, form, onProgress);
}

const itemPath = (guildId: string, caseRef: string, evidenceId: string) => ({
  path: { guild_id: guildId, case_ref: caseRef, evidence_id: evidenceId },
});

/**
 * setEvidenceNote saves or clears (null or blank) an item's staff note. It
 * sends the revision it read; a newer one is 409 note_changed.
 */
export function setEvidenceNote(
  guildId: string,
  caseRef: string,
  evidenceId: string,
  note: string | null,
  expectedRevision: number,
): Promise<EvidenceEdited> {
  return write((init) =>
    api.PUT("/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/note", {
      params: itemPath(guildId, caseRef, evidenceId),
      body: { note, expected_note_revision: expectedRevision },
      ...init,
    }),
  );
}

/** setEvidenceVisibility shows uploaded files to the case's member, or hides them. */
export function setEvidenceVisibility(
  guildId: string,
  caseRef: string,
  evidenceId: string,
  visible: boolean,
): Promise<EvidenceEdited> {
  return write((init) =>
    api.PUT("/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/visibility", {
      params: itemPath(guildId, caseRef, evidenceId),
      body: { visible_to_member: visible },
      ...init,
    }),
  );
}

/**
 * removeEvidence hides an item added by mistake. It stays in the record,
 * listed collapsed here, and is never deleted.
 */
export function removeEvidence(
  guildId: string,
  caseRef: string,
  evidenceId: string,
): Promise<EvidenceEdited> {
  return write((init) =>
    api.POST("/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/remove", {
      params: itemPath(guildId, caseRef, evidenceId),
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
