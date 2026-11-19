-- Versions 4 (staff_roles) and 5 (discord_user_mfa) as the current Go code
-- applies them, plus the obsolete ticket column a fresh v5 database lacks.
ALTER TABLE guild_settings ADD COLUMN moderator_role_ids varchar(1024) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT '', ADD COLUMN rules_manager_role_ids varchar(1024) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT '';
CREATE TABLE `discord_user_mfa` (
  `discord_user_id` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(3) NOT NULL,
  `updated_at` datetime(3) NOT NULL,
  `mfa_enabled` tinyint(1) NOT NULL,
  `mfa_checked_at` datetime(3) NOT NULL,
  PRIMARY KEY (`discord_user_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
ALTER TABLE tickets DROP COLUMN queue_delivery_attempt_id;
INSERT INTO quack_schema_migrations (version, name, applied_at) VALUES (4, 'staff_roles', '2025-11-01 00:00:00.000');
INSERT INTO quack_schema_migrations (version, name, applied_at) VALUES (5, 'discord_user_mfa', '2025-11-02 00:00:00.000');
UPDATE guild_settings SET moderator_role_ids = '600000000000000001,600000000000000001, bad,600000000000000002', rules_manager_role_ids = '' WHERE id = '01JAGS00010000000000000000';
INSERT INTO discord_user_mfa VALUES ('300000000000000002', '2026-01-01 00:00:00.000', '2026-01-02 00:00:00.000', 1, '2026-01-02 00:00:00.000');
INSERT INTO discord_user_mfa VALUES ('300000000000000003', '2026-01-01 00:00:00.000', '2026-01-03 00:00:00.000', 0, '2026-01-03 00:00:00.000');
INSERT INTO discord_user_mfa VALUES ('quack-bad', '2026-01-01 00:00:00.000', '2026-01-03 00:00:00.000', 0, '2026-01-03 00:00:00.000');
