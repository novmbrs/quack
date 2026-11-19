-- Rows imported from the Go database keep what it knew and nothing more
-- (§19): a few facts it didn't always record may be missing, instead of the
-- importer inventing them or refusing the whole server. Rows Quack writes
-- itself still always have them.

-- A sent DM whose channel Go didn't keep (it kept the message ID). Only a
-- DM being sent needs its channel; a sent one needs its message.
ALTER TABLE dms DROP CONSTRAINT dms_check4;
ALTER TABLE dms ADD CONSTRAINT dms_sending_channel
  CHECK (state <> 'sending' OR (channel_id IS NOT NULL AND first_attempt_at IS NOT NULL));
ALTER TABLE dms ADD CONSTRAINT dms_sent_started
  CHECK (state <> 'sent' OR first_attempt_at IS NOT NULL);

-- A failure Go marked dismissed without saying who dismissed it.
ALTER TABLE actions DROP CONSTRAINT actions_check14;
ALTER TABLE actions ADD CONSTRAINT actions_dismissed_by
  CHECK (dismissed_by IS NULL OR dismissed_at IS NOT NULL);

-- A Discord or dashboard case Go saved without its moderator. Quack's own
-- cases always carry an operation key; imported ones have none or a
-- `legacy:` one.
ALTER TABLE cases DROP CONSTRAINT cases_check6;
ALTER TABLE cases ADD CONSTRAINT cases_moderator
  CHECK (source IN ('honeypot', 'imported') OR moderator_id IS NOT NULL
         OR operation_key IS NULL OR operation_key LIKE 'legacy:%');

-- A ticket from a server whose ticket settings Go didn't keep: its entry
-- channel is unknown. Only a ticket still being opened needs it.
ALTER TABLE tickets ALTER COLUMN entry_channel_id DROP NOT NULL;
ALTER TABLE tickets ADD CONSTRAINT tickets_opening_entry
  CHECK (state <> 'opening' OR entry_channel_id IS NOT NULL);
