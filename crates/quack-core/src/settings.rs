//! Validation of server and module settings values (§13.1, §13.2, §13.4)
//! that needs no Discord facts.

use crate::ids::{GuildId, RoleId};
use crate::limits::{self, CharLimit};

str_enum! {
    /// The machine code of one settings problem.
    pub enum SettingsProblem {
        /// More than 25 roles in a staff role list.
        TooManyRoles = "too_many_roles",
        /// A role appears twice in a staff role list.
        DuplicateRole = "duplicate_role",
        /// @everyone can't be a staff role.
        EveryoneRole = "everyone_role",
        /// A role the server doesn't have (and that wasn't already saved).
        UnknownRole = "unknown_role",
        /// Not a `discord.gg`, `discord.com/invite`, or
        /// `discordapp.com/invite` link with a valid code.
        InviteUrl = "invite_url",
        /// Text is longer than the setting allows.
        TooLong = "too_long",
        /// Ticket transcript retention is not 1–365 days.
        RetentionDays = "retention_days",
    }
}

/// Checks a staff role list (§13.1.1): at most 25 distinct ids, never
/// @everyone, each a current role of the server or already saved.
pub fn check_role_list(
    guild: GuildId,
    roles: &[RoleId],
    server_roles: &[RoleId],
    saved: &[RoleId],
) -> Result<(), SettingsProblem> {
    if roles.len() > limits::MAX_STAFF_ROLES {
        return Err(SettingsProblem::TooManyRoles);
    }
    for (i, role) in roles.iter().enumerate() {
        if roles[..i].contains(role) {
            return Err(SettingsProblem::DuplicateRole);
        }
        if *role == guild.everyone_role() {
            return Err(SettingsProblem::EveryoneRole);
        }
        if !server_roles.contains(role) && !saved.contains(role) {
            return Err(SettingsProblem::UnknownRole);
        }
    }
    Ok(())
}

/// Normalizes a rejoin invite (§13.1.1): `discord.gg/<code>`,
/// `discord.com/invite/<code>`, or `discordapp.com/invite/<code>`, with or
/// without `https://` and `www.`, and a code of 2–64 characters of
/// `[A-Za-z0-9_-]`. Stored as `https://discord.gg/<code>`. Empty clears it.
pub fn normalize_invite(url: &str) -> Result<Option<String>, SettingsProblem> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(None);
    }
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let code = [
        "discord.gg/",
        "discord.com/invite/",
        "discordapp.com/invite/",
    ]
    .iter()
    .find_map(|prefix| rest.strip_prefix(prefix))
    .ok_or(SettingsProblem::InviteUrl)?;
    let valid = (limits::INVITE_CODE.min..=limits::INVITE_CODE.max).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(Some(format!("https://discord.gg/{code}")))
    } else {
        Err(SettingsProblem::InviteUrl)
    }
}

/// Normalizes optional text such as the DM introduction or footer or the
/// honeypot warning: trimmed, empty stores nothing.
pub fn normalize_optional_text(
    text: &str,
    limit: CharLimit,
) -> Result<Option<String>, SettingsProblem> {
    let text = text.trim();
    if !limit.allows(text) {
        Err(SettingsProblem::TooLong)
    } else if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text.to_owned()))
    }
}

/// Checks a ticket transcript retention in days.
pub fn check_retention_days(days: u16) -> Result<(), SettingsProblem> {
    if (limits::MIN_TICKET_RETENTION_DAYS..=limits::MAX_TICKET_RETENTION_DAYS).contains(&days) {
        Ok(())
    } else {
        Err(SettingsProblem::RetentionDays)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: u64) -> RoleId {
        RoleId::new(n).unwrap()
    }

    #[test]
    fn role_lists() {
        let g = GuildId::new(1).unwrap();
        let server = [r(2), r(3)];
        assert_eq!(check_role_list(g, &[r(2), r(3)], &server, &[]), Ok(()));
        assert_eq!(check_role_list(g, &[r(9)], &server, &[r(9)]), Ok(()));
        assert_eq!(
            check_role_list(g, &[r(9)], &server, &[]),
            Err(SettingsProblem::UnknownRole)
        );
        assert_eq!(
            check_role_list(g, &[r(2), r(2)], &server, &[]),
            Err(SettingsProblem::DuplicateRole)
        );
        assert_eq!(
            check_role_list(g, &[r(1)], &[r(1)], &[]),
            Err(SettingsProblem::EveryoneRole)
        );
        let many: Vec<RoleId> = (2..=27).map(r).collect();
        assert_eq!(
            check_role_list(g, &many, &many, &[]),
            Err(SettingsProblem::TooManyRoles)
        );
    }

    #[test]
    fn invites() {
        for (input, code) in [
            ("https://discord.gg/abc-1", "abc-1"),
            (" https://discord.com/invite/Zz ", "Zz"),
            ("discord.gg/a_b", "a_b"),
            ("www.discord.gg/abc", "abc"),
            ("https://www.discord.com/invite/abc", "abc"),
            ("discordapp.com/invite/abc", "abc"),
            ("https://www.discordapp.com/invite/abc", "abc"),
        ] {
            assert_eq!(
                normalize_invite(input),
                Ok(Some(format!("https://discord.gg/{code}"))),
                "{input}"
            );
        }
        assert_eq!(normalize_invite(""), Ok(None));
        let longest = "a".repeat(64);
        assert_eq!(
            normalize_invite(&format!("discord.gg/{longest}")),
            Ok(Some(format!("https://discord.gg/{longest}")))
        );
        for bad in [
            "http://discord.gg/abc".to_owned(),
            "https://discord.gg/a".to_owned(),
            "https://discord.gg/a.b".to_owned(),
            "https://discord.gg/a b".to_owned(),
            "https://discord.gg/".to_owned(),
            "https://discord.gg/abc/def".to_owned(),
            "https://evil.com/discord.gg/abc".to_owned(),
            "https://discord.com/abc".to_owned(),
            format!("https://discord.gg/{}", "a".repeat(65)),
        ] {
            assert_eq!(
                normalize_invite(&bad),
                Err(SettingsProblem::InviteUrl),
                "{bad}"
            );
        }
    }

    #[test]
    fn optional_text_and_retention() {
        assert_eq!(normalize_optional_text("  ", limits::DM_INTRO), Ok(None));
        assert_eq!(
            normalize_optional_text(" hi ", limits::DM_FOOTER),
            Ok(Some("hi".into()))
        );
        assert_eq!(
            normalize_optional_text(&"x".repeat(501), limits::DM_FOOTER),
            Err(SettingsProblem::TooLong)
        );
        assert!(normalize_optional_text(&"x".repeat(1500), limits::HONEYPOT_WARNING).is_ok());
        assert_eq!(check_retention_days(1), Ok(()));
        assert_eq!(check_retention_days(365), Ok(()));
        assert_eq!(check_retention_days(0), Err(SettingsProblem::RetentionDays));
        assert_eq!(
            check_retention_days(366),
            Err(SettingsProblem::RetentionDays)
        );
    }
}
