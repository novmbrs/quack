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
        /// Not a `discord.gg` or `discord.com/invite` link.
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

/// Normalizes a rejoin invite: `https://discord.gg/<code>` or
/// `https://discord.com/invite/<code>` with a code of 2–32 characters of
/// `[A-Za-z0-9-]`, stored as `https://discord.gg/<code>`. Empty clears it.
pub fn normalize_invite(url: &str) -> Result<Option<String>, SettingsProblem> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(None);
    }
    let code = url
        .strip_prefix("https://discord.gg/")
        .or_else(|| url.strip_prefix("https://discord.com/invite/"))
        .ok_or(SettingsProblem::InviteUrl)?;
    let valid = limits::INVITE_CODE.allows(code)
        && code.len() == code.trim().len()
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
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
        assert_eq!(
            normalize_invite("https://discord.gg/abc-1"),
            Ok(Some("https://discord.gg/abc-1".into()))
        );
        assert_eq!(
            normalize_invite(" https://discord.com/invite/Zz "),
            Ok(Some("https://discord.gg/Zz".into()))
        );
        assert_eq!(normalize_invite(""), Ok(None));
        for bad in [
            "http://discord.gg/abc",
            "https://discord.gg/a",
            "https://discord.gg/a_b",
            "https://discord.gg/",
            "discord.gg/abc",
        ] {
            assert_eq!(
                normalize_invite(bad),
                Err(SettingsProblem::InviteUrl),
                "{bad}"
            );
        }
        assert_eq!(
            normalize_invite(&format!("https://discord.gg/{}", "a".repeat(33))),
            Err(SettingsProblem::InviteUrl)
        );
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
