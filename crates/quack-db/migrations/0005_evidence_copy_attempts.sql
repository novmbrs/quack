-- Upload tries of one evidence file, reserved before each upload so the
-- three-try budget of design §7.3 holds across job runs and crashes.
ALTER TABLE evidence_files
  ADD COLUMN copy_attempts smallint NOT NULL DEFAULT 0 CHECK (copy_attempts >= 0);
