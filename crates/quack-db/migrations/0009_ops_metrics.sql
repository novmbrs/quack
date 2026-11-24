-- Ops metrics (design §21.6, §21.8): each server's member count, kept from
-- the gateway, and the indexes the fleet-wide windows read.

-- Members as of the last GUILD_CREATE, moved by member joins and leaves;
-- NULL until Quack has seen the server since this column existed.
ALTER TABLE guilds ADD COLUMN member_count integer NULL CHECK (member_count >= 0);

-- Cases and installs over time across every server (the 24-hour, 7-day,
-- and 30-day windows of the status board and `/ops metrics`).
CREATE INDEX cases_created_all ON cases (created_at);
CREATE INDEX mod_log_installs ON mod_log (created_at, kind)
  WHERE kind IN ('quack_joined', 'quack_left');
