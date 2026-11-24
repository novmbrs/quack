-- Ops metrics (design §21.6, §21.8): each server's member count, kept from
-- the gateway, and the indexes the fleet-wide windows read.

-- Members as the gateway last knew them (GUILD_CREATE's count, moved by
-- joins and leaves in gateway order), and when the gateway read it: a
-- write older than the stored one is ignored, so events handled out of
-- order can't move the count back. NULL until Quack has seen the server
-- since this column existed.
ALTER TABLE guilds
  ADD COLUMN member_count    integer NULL CHECK (member_count >= 0),
  ADD COLUMN member_count_at timestamptz NULL,
  ADD CHECK ((member_count IS NULL) = (member_count_at IS NULL));

-- Activity over time across every server (the 24-hour, 7-day, and 30-day
-- windows of the status board and `/ops metrics`).
CREATE INDEX cases_created_all   ON cases (created_at);
CREATE INDEX appeals_created_all ON appeals (created_at);
CREATE INDEX tickets_created_all ON tickets (created_at);
CREATE INDEX mod_log_installs    ON mod_log (created_at, kind)
  WHERE kind IN ('quack_joined', 'quack_left');
