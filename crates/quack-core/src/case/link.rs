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

/// Returns the Discord message links inside free text (§4.6), in order,
/// without repeats: every `https://` URL on a message-link host, cut at
/// whitespace, `<>()[]"'|` and backticks, with trailing punctuation trimmed.
/// Links into other servers are included; the caller keeps only those that
/// point into the case's server. The text itself is never changed.
pub fn find_message_links(text: &str) -> Vec<MessageLink> {
    let mut found: Vec<MessageLink> = Vec::new();
    let mut rest = text;
    while let Some(at) = find_ascii_case_insensitive(rest, "https://") {
        let candidate = &rest[at..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || "<>()[]\"'|`".contains(c))
            .unwrap_or(candidate.len());
        let url = candidate[..end].trim_end_matches(|c: char| ".,;:!?*_~".contains(c));
        if let Ok(link) = MessageLink::parse(url)
            && !found.contains(&link)
        {
            found.push(link);
        }
        rest = &candidate["https://".len()..];
    }
    found
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

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
    /// Not a Discord message link (or longer than 4,000 characters).
    #[error("not a message link")]
    NotAMessageLink,
    /// A message link into another server.
    #[error("message link into another server")]
    OtherServer,
    /// A message in the evidence channel, or one of Quack's copies there:
    /// never an evidence source (§7.1).
    #[error("message in the evidence channel")]
    EvidenceChannel,
    /// A message the actor can't read now, whose item adding it again would
    /// restore or capture again (§7.1).
    #[error("message the actor can't read")]
    NoAccess,
}

impl MessageLink {
    /// Parses a message link: `https://` on `discord.com` or `discordapp.com`
    /// (also `ptb.`, `canary.`, `www.`), path `/channels/<g>/<c>/<m>` where
    /// each id is a positive decimal below 2^63, input at most 4,000
    /// characters. A query or fragment is dropped.
    pub fn parse(text: &str) -> Result<Self, LinkProblem> {
        let text = text.trim();
        let bad = LinkProblem::NotAMessageLink;
        if text.chars().count() > CONTEXT_MESSAGE_LINK_MAX_CHARS {
            return Err(bad);
        }
        let rest = text
            .get(..8)
            .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
            .map(|_| &text[8..])
            .ok_or(bad)?;
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
                .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
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
            "x".repeat(4_000)
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
            format!("https://discord.com/channels/{G}/{C}/123456789012345678901"),
            format!("https://discord.com/channels/{G}/{C}/99999999999999999999"),
            format!("https://discord.com/channels/{G}/{C}/9223372036854775808"),
            format!("https://discord.com/channels/{G}/{C}/0"),
            format!("https://discord.com/channels/{G}/{C}/-23456789012345678"),
            format!("https://discord.com/channels/{G}/{C}/+23456789012345678"),
            format!("https://discord.com/channels/{G}//{M}"),
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
    fn ids_are_any_positive_decimal_below_2_pow_63() {
        let link =
            MessageLink::parse("https://discord.com/channels/1/2/9223372036854775807").unwrap();
        assert_eq!(link.message_id.get(), 9_223_372_036_854_775_807);
        assert_eq!(
            link.to_url(),
            "https://discord.com/channels/1/2/9223372036854775807"
        );
        let at_limit = format!("https://discord.com/channels/{G}/{C}/{M}?");
        let at_limit = format!("{at_limit}{}", "x".repeat(4_000 - at_limit.len()));
        assert!(MessageLink::parse(&at_limit).is_ok());
        assert!(MessageLink::parse(&format!("HTTPS://discord.com/channels/{G}/{C}/{M}")).is_ok());
    }

    #[test]
    fn links_inside_text() {
        let a = format!("https://discord.com/channels/{G}/{C}/{M}");
        let b = "https://ptb.discordapp.com/channels/1/2/3";
        let text = format!(
            "See {a}. Also <{b}>, ({a}) and [here]({b}?x=1)! ||https://canary.discord.com/channels/{G}/{C}/4||, \
             not https://discord.com/channels/{G}/{C} or https://evil.com/channels/1/2/3 or http://discord.com/channels/1/2/5"
        );
        let found: Vec<String> = find_message_links(&text)
            .iter()
            .map(MessageLink::to_url)
            .collect();
        assert_eq!(
            found,
            [
                a.clone(),
                "https://discord.com/channels/1/2/3".to_owned(),
                format!("https://discord.com/channels/{G}/{C}/4"),
            ]
        );
        assert_eq!(find_message_links(""), vec![]);
        assert_eq!(find_message_links("https://"), vec![]);
        assert_eq!(find_message_links(&format!("{a}...")).len(), 1);
        assert_eq!(find_message_links(&format!("é{a}é")).len(), 0);
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
