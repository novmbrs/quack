-- Ops in Discord (design §21.3): durable posts to the ops server's
-- channels, and one row about the current run, the status board, and the
-- alert watermarks.

ALTER TYPE job_kind ADD VALUE 'post_ops';

-- A post waiting for its `post_ops:<id>` job; deleted once posted or
-- dropped.
CREATE TABLE ops_posts (
  id         uuid PRIMARY KEY DEFAULT uuidv7(),
  feed       text NOT NULL CHECK (feed IN ('deploys', 'installs', 'alerts')),
  body       jsonb NOT NULL CHECK (jsonb_typeof(body) = 'object'),
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ops_state (
  id                         boolean PRIMARY KEY DEFAULT true CHECK (id),
  version                    text NULL CHECK (char_length(version) <= 100),
  started_at                 timestamptz NULL,
  seen_at                    timestamptz NULL,       -- the last heartbeat
  stopped_at                 timestamptz NULL,       -- set by a clean stop
  stop_reason                text NULL CHECK (stop_reason IN ('signal', 'gateway_closed')),
  status_channel_id          bigint NULL,
  status_message_id          bigint NULL,
  dead_jobs_seen_until       timestamptz NOT NULL DEFAULT now(),
  failed_actions_seen_until  timestamptz NOT NULL DEFAULT now(),
  CHECK ((stopped_at IS NULL) = (stop_reason IS NULL))
);

INSERT INTO ops_state DEFAULT VALUES;
