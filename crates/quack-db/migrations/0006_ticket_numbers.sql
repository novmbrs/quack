-- Per-server ticket numbers (design §13.2.3, §15.5): a ticket's thread is
-- `ticket-0042` and its transcript `ticket-0042-transcript.txt`, readable
-- and still unique per server. The number is allocated in the transaction
-- that creates the ticket, from a counter whose row lock serializes ticket
-- creation per server, like case numbers.

CREATE TABLE ticket_counters (
  guild_id    bigint PRIMARY KEY REFERENCES guilds (guild_id),
  last_number bigint NOT NULL DEFAULT 0 CHECK (last_number BETWEEN 0 AND 9007199254740991)  -- 2^53 - 1
);

ALTER TABLE tickets
  ADD COLUMN number bigint NULL CHECK (number BETWEEN 1 AND 9007199254740991);

-- Existing tickets are numbered per server in creation order.
UPDATE tickets t SET number = n.number
  FROM (SELECT id, row_number() OVER (PARTITION BY guild_id ORDER BY created_at, id) AS number FROM tickets) n
 WHERE n.id = t.id;

INSERT INTO ticket_counters (guild_id, last_number)
SELECT guild_id, max(number) FROM tickets GROUP BY guild_id;

ALTER TABLE tickets
  ALTER COLUMN number SET NOT NULL,
  ADD CONSTRAINT tickets_guild_number UNIQUE (guild_id, number);

-- A ticket inserted without a number takes the server's next one; one
-- inserted with a number (the importer's, in Go's creation order) moves
-- the counter past it.
CREATE FUNCTION number_ticket() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.number IS NULL THEN
    INSERT INTO ticket_counters AS k (guild_id, last_number) VALUES (NEW.guild_id, 1)
    ON CONFLICT (guild_id) DO UPDATE SET last_number = k.last_number + 1
    RETURNING k.last_number INTO NEW.number;
  ELSE
    INSERT INTO ticket_counters AS k (guild_id, last_number) VALUES (NEW.guild_id, NEW.number)
    ON CONFLICT (guild_id) DO UPDATE SET last_number = GREATEST(k.last_number, NEW.number);
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER tickets_number BEFORE INSERT ON tickets
  FOR EACH ROW EXECUTE FUNCTION number_ticket();
