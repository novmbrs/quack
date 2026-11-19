-- Quack schema, version 1: the SQL blocks of docs/design.md §15.1–§15.7,
-- concatenated in that order. Immutable once released; later changes are new
-- files (0002_…).

CREATE TYPE case_source            AS ENUM ('discord', 'dashboard', 'honeypot', 'imported');
CREATE TYPE void_cause             AS ENUM ('staff', 'appeal_accepted');
CREATE TYPE action_kind            AS ENUM ('timeout', 'kick', 'ban', 'remove_timeout', 'unban');
CREATE TYPE action_state           AS ENUM ('pending', 'running', 'done', 'failed', 'cancelled', 'skipped');
CREATE TYPE action_failure         AS ENUM ('quack_not_in_server', 'quack_missing_permission', 'quack_role_too_low',
                                            'member_is_owner', 'member_not_in_server', 'member_is_administrator',
                                            'timeout_window_passed', 'discord_refused', 'retries_exhausted',
                                            'unconfirmed');
CREATE TYPE action_skip_reason     AS ENUM ('other_case_applies', 'changed_outside_case');
CREATE TYPE attempt_outcome        AS ENUM ('done', 'no_change', 'refused', 'not_sent', 'unknown', 'interrupted');
CREATE TYPE discord_permission     AS ENUM ('moderate_members', 'kick_members', 'ban_members');
CREATE TYPE dm_kind                AS ENUM ('case', 'appeal_update', 'ticket_closed');
CREATE TYPE dm_state               AS ENUM ('pending', 'sending', 'sent', 'failed', 'unknown', 'not_needed');
CREATE TYPE dm_failure             AS ENUM ('dms_closed', 'not_in_shared_server', 'discord_unavailable', 'discord_refused');
CREATE TYPE evidence_state         AS ENUM ('saving', 'saved', 'failed');
CREATE TYPE evidence_failure       AS ENUM ('message_deleted', 'moderator_no_access', 'quack_no_access', 'other_server', 'discord_error');
CREATE TYPE file_copy_state        AS ENUM ('pending', 'copied', 'link_only', 'too_large', 'unsupported_type', 'copy_failed');
CREATE TYPE appeal_state           AS ENUM ('pending', 'needs_information', 'accepted', 'rejected', 'closed');
CREATE TYPE appeal_event_kind      AS ENUM ('submitted', 'information_requested', 'information_provided',
                                            'accepted', 'rejected', 'closed', 'reopened');
CREATE TYPE mod_log_kind           AS ENUM ('case_created', 'case_create_refused', 'case_voided', 'case_context_edited',
                                            'evidence_added', 'action_done', 'action_failed', 'action_retry_requested',
                                            'action_dismissed', 'action_undo_requested', 'action_undo_skipped',
                                            'dm_sent', 'dm_failed', 'appeal_submitted', 'appeal_information_requested',
                                            'appeal_information_provided', 'appeal_accepted', 'appeal_rejected',
                                            'appeal_closed', 'appeal_reopened', 'rule_created', 'rule_edited',
                                            'rule_archived', 'rule_restored', 'rule_imported', 'settings_changed',
                                            'module_enabled', 'module_disabled', 'module_settings_changed',
                                            'quack_joined', 'quack_left', 'channel_setting_cleared',
                                            'log_channel_failed', 'ticket_opened', 'ticket_closed',
                                            'honeypot_case_failed', 'legacy');
CREATE TYPE mod_log_result         AS ENUM ('ok', 'failed', 'refused');
CREATE TYPE mod_log_source         AS ENUM ('discord', 'dashboard', 'quack', 'imported');
CREATE TYPE message_kind           AS ENUM ('case_receipt', 'appeal_post', 'ticket_panel', 'ticket_post', 'honeypot_warning');
CREATE TYPE message_state          AS ENUM ('active', 'gone');
CREATE TYPE job_kind               AS ENUM ('run_action', 'send_dm', 'sync_message', 'post_mod_log', 'capture_evidence',
                                            'honeypot_case', 'honeypot_cleanup', 'open_ticket', 'close_ticket',
                                            'housekeeping');
CREATE TYPE ticket_state           AS ENUM ('opening', 'open', 'closing', 'closed');
CREATE TYPE ticket_close_reason    AS ENUM ('open_failed');
CREATE TYPE transcript_partial     AS ENUM ('history', 'trimmed');
CREATE TYPE module_disabled_reason AS ENUM ('channel_deleted', 'quack_left');
CREATE TYPE activity_event         AS ENUM ('message_edited', 'message_deleted', 'messages_bulk_deleted', 'member_joined',
                                            'member_left', 'member_banned', 'member_unbanned', 'server_changed',
                                            'channel_changed');
CREATE TYPE honeypot_hit_state     AS ENUM ('pending', 'case_created', 'failed');

-- One row per Discord server Quack has ever joined. Never deleted.
CREATE TABLE guilds (
  guild_id        bigint PRIMARY KEY CHECK (guild_id > 0),          -- the Discord server ID
  name            text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
  icon_hash       text NULL CHECK (char_length(icon_hash) <= 64),
  owner_id        bigint NOT NULL CHECK (owner_id > 0),
  mfa_required    boolean NOT NULL DEFAULT false,                   -- Discord MFA level is elevated
  installed       boolean NOT NULL DEFAULT true,                    -- false after Quack leaves
  first_joined_at timestamptz NOT NULL DEFAULT now(),
  left_at         timestamptz NULL,
  updated_at      timestamptz NOT NULL DEFAULT now()
);

-- Per-server case number counter; its row lock serializes case creation.
CREATE TABLE case_counters (
  guild_id    bigint PRIMARY KEY REFERENCES guilds (guild_id),
  last_number bigint NOT NULL DEFAULT 0 CHECK (last_number BETWEEN 0 AND 9007199254740991)  -- 2^53 - 1
);

-- Discord accounts that signed in to the dashboard; holds the 2FA confirmation.
CREATE TABLE discord_accounts (
  account_id     bigint PRIMARY KEY CHECK (account_id > 0),
  mfa_enabled    boolean NOT NULL,
  mfa_checked_at timestamptz NOT NULL,                              -- when Discord last reported mfa_enabled
  updated_at     timestamptz NOT NULL DEFAULT now()
);

-- Dashboard sessions. Only the SHA-256 of the session ID is stored.
CREATE TABLE sessions (
  id_hash                  bytea PRIMARY KEY CHECK (octet_length(id_hash) = 32),
  account_id               bigint NOT NULL REFERENCES discord_accounts (account_id),
  csrf_token               text NOT NULL CHECK (char_length(csrf_token) = 43),
  discord_token_ciphertext bytea NOT NULL,                          -- AES-256-GCM(nonce || ciphertext || tag)
  key_fingerprint          bytea NOT NULL CHECK (octet_length(key_fingerprint) = 8),  -- which session secret
  created_at               timestamptz NOT NULL DEFAULT now(),
  last_seen_at             timestamptz NOT NULL DEFAULT now(),
  expires_at               timestamptz NOT NULL
);
CREATE INDEX sessions_account ON sessions (account_id);
CREATE INDEX sessions_expiry  ON sessions (expires_at);

-- One-time OAuth login states (10 minutes).
CREATE TABLE oauth_states (
  state_hash    bytea PRIMARY KEY CHECK (octet_length(state_hash) = 32),
  code_verifier text NOT NULL CHECK (char_length(code_verifier) BETWEEN 43 AND 128),
  redirect_to   text NOT NULL CHECK (char_length(redirect_to) BETWEEN 1 AND 2048),
  nonce_hash    bytea NOT NULL CHECK (octet_length(nonce_hash) = 32),
  created_at    timestamptz NOT NULL DEFAULT now(),
  expires_at    timestamptz NOT NULL
);
CREATE INDEX oauth_states_expiry ON oauth_states (expires_at);

-- Rule identity; the definition lives in rule_versions.
CREATE TABLE rules (
  id              uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id        bigint NOT NULL REFERENCES guilds (guild_id),
  name            text NOT NULL CHECK (char_length(name) >= 1),     -- current version's name; 1-100 on write (§3.3)
  current_version integer NOT NULL CHECK (current_version >= 1),
  archived_at     timestamptz NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  created_by      bigint NULL,                                      -- NULL = Quack
  updated_at      timestamptz NOT NULL DEFAULT now(),
  updated_by      bigint NULL,
  UNIQUE (guild_id, id)
);
CREATE UNIQUE INDEX rules_active_name ON rules (guild_id, lower(name)) WHERE archived_at IS NULL;
CREATE INDEX rules_guild ON rules (guild_id, archived_at);

-- Immutable rule versions.
CREATE TABLE rule_versions (
  rule_id       uuid NOT NULL,
  version       integer NOT NULL CHECK (version >= 1),
  guild_id      bigint NOT NULL,
  definition    jsonb NOT NULL CHECK (jsonb_typeof(definition) = 'object'),  -- RuleDefinition (§3.2)
  reconstructed boolean NOT NULL DEFAULT false,                     -- partial, rebuilt by the importer (§19.5)
  created_at    timestamptz NOT NULL DEFAULT now(),
  created_by    bigint NULL,
  PRIMARY KEY (rule_id, version),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id)
);

ALTER TABLE rules ADD FOREIGN KEY (id, current_version)
  REFERENCES rule_versions (rule_id, version) DEFERRABLE INITIALLY DEFERRED;

-- Rejects every UPDATE, DELETE, and TRUNCATE on permanent history.
CREATE FUNCTION reject_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION '% is append-only', TG_TABLE_NAME USING ERRCODE = 'insufficient_privilege';
END $$;
CREATE TRIGGER rule_versions_no_change BEFORE UPDATE OR DELETE ON rule_versions
  FOR EACH ROW EXECUTE FUNCTION reject_change();
CREATE TRIGGER rule_versions_no_truncate BEFORE TRUNCATE ON rule_versions
  FOR EACH STATEMENT EXECUTE FUNCTION reject_change();

-- Core server settings.
CREATE TABLE guild_settings (
  guild_id                       bigint PRIMARY KEY REFERENCES guilds (guild_id),
  version                        bigint NOT NULL DEFAULT 1 CHECK (version >= 1),  -- optimistic check for staff role edits
  moderator_role_ids             bigint[] NOT NULL DEFAULT '{}'
                                 CHECK (cardinality(moderator_role_ids) <= 25 AND array_position(moderator_role_ids, NULL) IS NULL),
  rules_manager_role_ids         bigint[] NOT NULL DEFAULT '{}'
                                 CHECK (cardinality(rules_manager_role_ids) <= 25 AND array_position(rules_manager_role_ids, NULL) IS NULL),
  log_channel_id                 bigint NULL CHECK (log_channel_id > 0),
  appeal_channel_id              bigint NULL CHECK (appeal_channel_id > 0),
  evidence_channel_id            bigint NULL CHECK (evidence_channel_id > 0),
  appeal_invite_url              text NULL CHECK (appeal_invite_url ~ '^https://discord\.gg/[A-Za-z0-9_-]{2,64}$'),
  dm_intro                       text NULL CHECK (char_length(dm_intro) >= 1),   -- 1-1,000 on write
  dm_footer                      text NULL CHECK (char_length(dm_footer) >= 1),  -- 1-500 on write
  starter_rule_id                uuid NULL,
  starter_notice_acknowledged_at timestamptz NULL,
  updated_at                     timestamptz NOT NULL DEFAULT now(),
  updated_by                     bigint NULL,
  FOREIGN KEY (guild_id, starter_rule_id) REFERENCES rules (guild_id, id)
);

-- Module settings (one row each per server, created by bootstrap).
CREATE TABLE ticket_settings (
  guild_id                  bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled                   boolean NOT NULL DEFAULT false,
  disabled_reason           module_disabled_reason NULL,
  entry_channel_id          bigint NULL CHECK (entry_channel_id > 0),
  queue_channel_id          bigint NULL CHECK (queue_channel_id > 0),
  transcript_retention_days smallint NOT NULL DEFAULT 90 CHECK (transcript_retention_days BETWEEN 1 AND 365),
  version                   bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
  updated_at                timestamptz NOT NULL DEFAULT now(),
  updated_by                bigint NULL,
  CHECK (NOT enabled OR (entry_channel_id IS NOT NULL AND queue_channel_id IS NOT NULL
                         AND entry_channel_id <> queue_channel_id)),
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);

CREATE TABLE activity_log_settings (
  guild_id                bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled                 boolean NOT NULL DEFAULT false,
  disabled_reason         module_disabled_reason NULL,
  include_message_content boolean NOT NULL DEFAULT true,
  include_file_names      boolean NOT NULL DEFAULT true,
  include_embed_types     boolean NOT NULL DEFAULT true,
  version                 bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
  updated_at              timestamptz NOT NULL DEFAULT now(),
  updated_by              bigint NULL,
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);

CREATE TABLE activity_log_routes (
  guild_id   bigint NOT NULL REFERENCES activity_log_settings (guild_id),
  event      activity_event NOT NULL,
  channel_id bigint NOT NULL CHECK (channel_id > 0),
  PRIMARY KEY (guild_id, event)
);
CREATE INDEX activity_log_routes_channel ON activity_log_routes (guild_id, channel_id);

CREATE TABLE honeypot_settings (
  guild_id        bigint PRIMARY KEY REFERENCES guilds (guild_id),
  enabled         boolean NOT NULL DEFAULT false,
  disabled_reason module_disabled_reason NULL,
  channel_id      bigint NULL CHECK (channel_id > 0),
  rule_id         uuid NULL,
  warning_text    text NULL CHECK (char_length(warning_text) >= 1),  -- 1-1,500 on write
  version         bigint NOT NULL DEFAULT 1 CHECK (version >= 1),    -- stamped on every hit (§13.4.2)
  updated_at      timestamptz NOT NULL DEFAULT now(),
  updated_by      bigint NULL,
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  CHECK (NOT enabled OR (channel_id IS NOT NULL AND rule_id IS NOT NULL)),
  CHECK (NOT (enabled AND disabled_reason IS NOT NULL))
);

-- One row per case. Never deleted.
CREATE TABLE cases (
  id                         uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                   bigint NOT NULL REFERENCES guilds (guild_id),
  number                     bigint NOT NULL CHECK (number BETWEEN 1 AND 9007199254740991),
  member_id                  bigint NOT NULL CHECK (member_id > 0),
  member_joined_at           timestamptz NULL,                    -- the membership the case is about; NULL when imported
  source                     case_source NOT NULL,
  moderator_id               bigint NULL CHECK (moderator_id > 0), -- NULL for honeypot and some imported cases
  rule_id                    uuid NULL,
  rule_version               integer NULL,
  case_count                 integer NULL CHECK (case_count >= 1),
  level_index                smallint NULL CHECK (level_index >= 1),
  level_from_case_count      integer NULL CHECK (level_from_case_count >= 1),
  action_kind                action_kind NULL CHECK (action_kind IN ('timeout', 'kick', 'ban')),
  timeout_seconds            integer NULL CHECK (timeout_seconds BETWEEN 1 AND 2419200),
  ban_delete_message_seconds integer NULL CHECK (ban_delete_message_seconds BETWEEN 0 AND 604800),
  sends_dm                   boolean NOT NULL,
  context                    jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(context) = 'object'),  -- field key -> value
  context_revision           integer NOT NULL DEFAULT 1 CHECK (context_revision >= 1),
  operation_key              text NULL CHECK (char_length(operation_key) BETWEEN 1 AND 400),  -- <account>:<operation>:<key>
  request_hash               bytea NULL CHECK (octet_length(request_hash) = 32),
  replaces_case_id           uuid NULL,
  voided_at                  timestamptz NULL,
  voided_by                  bigint NULL,                         -- NULL = Quack or unknown (imported)
  void_reason                text NULL CHECK (char_length(void_reason) >= 1),  -- 1-1,000 on write
  void_cause                 void_cause NULL,
  imported                   jsonb NULL CHECK (jsonb_typeof(imported) = 'object'),  -- {v4_action, v4_moderator, v4_reason, v4_source_id}
  created_at                 timestamptz NOT NULL DEFAULT now(),
  updated_at                 timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, number),
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, id, member_id),
  UNIQUE (guild_id, operation_key),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  FOREIGN KEY (rule_id, rule_version) REFERENCES rule_versions (rule_id, version),
  FOREIGN KEY (guild_id, replaces_case_id) REFERENCES cases (guild_id, id),
  CHECK (replaces_case_id IS DISTINCT FROM id),
  CHECK ((rule_id IS NULL) = (rule_version IS NULL)),
  CHECK (request_hash IS NULL OR operation_key IS NOT NULL),
  CHECK ((voided_at IS NULL) = (void_reason IS NULL) AND (voided_at IS NULL) = (void_cause IS NULL)),
  CHECK ((source = 'imported') = (imported IS NOT NULL)),
  CHECK (source = 'imported' OR (rule_id IS NOT NULL AND case_count IS NOT NULL
                                 AND level_index IS NOT NULL AND level_from_case_count IS NOT NULL)),
  CHECK (source IN ('honeypot', 'imported') OR moderator_id IS NOT NULL),
  CHECK ((action_kind IS NOT DISTINCT FROM 'timeout') = (timeout_seconds IS NOT NULL)),
  CHECK ((action_kind IS NOT DISTINCT FROM 'ban') = (ban_delete_message_seconds IS NOT NULL)),
  CHECK (source <> 'imported' OR (action_kind IS NULL AND NOT sends_dm))
);
CREATE INDEX cases_escalation ON cases (guild_id, member_id, rule_id, created_at)
  WHERE voided_at IS NULL AND source <> 'imported';
CREATE UNIQUE INDEX cases_replaces ON cases (replaces_case_id) WHERE replaces_case_id IS NOT NULL;
CREATE INDEX cases_member     ON cases (guild_id, member_id, number DESC);
CREATE INDEX cases_rule       ON cases (guild_id, rule_id, number DESC);
CREATE INDEX cases_moderator  ON cases (guild_id, moderator_id, number DESC) WHERE moderator_id IS NOT NULL;
CREATE INDEX cases_created    ON cases (guild_id, created_at);                -- statistics
CREATE INDEX cases_honeypot   ON cases (guild_id) WHERE source = 'honeypot';  -- the warning's count
CREATE INDEX cases_member_all ON cases (member_id, created_at DESC, id DESC);  -- a member's own cases

-- Original actions (timeout, kick, ban) and undo actions (remove_timeout, unban).
CREATE TABLE actions (
  id                      uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                bigint NOT NULL,
  case_id                 uuid NOT NULL,
  member_id               bigint NOT NULL,                        -- the case's member (composite FK), for the undo guard
  kind                    action_kind NOT NULL,
  undo_of_action_id       uuid NULL,                              -- set exactly for undo actions; same case
  state                   action_state NOT NULL DEFAULT 'pending',
  timeout_ends_at         timestamptz NULL,                       -- fixed at case creation, whole seconds (§5.2)
  delete_message_seconds  integer NULL CHECK (delete_message_seconds BETWEEN 0 AND 604800),
  requested_by            bigint NULL,                            -- staff who requested an undo or retry; NULL = Quack
  attempts                integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),  -- in the current round
  next_attempt_at         timestamptz NULL,
  last_attempt_at         timestamptz NULL,
  failure_reason          action_failure NULL,
  failure_permission      discord_permission NULL,                -- for quack_missing_permission
  failure_http_status     smallint NULL,
  failure_discord_code    integer NULL,
  skip_reason             action_skip_reason NULL,
  skipped_because_case_id uuid NULL,
  no_change               boolean NOT NULL DEFAULT false,         -- done because the desired state already held
  done_at                 timestamptz NULL,
  dismissed_at            timestamptz NULL,
  dismissed_by            bigint NULL,
  created_at              timestamptz NOT NULL DEFAULT now(),
  updated_at              timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, case_id, id),
  UNIQUE (undo_of_action_id),
  FOREIGN KEY (guild_id, case_id, member_id) REFERENCES cases (guild_id, id, member_id),
  FOREIGN KEY (guild_id, case_id, undo_of_action_id) REFERENCES actions (guild_id, case_id, id),
  FOREIGN KEY (guild_id, skipped_because_case_id) REFERENCES cases (guild_id, id),
  CHECK (undo_of_action_id IS DISTINCT FROM id),
  CHECK ((kind IN ('remove_timeout', 'unban')) = (undo_of_action_id IS NOT NULL)),
  CHECK ((kind = 'timeout') = (timeout_ends_at IS NOT NULL)),
  CHECK ((kind = 'ban') = (delete_message_seconds IS NOT NULL)),
  CHECK ((state = 'failed') = (failure_reason IS NOT NULL)),
  CHECK (state = 'failed' OR (failure_permission IS NULL AND failure_http_status IS NULL
                              AND failure_discord_code IS NULL)),
  CHECK ((failure_reason IS NOT DISTINCT FROM 'quack_missing_permission') = (failure_permission IS NOT NULL)),
  CHECK ((state = 'skipped') = (skip_reason IS NOT NULL)),
  CHECK ((skip_reason IS NOT DISTINCT FROM 'other_case_applies') = (skipped_because_case_id IS NOT NULL)),
  CHECK (state <> 'skipped' OR undo_of_action_id IS NOT NULL),
  CHECK (state <> 'cancelled' OR undo_of_action_id IS NULL),
  CHECK ((state = 'done') = (done_at IS NOT NULL)),
  CHECK (NOT no_change OR state = 'done'),
  CHECK (dismissed_at IS NULL OR state = 'failed'),
  CHECK ((dismissed_at IS NULL) = (dismissed_by IS NULL))
);
CREATE UNIQUE INDEX actions_one_original ON actions (case_id) WHERE undo_of_action_id IS NULL;
CREATE INDEX actions_case   ON actions (case_id);
CREATE INDEX actions_member ON actions (guild_id, member_id, kind) WHERE state IN ('running', 'done');
CREATE INDEX actions_review ON actions (guild_id, created_at DESC, id DESC) WHERE state = 'failed' AND dismissed_at IS NULL;
CREATE INDEX actions_guild_state ON actions (guild_id, state, created_at DESC, id DESC);

-- Every attempt, opened before the Discord call.
CREATE TABLE action_attempts (
  guild_id     bigint NOT NULL,
  action_id    uuid NOT NULL,
  number       integer NOT NULL CHECK (number >= 1),               -- across all rounds
  started_at   timestamptz NOT NULL,
  finished_at  timestamptz NULL,
  outcome      attempt_outcome NULL,
  http_status  smallint NULL,
  discord_code integer NULL,
  PRIMARY KEY (action_id, number),
  FOREIGN KEY (guild_id, action_id) REFERENCES actions (guild_id, id),
  CHECK ((finished_at IS NULL) = (outcome IS NULL)),
  CHECK (finished_at IS NULL OR finished_at >= started_at)
);
CREATE UNIQUE INDEX action_attempts_one_open ON action_attempts (action_id) WHERE finished_at IS NULL;

-- Saved messages. One row per (case, message).
CREATE TABLE evidence (
  id                          uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id                    bigint NOT NULL,
  case_id                     uuid NOT NULL,
  channel_id                  bigint NOT NULL CHECK (channel_id > 0),
  message_id                  bigint NOT NULL CHECK (message_id > 0),
  added_by                    bigint NULL,                        -- NULL = Quack (honeypot, imported)
  state                       evidence_state NOT NULL DEFAULT 'saving',
  failure                     evidence_failure NULL,
  author_id                   bigint NULL,
  content                     text NULL CHECK (char_length(content) <= 4000),
  message_created_at          timestamptz NULL,
  message_edited_at           timestamptz NULL,
  embeds                      jsonb NOT NULL DEFAULT '[]'
                              CHECK (jsonb_typeof(embeds) = 'array' AND jsonb_array_length(embeds) <= 10),
  files_not_saved             integer NOT NULL DEFAULT 0 CHECK (files_not_saved >= 0),
  copy_skipped_not_staff_only boolean NOT NULL DEFAULT false,
  created_at                  timestamptz NOT NULL DEFAULT now(),
  saved_at                    timestamptz NULL,
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, case_id, id),
  UNIQUE (case_id, message_id),
  FOREIGN KEY (guild_id, case_id) REFERENCES cases (guild_id, id),
  CHECK ((state = 'failed') = (failure IS NOT NULL)),
  CHECK ((state = 'saved') = (saved_at IS NOT NULL))
);
CREATE INDEX evidence_case ON evidence (case_id, created_at, id);

-- Files of a saved message (at most 10 per message, 100 per case), and their copies.
CREATE TABLE evidence_files (
  id                 uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id           bigint NOT NULL,
  case_id            uuid NOT NULL,
  evidence_id        uuid NOT NULL,
  position           smallint NOT NULL CHECK (position BETWEEN 1 AND 10),
  attachment_id      bigint NULL CHECK (attachment_id > 0),       -- the original's Discord attachment ID
  filename           text NOT NULL CHECK (char_length(filename) BETWEEN 1 AND 255),
  content_type       text NULL CHECK (char_length(content_type) <= 191),
  size_bytes         bigint NOT NULL CHECK (size_bytes >= 0),
  original_url       text NOT NULL CHECK (char_length(original_url) <= 2048),
  copy_state         file_copy_state NOT NULL DEFAULT 'pending',
  copy_started_at    timestamptz NULL,                            -- the upload's nonce window (§10.4)
  copy_channel_id    bigint NULL,
  copy_message_id    bigint NULL,
  copy_attachment_id bigint NULL,
  created_at         timestamptz NOT NULL DEFAULT now(),
  UNIQUE (evidence_id, position),
  FOREIGN KEY (guild_id, case_id, evidence_id) REFERENCES evidence (guild_id, case_id, id),
  CHECK (num_nonnulls(copy_channel_id, copy_message_id, copy_attachment_id)
         = CASE WHEN copy_state = 'copied' THEN 3 ELSE 0 END)
);
CREATE INDEX evidence_files_case ON evidence_files (case_id);

-- One appeal per case.
CREATE TABLE appeals (
  id               uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id         bigint NOT NULL,
  case_id          uuid NOT NULL,
  member_id        bigint NOT NULL,                               -- the case's member (composite FK)
  state            appeal_state NOT NULL DEFAULT 'pending',
  version          integer NOT NULL DEFAULT 1 CHECK (version >= 1),
  statement        text NOT NULL CHECK (char_length(statement) >= 1),  -- 1-4,000 on write
  decided_by       bigint NULL,                                   -- latest staff decision
  decided_at       timestamptz NULL,
  decision_reason  text NULL CHECK (char_length(decision_reason) >= 1),
  first_decided_at timestamptz NULL,                              -- first accept/reject/close, for metrics
  created_at       timestamptz NOT NULL DEFAULT now(),
  updated_at       timestamptz NOT NULL DEFAULT now(),
  UNIQUE (case_id),
  UNIQUE (guild_id, id),
  FOREIGN KEY (guild_id, case_id, member_id) REFERENCES cases (guild_id, id, member_id)
);
CREATE INDEX appeals_guild_state   ON appeals (guild_id, state, created_at, id);
CREATE INDEX appeals_guild_created ON appeals (guild_id, created_at DESC, id DESC);

-- Appeal timeline: statement, replies, decisions.
CREATE TABLE appeal_events (
  id             uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id       bigint NOT NULL,
  appeal_id      uuid NOT NULL,
  kind           appeal_event_kind NOT NULL,
  actor_id       bigint NULL,                                     -- member or staff; NULL = Quack
  text           text NOT NULL CHECK (char_length(text) >= 1),    -- 1-4,000 (replies) or 1-2,000 (reasons) on write
  closed_by_void boolean NOT NULL DEFAULT false,
  created_at     timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  FOREIGN KEY (guild_id, appeal_id) REFERENCES appeals (guild_id, id),
  CHECK (NOT closed_by_void OR (kind = 'closed' AND actor_id IS NULL))
);
CREATE INDEX appeal_events_appeal ON appeal_events (appeal_id, created_at, id);

CREATE TABLE tickets (
  id                   uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id             bigint NOT NULL REFERENCES guilds (guild_id),
  member_id            bigint NOT NULL CHECK (member_id > 0),
  state                ticket_state NOT NULL DEFAULT 'opening',
  entry_channel_id     bigint NOT NULL,                           -- the thread's parent
  thread_id            bigint NULL UNIQUE,                        -- named ticket-<id u22>
  opened_at            timestamptz NULL,
  close_requested_at   timestamptz NULL,
  closed_at            timestamptz NULL,
  closed_by            bigint NULL,                               -- member or staff; NULL = Quack
  close_reason         ticket_close_reason NULL,
  journal_gaps         boolean NOT NULL DEFAULT false,            -- the journal may be missing messages
  welcome_started_at   timestamptz NULL,                          -- the welcome's nonce window (§10.4)
  welcome_posted_at    timestamptz NULL,
  frozen_at            timestamptz NULL,                          -- close step 1 done
  transcript_saved_at  timestamptz NULL,                          -- close step 2 done
  thread_deleted_at    timestamptz NULL,                          -- close step 4 done
  thread_delete_failed boolean NOT NULL DEFAULT false,
  created_at           timestamptz NOT NULL DEFAULT now(),
  updated_at           timestamptz NOT NULL DEFAULT now(),
  UNIQUE (guild_id, id),
  UNIQUE (guild_id, id, member_id),
  CHECK ((state = 'closed') = (closed_at IS NOT NULL)),
  CHECK (state <> 'open' OR (thread_id IS NOT NULL AND opened_at IS NOT NULL)),
  CHECK (close_reason IS NULL OR state IN ('closing', 'closed'))
);
CREATE UNIQUE INDEX tickets_one_live ON tickets (guild_id, member_id) WHERE state IN ('opening', 'open', 'closing');
CREATE INDEX tickets_guild_state ON tickets (guild_id, state, created_at, id);

-- Ticket journal: original text of every message seen in a ticket thread before it froze.
CREATE TABLE ticket_messages (
  guild_id    bigint NOT NULL,
  ticket_id   uuid NOT NULL,
  message_id  bigint NOT NULL,
  author_id   bigint NOT NULL,
  author_name text NOT NULL CHECK (char_length(author_name) >= 1),
  text        text NOT NULL,
  files       jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(files) = 'array'),  -- [{name, size}]
  sent_at     timestamptz NOT NULL,
  expires_at  timestamptz NULL,                                   -- set at close
  PRIMARY KEY (ticket_id, message_id),
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id)
);
CREATE INDEX ticket_messages_expiry ON ticket_messages (expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE ticket_transcripts (
  ticket_id     uuid PRIMARY KEY,
  guild_id      bigint NOT NULL,
  content       text NOT NULL CHECK (octet_length(content) <= 8388608),
  complete      boolean NOT NULL,
  partial_cause transcript_partial NULL,
  captured_at   timestamptz NOT NULL DEFAULT now(),
  expires_at    timestamptz NOT NULL,
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id),
  CHECK (complete = (partial_cause IS NULL))
);
CREATE INDEX ticket_transcripts_expiry ON ticket_transcripts (expires_at);

-- Case DMs, appeal update DMs, ticket transcript DMs.
CREATE TABLE dms (
  id                   uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id             bigint NOT NULL,
  member_id            bigint NOT NULL,
  kind                 dm_kind NOT NULL,
  case_id              uuid NULL,
  appeal_event_id      uuid NULL,
  ticket_id            uuid NULL,
  state                dm_state NOT NULL DEFAULT 'pending',
  failure              dm_failure NULL,
  failure_discord_code integer NULL,
  channel_id           bigint NULL,                               -- the DM channel
  message_id           bigint NULL,
  first_attempt_at     timestamptz NULL,                          -- start of the nonce window (§10.4)
  settled_at           timestamptz NULL,
  created_at           timestamptz NOT NULL DEFAULT now(),
  UNIQUE (appeal_event_id),
  UNIQUE (ticket_id),
  FOREIGN KEY (guild_id, case_id, member_id) REFERENCES cases (guild_id, id, member_id),
  FOREIGN KEY (guild_id, appeal_event_id) REFERENCES appeal_events (guild_id, id),
  FOREIGN KEY (guild_id, ticket_id, member_id) REFERENCES tickets (guild_id, id, member_id),
  CHECK (kind <> 'case'          OR (case_id IS NOT NULL AND appeal_event_id IS NULL AND ticket_id IS NULL)),
  CHECK (kind <> 'appeal_update' OR (case_id IS NOT NULL AND appeal_event_id IS NOT NULL AND ticket_id IS NULL)),
  CHECK (kind <> 'ticket_closed' OR (ticket_id IS NOT NULL AND case_id IS NULL AND appeal_event_id IS NULL)),
  CHECK ((state = 'failed') = (failure IS NOT NULL)),
  CHECK (state NOT IN ('sending', 'sent') OR (channel_id IS NOT NULL AND first_attempt_at IS NOT NULL)),
  CHECK ((state = 'sent') = (message_id IS NOT NULL)),
  CHECK ((settled_at IS NULL) = (state IN ('pending', 'sending')))
);
CREATE UNIQUE INDEX dms_one_per_case ON dms (case_id) WHERE kind = 'case';

-- Append-only moderation log (§9).
CREATE TABLE mod_log (
  id         uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id   bigint NOT NULL REFERENCES guilds (guild_id),
  kind       mod_log_kind NOT NULL,
  result     mod_log_result NOT NULL,
  actor_id   bigint NULL,                                         -- NULL = Quack
  source     mod_log_source NOT NULL,
  case_id    uuid NULL,
  member_id  bigint NULL,
  appeal_id  uuid NULL,
  action_id  uuid NULL,
  rule_id    uuid NULL,
  ticket_id  uuid NULL,
  detail     jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(detail) = 'object'),
  trace_id   text NULL CHECK (trace_id ~ '^[0-9a-f]{32}$'),
  created_at timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (guild_id, case_id)   REFERENCES cases (guild_id, id),
  FOREIGN KEY (guild_id, appeal_id) REFERENCES appeals (guild_id, id),
  FOREIGN KEY (guild_id, action_id) REFERENCES actions (guild_id, id),
  FOREIGN KEY (guild_id, rule_id)   REFERENCES rules (guild_id, id),
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id)
);
CREATE INDEX mod_log_guild_time ON mod_log (guild_id, created_at DESC, id DESC);
CREATE INDEX mod_log_kind       ON mod_log (guild_id, kind, created_at DESC, id DESC);
CREATE INDEX mod_log_case       ON mod_log (case_id, created_at, id) WHERE case_id IS NOT NULL;
CREATE INDEX mod_log_member     ON mod_log (guild_id, member_id, created_at DESC, id DESC) WHERE member_id IS NOT NULL;
CREATE INDEX mod_log_actor      ON mod_log (guild_id, actor_id, created_at DESC, id DESC) WHERE actor_id IS NOT NULL;
CREATE INDEX mod_log_appeal     ON mod_log (appeal_id) WHERE appeal_id IS NOT NULL;
CREATE INDEX mod_log_rule       ON mod_log (rule_id, created_at DESC, id DESC) WHERE rule_id IS NOT NULL;
CREATE TRIGGER mod_log_no_change BEFORE UPDATE OR DELETE ON mod_log
  FOR EACH ROW EXECUTE FUNCTION reject_change();
CREATE TRIGGER mod_log_no_truncate BEFORE TRUNCATE ON mod_log
  FOR EACH STATEMENT EXECUTE FUNCTION reject_change();

-- Tracked Discord messages (§10). The owner column matches the kind.
CREATE TABLE messages (
  id                uuid PRIMARY KEY DEFAULT uuidv7(),
  guild_id          bigint NOT NULL REFERENCES guilds (guild_id),
  kind              message_kind NOT NULL,
  case_id           uuid NULL,
  appeal_id         uuid NULL,
  ticket_id         uuid NULL,
  channel_id        bigint NULL,                                  -- NULL: an appeal post waiting for a channel
  message_id        bigint NULL,                                  -- NULL until posted
  generation        integer NOT NULL DEFAULT 1 CHECK (generation >= 1),
  create_started_at timestamptz NULL,                             -- the current create's nonce window (§10.4)
  private           boolean NOT NULL DEFAULT false,               -- a private receipt (token edits only)
  interaction_token text NULL,                                    -- secret; never logged
  token_expires_at  timestamptz NULL,
  wanted_revision   integer NOT NULL DEFAULT 1,
  posted_revision   integer NOT NULL DEFAULT 0,
  wanted_at         timestamptz NOT NULL DEFAULT now(),
  state             message_state NOT NULL DEFAULT 'active',
  stale             jsonb NOT NULL DEFAULT '[]'                   -- [{channel_id, message_id}] to clean up
                    CHECK (jsonb_typeof(stale) = 'array' AND jsonb_array_length(stale) <= 5),
  attachment_id     bigint NULL,                                  -- the attached transcript file
  last_error        text NULL CHECK (char_length(last_error) <= 200),  -- machine code, e.g. "http_403_50001"
  created_at        timestamptz NOT NULL DEFAULT now(),
  updated_at        timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (guild_id, case_id)   REFERENCES cases (guild_id, id),
  FOREIGN KEY (guild_id, appeal_id) REFERENCES appeals (guild_id, id),
  FOREIGN KEY (guild_id, ticket_id) REFERENCES tickets (guild_id, id),
  CHECK ((kind = 'case_receipt') = (case_id IS NOT NULL)),
  CHECK ((kind = 'appeal_post') = (appeal_id IS NOT NULL)),
  CHECK ((kind = 'ticket_post') = (ticket_id IS NOT NULL)),
  CHECK (channel_id IS NOT NULL OR kind = 'appeal_post'),
  CHECK ((interaction_token IS NULL) = (token_expires_at IS NULL)),
  CHECK (interaction_token IS NULL OR kind = 'case_receipt'),
  CHECK (NOT private OR kind = 'case_receipt'),
  CHECK (wanted_revision >= 1 AND posted_revision BETWEEN 0 AND wanted_revision)
);
CREATE UNIQUE INDEX messages_case      ON messages (case_id) WHERE case_id IS NOT NULL;
CREATE UNIQUE INDEX messages_appeal    ON messages (appeal_id) WHERE appeal_id IS NOT NULL;
CREATE UNIQUE INDEX messages_ticket    ON messages (ticket_id) WHERE ticket_id IS NOT NULL;
CREATE UNIQUE INDEX messages_singleton ON messages (guild_id, kind) WHERE kind IN ('ticket_panel', 'honeypot_warning');
CREATE INDEX messages_location         ON messages (channel_id, message_id) WHERE message_id IS NOT NULL;
CREATE INDEX messages_waiting          ON messages (guild_id) WHERE kind = 'appeal_post' AND (channel_id IS NULL OR state = 'gone');

-- The job queue (§16).
CREATE TABLE jobs (
  id            uuid PRIMARY KEY DEFAULT uuidv7(),
  kind          job_kind NOT NULL,
  key           text NOT NULL UNIQUE CHECK (char_length(key) BETWEEN 1 AND 200),  -- <kind>:<owner ids>
  guild_id      bigint NULL REFERENCES guilds (guild_id),         -- NULL only for housekeeping
  payload       jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(payload) = 'object'),  -- immutable IDs only
  run_at        timestamptz NOT NULL DEFAULT now(),
  attempts      integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts  integer NOT NULL CHECK (max_attempts >= 1),
  claim_token   uuid NULL,                                        -- the current claim's fence
  locked_until  timestamptz NULL,
  rerun_at      timestamptz NULL,                                 -- enqueued again while claimed
  last_error    text NULL CHECK (char_length(last_error) <= 500),
  trace_context text NULL CHECK (char_length(trace_context) <= 256),  -- W3C traceparent of the first enqueuer
  dead_at       timestamptz NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now(),
  CHECK (key = kind::text OR starts_with(key, kind::text || ':')),
  CHECK ((claim_token IS NULL) = (locked_until IS NULL)),
  CHECK (dead_at IS NULL OR claim_token IS NULL),
  CHECK ((kind = 'housekeeping') = (guild_id IS NULL))
);
CREATE INDEX jobs_runnable ON jobs (kind, run_at, id) WHERE dead_at IS NULL AND locked_until IS NULL;
CREATE INDEX jobs_leased   ON jobs (locked_until) WHERE locked_until IS NOT NULL;
CREATE INDEX jobs_dead     ON jobs (kind, dead_at) WHERE dead_at IS NOT NULL;

-- Operation keys (§4.3). `response` is a result reference, e.g. {"case_id": "…"}.
CREATE TABLE operation_keys (
  guild_id        bigint NOT NULL CHECK (guild_id >= 0),          -- 0 = no server (DMs, /api/me)
  account_id      bigint NOT NULL CHECK (account_id >= 0),        -- 0 = Quack
  operation       text NOT NULL CHECK (char_length(operation) BETWEEN 1 AND 120),
  key             text NOT NULL CHECK (char_length(key) BETWEEN 1 AND 200),
  request_hash    bytea NOT NULL CHECK (octet_length(request_hash) = 32),
  response_status smallint NOT NULL,
  response        jsonb NOT NULL CHECK (jsonb_typeof(response) = 'object'),
  created_at      timestamptz NOT NULL DEFAULT now(),
  expires_at      timestamptz NOT NULL,
  PRIMARY KEY (guild_id, account_id, operation, key)
);
CREATE INDEX operation_keys_expiry ON operation_keys (expires_at);

-- /case add drafts (§11.6).
CREATE TABLE case_drafts (
  id                 uuid PRIMARY KEY,
  guild_id           bigint NOT NULL REFERENCES guilds (guild_id),
  actor_id           bigint NOT NULL,
  member_id          bigint NOT NULL,
  rule_id            uuid NULL,
  rule_version       integer NULL,
  private            boolean NOT NULL,
  receipt_channel_id bigint NOT NULL,                             -- where the command was used
  source_channel_id  bigint NULL,                                 -- the message command's message
  source_message_id  bigint NULL,
  values             jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(values) = 'object'),
  page               smallint NOT NULL DEFAULT 1 CHECK (page BETWEEN 1 AND 3),  -- 3 = every page saved
  expires_at         timestamptz NOT NULL,
  created_at         timestamptz NOT NULL DEFAULT now(),
  updated_at         timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  FOREIGN KEY (rule_id, rule_version) REFERENCES rule_versions (rule_id, version),
  CHECK ((rule_id IS NULL) = (rule_version IS NULL)),
  CHECK ((source_channel_id IS NULL) = (source_message_id IS NULL))
);
CREATE INDEX case_drafts_expiry ON case_drafts (expires_at);

-- Importer runs and per-server completion (§19).
CREATE TABLE import_runs (
  id                 uuid PRIMARY KEY DEFAULT uuidv7(),
  mode               text NOT NULL CHECK (mode IN ('dry_run', 'apply')),
  source_fingerprint text NOT NULL,                               -- SHA-256 over the source snapshot (§19.11)
  transform_version  integer NOT NULL,
  evaluated_at       timestamptz NOT NULL,                        -- the fixed "now" of the import
  started_at         timestamptz NOT NULL DEFAULT now(),
  finished_at        timestamptz NULL,
  report             jsonb NULL
);
CREATE TABLE import_guilds (
  guild_id           bigint PRIMARY KEY REFERENCES guilds (guild_id),
  run_id             uuid NOT NULL REFERENCES import_runs (id),
  source_fingerprint text NOT NULL,
  imported_at        timestamptz NOT NULL DEFAULT now()
);

-- One row per post in the honeypot channel. Pruned after 30 days (§16.6).
CREATE TABLE honeypot_hits (
  guild_id          bigint NOT NULL REFERENCES guilds (guild_id),
  message_id        bigint NOT NULL,
  channel_id        bigint NOT NULL,
  member_id         bigint NOT NULL,
  settings_version  bigint NOT NULL,                              -- honeypot_settings.version when admitted
  rule_id           uuid NULL,                                    -- honeypot_settings.rule_id when admitted
  state             honeypot_hit_state NOT NULL DEFAULT 'pending',
  case_id           uuid NULL,
  failure           text NULL CHECK (char_length(failure) <= 64), -- a MemberRefusal code, exempt, policy_changed, ...
  cleaned_up_at     timestamptz NULL,                             -- the post was deleted
  cleanup_failed_at timestamptz NULL,
  created_at        timestamptz NOT NULL DEFAULT now(),
  updated_at        timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (guild_id, message_id),
  FOREIGN KEY (guild_id, case_id, member_id) REFERENCES cases (guild_id, id, member_id),
  FOREIGN KEY (guild_id, rule_id) REFERENCES rules (guild_id, id),
  CHECK ((state = 'case_created') = (case_id IS NOT NULL)),
  CHECK ((state = 'failed') = (failure IS NOT NULL)),
  CHECK (cleaned_up_at IS NULL OR cleanup_failed_at IS NULL)
);
CREATE INDEX honeypot_hits_pending ON honeypot_hits (guild_id, member_id, message_id) WHERE state = 'pending';
CREATE INDEX honeypot_hits_burst   ON honeypot_hits (guild_id, member_id, created_at) WHERE state = 'case_created';
CREATE INDEX honeypot_hits_expiry  ON honeypot_hits (created_at);

