-- The public status board (design §21.6): its message, like the ops
-- board's, so an edit finds it after a restart.
ALTER TABLE ops_state
  ADD COLUMN public_channel_id bigint NULL,
  ADD COLUMN public_message_id bigint NULL;
