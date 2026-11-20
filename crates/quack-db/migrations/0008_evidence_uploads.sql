-- Evidence beyond message links (design §7): uploaded-files items, staff
-- notes, member visibility, removal (hidden, never deleted), the messages
-- around a saved message, dashboard upload bytes waiting for their copy,
-- and a `/case add` draft's file.

CREATE TYPE evidence_kind AS ENUM ('message', 'upload');

-- An uploaded-files item where no file ended up copied (§7.2), and a
-- message in the evidence channel or one of Quack's copies there, which is
-- never a source (§7.1).
ALTER TYPE evidence_failure ADD VALUE 'no_file_copied';
ALTER TYPE evidence_failure ADD VALUE 'evidence_copy';

-- Removal and visibility changes are recorded, not posted (§9.2).
ALTER TYPE mod_log_kind ADD VALUE 'evidence_removed';
ALTER TYPE mod_log_kind ADD VALUE 'evidence_visibility_changed';

-- Uploaded-files items have no source message; `UNIQUE (case_id,
-- message_id)` keeps one row per message, and NULLs never collide.
ALTER TABLE evidence
  ADD COLUMN kind               evidence_kind NOT NULL DEFAULT 'message',
  ADD COLUMN visible_to_member  boolean NOT NULL DEFAULT true,
  ADD COLUMN note               text NULL CHECK (char_length(note) BETWEEN 1 AND 500),
  ADD COLUMN note_revision      integer NOT NULL DEFAULT 0 CHECK (note_revision >= 0),
  ADD COLUMN note_updated_by    bigint NULL,
  ADD COLUMN note_updated_at    timestamptz NULL,
  ADD COLUMN removed_at         timestamptz NULL,
  ADD COLUMN removed_by         bigint NULL,
  -- Up to 5 messages before and 5 after a staff-added message, text only;
  -- NULL when none were saved (Quack's own items, a failed fetch, older rows).
  ADD COLUMN surrounding        jsonb NULL
                                CHECK (jsonb_typeof(surrounding) = 'array' AND jsonb_array_length(surrounding) <= 10),
  -- Whose access the capture checks: the staff member whose request last
  -- started it (a new item, or one captured again); `added_by` keeps who
  -- first added it. NULL = Quack, and rows from before this migration,
  -- which fall back to `added_by`.
  ADD COLUMN capture_by         bigint NULL,
  ALTER COLUMN channel_id DROP NOT NULL,
  ALTER COLUMN message_id DROP NOT NULL,
  ADD CONSTRAINT evidence_source CHECK (
    (kind = 'message') = (channel_id IS NOT NULL AND message_id IS NOT NULL)
    AND (kind = 'message' OR (channel_id IS NULL AND message_id IS NULL))),
  -- A saved message is always visible to the member; only uploads are staff-only.
  ADD CONSTRAINT evidence_visibility CHECK (kind = 'upload' OR visible_to_member),
  ADD CONSTRAINT evidence_removed CHECK ((removed_at IS NULL) = (removed_by IS NULL)),
  ADD CONSTRAINT evidence_note_editor CHECK (note_updated_at IS NOT NULL OR note_updated_by IS NULL),
  ADD CONSTRAINT evidence_no_upload_surrounding CHECK (kind = 'message' OR surrounding IS NULL);

-- Interaction attachments have Discord ids and short-lived CDN URLs; a
-- dashboard upload has neither: its bytes wait in `evidence_upload_bytes`.
ALTER TABLE evidence_files
  ALTER COLUMN original_url DROP NOT NULL,
  ADD CONSTRAINT evidence_files_guild_id UNIQUE (guild_id, id);

-- Finds Quack's own copy posts, in whichever channel they are, so they are
-- never saved as evidence (§7.1).
CREATE INDEX evidence_files_copy_message ON evidence_files (guild_id, copy_message_id)
  WHERE copy_message_id IS NOT NULL;

-- Bytes of a dashboard upload until the capture job copies the file to the
-- evidence channel (§7.3). Deleted when the file stops being pending, and by
-- housekeeping after 24 hours: Quack is never the host of evidence.
CREATE TABLE evidence_upload_bytes (
  file_id     uuid PRIMARY KEY,
  guild_id    bigint NOT NULL,
  bytes       bytea NOT NULL CHECK (octet_length(bytes) <= 10485760),
  created_at  timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (guild_id, file_id) REFERENCES evidence_files (guild_id, id)
);
CREATE INDEX evidence_upload_bytes_expiry ON evidence_upload_bytes (created_at);

-- The `/case add` file option, kept across the draft's pages (§11.6):
-- `{attachment_id, filename, content_type, size_bytes, url}`.
ALTER TABLE case_drafts
  ADD COLUMN evidence_file jsonb NULL CHECK (jsonb_typeof(evidence_file) = 'object');
