-- When the log channel last accepted a post, for the "once per outage" rule
-- of design §9.4. Set by the post_mod_log job on Done; never shown, never part
-- of version, no NOTIFY. Imported servers start with NULL.
ALTER TABLE guild_settings ADD COLUMN log_channel_posted_at timestamptz NULL;
