//! Permission and role problems (§12.3 `access.*`): the one sentence for
//! each, with Discord's own permission names and the settings page that
//! fixes it. Refusals, action failures, setup, modules, evidence, and the
//! log channel all use these, so the same problem reads the same way
//! everywhere and Quack's role is never confused with the moderator's.

use quack_core::permissions::Permission;

use crate::fmt;
use crate::model::{ChannelRef, Person, Text};

/// Who reads the fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader {
    /// A moderator or member who has to ask an administrator.
    Staff,
    /// Someone setting Quack up, who can change it themselves.
    Administrator,
}

/// `{permission}`: the name Discord's English UI shows for a permission.
pub const fn permission(p: Permission) -> &'static str {
    match p {
        Permission::KickMembers => "Kick Members",
        Permission::BanMembers => "Ban Members",
        Permission::Administrator => "Administrator",
        Permission::ManageChannels => "Manage Channels",
        Permission::ManageGuild => "Manage Server",
        Permission::ViewAuditLog => "View Audit Log",
        Permission::ViewChannel => "View Channel",
        Permission::SendMessages => "Send Messages",
        Permission::ManageMessages => "Manage Messages",
        Permission::EmbedLinks => "Embed Links",
        Permission::AttachFiles => "Attach Files",
        Permission::ReadMessageHistory => "Read Message History",
        Permission::ManageThreads => "Manage Threads",
        Permission::CreatePublicThreads => "Create Public Threads",
        Permission::CreatePrivateThreads => "Create Private Threads",
        Permission::SendMessagesInThreads => "Send Messages in Threads",
        Permission::ModerateMembers => "Timeout Members",
    }
}

/// `{permissions}`: "A", "A and B", or "A, B, and C".
pub fn permissions(perms: &[Permission]) -> String {
    let names: Vec<String> = perms.iter().map(|p| permission(*p).to_owned()).collect();
    fmt::list(&names, "and")
}

const fn it(n: usize) -> &'static str {
    if n == 1 { "it" } else { "them" }
}

/// `access.quack_role_needs`: Quack's role lacks server-wide permissions.
pub fn quack_role_needs(perms: &[Permission], reader: Reader) -> Text {
    let names = permissions(perms);
    match reader {
        Reader::Staff => Text::fixed(format!(
            "Quack needs {names}. Ask an administrator to grant {} to Quack's role in Server Settings → Roles.",
            it(perms.len())
        )),
        Reader::Administrator => Text::fixed(format!(
            "In Server Settings → Roles, give Quack's role {names}."
        )),
    }
}

/// `access.quack_channel_needs`: Quack lacks permissions in one channel.
pub fn quack_channel_needs(channel: &ChannelRef, perms: &[Permission], reader: Reader) -> Text {
    let names = permissions(perms);
    match reader {
        Reader::Staff => Text::fixed(format!("Quack needs {names} in "))
            .channel(channel)
            .s(format!(
                ". Ask an administrator to allow {} for Quack in the channel's Edit Channel → Permissions.",
                it(perms.len())
            )),
        Reader::Administrator => Text::fixed("In ")
            .channel(channel)
            .s(format!(" → Edit Channel → Permissions, allow {names} for Quack.")),
    }
}

/// `access.quack_role_too_low`: the member's highest role is at or above
/// Quack's.
pub fn quack_role_too_low(member: Option<&Person>) -> Text {
    let t = Text::fixed("Quack's highest role must be above ");
    match member {
        Some(m) => t.person(m).s("'s highest role."),
        None => t.s("the member's highest role."),
    }
    .s(" Ask an administrator to move Quack's role up in Server Settings → Roles.")
}

/// `access.your_role_too_low`: the member's highest role is at or above
/// the reader's own.
pub fn your_role_too_low(member: Option<&Person>) -> Text {
    let t = Text::fixed("Your highest role must be above ");
    match member {
        Some(m) => t
            .person(m)
            .s("'s highest role. Ask a moderator whose highest role is above ")
            .person(m)
            .s("'s."),
        None => {
            t.s("the member's highest role. Ask a moderator whose highest role is above theirs.")
        }
    }
}

/// `access.quack_not_in_server`.
pub fn quack_not_in_server() -> Text {
    Text::fixed("Quack is no longer in this server. Ask an administrator to add Quack again.")
}
