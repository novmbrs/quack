//! Discord message links (§4.6 `message_link`, §7.1).

use std::fmt;

use crate::ids::{ChannelId, GuildId, MessageId};
use crate::limits::CONTEXT_MESSAGE_LINK_MAX_CHARS;

const HOSTS: [&str; 8] = [
    "discord.com",
    "ptb.discord.com",
    "canary.discord.com",
    "www.discord.com",
    "discordapp.com",
    "ptb.discordapp.com",
    "canary.discordapp.com",
    "www.discordapp.com",
];

/// A link to one message in a server, stored in canonical form
/// `https://discord.com/channels/<guild>/<channel>/<message>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MessageLink {
    /// The server.
    pub guild_id: GuildId,
    /// The channel or thread.
    pub channel_id: ChannelId,
    /// The message.
    pub message_id: MessageId,
}

/// Why text is not an acceptable message link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LinkProblem {
    /// Not a Discord message link (or longer than 200 characters).
    #[error("not a message link")]
    NotAMessageLink,
    /// A message link into another server.
    #[error("message link into another server")]
    OtherServer,
}

impl MessageLink {
    /// Parses a message link: `https://` on `discord.com` or `discordapp.com`
    /// (also `ptb.`, `canary.`, `www.`), path `/channels/<g>/<c>/<m>` with
    /// ids of 17–20 digits, at most 200 characters. A query or fragment is
    /// dropped.
    pub fn parse(text: &str) -> Result<Self, LinkProblem> {
        let text = text.trim();
        let bad = LinkProblem::NotAMessageLink;
        if text.chars().count() > CONTEXT_MESSAGE_LINK_MAX_CHARS {
            return Err(bad);
        }
        let rest = text.strip_prefix("https://").ok_or(bad)?;
        let rest = rest.split(['?', '#']).next().unwrap_or_default();
        let (host, path) = rest.split_once('/').ok_or(bad)?;
        if !HOSTS.contains(&host.to_ascii_lowercase().as_str()) {
            return Err(bad);
        }
        let mut parts = path.split('/');
        if parts.next() != Some("channels") {
            return Err(bad);
        }
        let mut id = || {
            parts
                .next()
                .filter(|p| (17..=20).contains(&p.len()))
                .ok_or(bad)
        };
        let (guild, channel, message) = (id()?, id()?, id()?);
        if parts.next().is_some() {
            return Err(bad);
        }
        Ok(Self {
            guild_id: guild.parse().map_err(|_| bad)?,
            channel_id: channel.parse().map_err(|_| bad)?,
            message_id: message.parse().map_err(|_| bad)?,
        })
    }

    /// Parses a link that must point into `guild`.
    pub fn parse_in(text: &str, guild: GuildId) -> Result<Self, LinkProblem> {
        let link = Self::parse(text)?;
        if link.guild_id == guild {
            Ok(link)
        } else {
            Err(LinkProblem::OtherServer)
        }
    }

    /// The canonical URL.
    pub fn to_url(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for MessageLink {
    /// Writes the canonical URL.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "https://discord.com/channels/{}/{}/{}",
            self.guild_id, self.channel_id, self.message_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "123456789012345678";
    const C: &str = "223456789012345678";
    const M: &str = "323456789012345678";

    fn guild() -> GuildId {
        G.parse().unwrap()
    }

    #[test]
    fn accepts_every_host_and_canonicalizes() {
        let canonical = format!("https://discord.com/channels/{G}/{C}/{M}");
        for host in HOSTS {
            let link = MessageLink::parse(&format!("  https://{host}/channels/{G}/{C}/{M}?x=1#y "))
                .unwrap();
            assert_eq!(link.to_url(), canonical, "{host}");
        }
        assert_eq!(
            MessageLink::parse(&format!("https://DISCORD.com/channels/{G}/{C}/{M}"))
                .map(|l| l.guild_id),
            Ok(guild())
        );
    }

    #[test]
    fn rejects_everything_else() {
        let long_query = format!(
            "https://discord.com/channels/{G}/{C}/{M}?{}",
            "x".repeat(200)
        );
        for bad in [
            String::new(),
            format!("http://discord.com/channels/{G}/{C}/{M}"),
            format!("https://discord.gg/channels/{G}/{C}/{M}"),
            format!("https://evil.com/channels/{G}/{C}/{M}"),
            format!("https://discord.com.evil.com/channels/{G}/{C}/{M}"),
            format!("https://discord.com/channels/@me/{C}/{M}"),
            format!("https://discord.com/channels/{G}/{C}"),
            format!("https://discord.com/channels/{G}/{C}/{M}/"),
            format!("https://discord.com/channels/{G}/{C}/{M}/1"),
            format!("https://discord.com/messages/{G}/{C}/{M}"),
            format!("https://discord.com/channels/{G}/{C}/1234567890123456"),
            format!("https://discord.com/channels/{G}/{C}/123456789012345678901"),
            format!("https://discord.com/channels/{G}/{C}/99999999999999999999"),
            format!("https://discord.com/channels/{G}/{C}/-23456789012345678"),
            long_query,
        ] {
            assert_eq!(
                MessageLink::parse(&bad),
                Err(LinkProblem::NotAMessageLink),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn other_server() {
        let link = format!("https://discord.com/channels/{G}/{C}/{M}");
        assert!(MessageLink::parse_in(&link, guild()).is_ok());
        assert_eq!(
            MessageLink::parse_in(&link, GuildId::new(5).unwrap()),
            Err(LinkProblem::OtherServer)
        );
    }
}
