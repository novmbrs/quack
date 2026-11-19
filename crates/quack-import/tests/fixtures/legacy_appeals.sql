-- A database migrated before Go retired custom appeal forms: `content`
-- instead of `statement`, the form columns, and the form settings table.
ALTER TABLE appeals RENAME COLUMN statement TO content, ADD COLUMN question_snapshot_json json NULL, ADD COLUMN answers_json json NULL;
UPDATE appeals SET content = '', answers_json = '[{"value": " first answer "}, {"value": ""}, {"value": "second answer"}]' WHERE id = '01JAAP00020000000000000000';
CREATE TABLE `guild_appeal_settings` (
  `id` char(26) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(3) NOT NULL,
  `updated_at` datetime(3) NOT NULL,
  `guild_id` char(26) COLLATE utf8mb4_unicode_ci NOT NULL,
  `questions_json` json NOT NULL,
  `updated_by_discord_user_id` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
INSERT INTO guild_appeal_settings VALUES ('01JAGA00010000000000000000', '2025-01-01 00:00:00.000', '2025-01-01 00:00:00.000', '01JAGD00010000000000000000', '[]', '300000000000000001');
