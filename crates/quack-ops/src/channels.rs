//! The ops channels (§21.2): a category named `Ops` with five private text
//! channels, found first and created when missing.

use quack_core::ids::{ChannelId, MemberId, RoleId};
use quack_core::permissions::{
    ChannelFacts, Overwrite, OverwriteTarget, Permission, Permissions, TEXT_CHANNEL,
};
use quack_db::ops::Feed;
use quack_engine::port::{ChannelSpec, Outcome};

use crate::Ops;

/// Discord's category channel type.
const CATEGORY: u8 = 4;
/// The category's name (matched ignoring case).
const CATEGORY_NAME: &str = "Ops";

/// The ops channels; `None` for one Quack won't post in (an adopted
/// channel that isn't private).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channels {
    /// `#ops-status`.
    pub status: Option<ChannelId>,
    /// `#ops-deploys`.
    pub deploys: Option<ChannelId>,
    /// `#ops-installs`.
    pub installs: Option<ChannelId>,
    /// `#ops-alerts`.
    pub alerts: Option<ChannelId>,
    /// `#ops-errors`.
    pub errors: Option<ChannelId>,
}

impl Channels {
    /// The channel of a durable post's feed.
    pub const fn feed(&self, feed: Feed) -> Option<ChannelId> {
        match feed {
            Feed::Deploys => self.deploys,
            Feed::Installs => self.installs,
            Feed::Alerts => self.alerts,
        }
    }
}

/// Which channel, by name and topic.
#[derive(Clone, Copy)]
enum Slot {
    Status,
    Deploys,
    Installs,
    Alerts,
    Errors,
}

impl Slot {
    const ALL: [Self; 5] = [
        Self::Status,
        Self::Deploys,
        Self::Installs,
        Self::Alerts,
        Self::Errors,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Status => "ops-status",
            Self::Deploys => "ops-deploys",
            Self::Installs => "ops-installs",
            Self::Alerts => "ops-alerts",
            Self::Errors => "ops-errors",
        }
    }

    const fn topic(self) -> &'static str {
        match self {
            Self::Status => "Quack's status board, updated every 10 minutes.",
            Self::Deploys => "When Quack starts, and trouble with Discord's gateway.",
            Self::Installs => "When Quack joins or leaves a server.",
            Self::Alerts => "Dead jobs and failed actions. Grafana posts its alerts here too.",
            Self::Errors => "Errors from Quack's logs, once a minute.",
        }
    }

    fn set(self, channels: &mut Channels, id: Option<ChannelId>) {
        match self {
            Self::Status => channels.status = id,
            Self::Deploys => channels.deploys = id,
            Self::Installs => channels.installs = id,
            Self::Alerts => channels.alerts = id,
            Self::Errors => channels.errors = id,
        }
    }
}

/// Why the channels aren't ready yet; logged once per start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NotReady {
    /// Quack isn't in the ops server.
    NotInServer,
    /// Discord refused a read or a create (a missing permission).
    Refused(String),
    /// Discord didn't answer.
    Unavailable(String),
}

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInServer => f.write_str("Quack isn't in the ops server"),
            Self::Refused(code) => write!(f, "Discord refused ({code})"),
            Self::Unavailable(code) => write!(f, "Discord didn't answer ({code})"),
        }
    }
}

fn failed<T>(outcome: &Outcome<T>) -> NotReady {
    match outcome {
        Outcome::Refused(_) => NotReady::Refused(outcome.code()),
        _ => NotReady::Unavailable(outcome.code()),
    }
}

/// The overwrites of a created ops channel or category.
fn overwrites(ops: &Ops, quack: MemberId) -> Vec<Overwrite> {
    let everyone = RoleId::new(ops.guild().get());
    let mut list = Vec::new();
    if let Some(everyone) = everyone {
        list.push(Overwrite {
            target: OverwriteTarget::Role(everyone),
            allow: Permissions::NONE,
            deny: Permission::ViewChannel.into(),
        });
    }
    list.push(Overwrite {
        target: OverwriteTarget::Member(quack),
        allow: Permission::ViewChannel
            | Permission::SendMessages
            | Permission::ReadMessageHistory
            | Permission::EmbedLinks,
        deny: Permissions::NONE,
    });
    for operator in &ops.config().operators {
        if let Some(member) = MemberId::new(operator.get()) {
            list.push(Overwrite {
                target: OverwriteTarget::Member(member),
                allow: Permission::ViewChannel
                    | Permission::SendMessages
                    | Permission::ReadMessageHistory,
                deny: Permissions::NONE,
            });
        }
    }
    list
}

/// Whether a channel's own overwrites hide it from @everyone.
fn is_private(ops: &Ops, channel: &ChannelFacts) -> bool {
    let Some(everyone) = RoleId::new(ops.guild().get()) else {
        return false;
    };
    channel.overwrites.iter().any(|o| {
        o.target == OverwriteTarget::Role(everyone)
            && o.deny.contains(Permission::ViewChannel.into())
    })
}

/// Finds or creates the category and its channels.
pub(crate) async fn ensure(ops: &Ops) -> Result<Channels, NotReady> {
    let guild = ops.guild();
    let discord = ops.engine().discord();
    let quack = match discord.guild_facts(guild).await {
        Outcome::Done(facts) => facts.quack.ok_or(NotReady::NotInServer)?.id,
        Outcome::Refused(_) => return Err(NotReady::NotInServer),
        other => return Err(failed(&other)),
    };
    let existing = match discord.guild_channels(guild).await {
        Outcome::Done(list) => list,
        other => return Err(failed(&other)),
    };
    let overwrites = overwrites(ops, quack);
    let found = existing
        .iter()
        .find(|c| c.kind == CATEGORY && c.name.eq_ignore_ascii_case(CATEGORY_NAME));
    let category = if let Some(c) = found {
        c.id
    } else {
        let spec = ChannelSpec {
            name: CATEGORY_NAME.to_owned(),
            topic: None,
            overwrites: overwrites.clone(),
            parent: None,
            category: true,
        };
        match discord.create_channel(guild, &spec).await {
            Outcome::Done(id) => id,
            other => return Err(failed(&other)),
        }
    };
    let mut channels = Channels::default();
    for slot in Slot::ALL {
        let found = existing.iter().find(|c| {
            c.kind == TEXT_CHANNEL && c.name == slot.name() && c.parent_id == Some(category)
        });
        let id = match found {
            Some(c) if is_private(ops, c) => Some(c.id),
            Some(c) => {
                tracing::warn!(
                    channel_id = c.id.get(),
                    channel = slot.name(),
                    "an ops channel is visible to everyone; Quack won't post there"
                );
                None
            }
            None => {
                let spec = ChannelSpec {
                    name: slot.name().to_owned(),
                    topic: Some(slot.topic().to_owned()),
                    overwrites: overwrites.clone(),
                    parent: Some(category),
                    category: false,
                };
                match discord.create_channel(guild, &spec).await {
                    Outcome::Done(id) => Some(id),
                    other => return Err(failed(&other)),
                }
            }
        };
        slot.set(&mut channels, id);
    }
    Ok(channels)
}
