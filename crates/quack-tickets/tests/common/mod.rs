//! Shared set-up for the tickets scenario tests: a server with an entry
//! channel, a staff-only queue channel, and tickets turned on.

#![allow(dead_code, unreachable_pub, missing_docs)]

use anyhow::{Context as _, Result, bail};
use quack_core::ids::{ChannelId, MemberId, TicketId};
use quack_core::modlog::ModLogKind;
use quack_core::permissions::{Permission, Permissions};
use quack_db::modlog::{self, ModLogFilter};
use quack_db::tickets::{self, Ticket};
use quack_engine::settings::set_enabled;
use quack_engine::{Origin, Principal};
use quack_testkit::{ADMIN, Harness, MEMBER, sf};
use quack_tickets::{JournalEntry, OpenAnswer, TicketSettingsInput};
use time::OffsetDateTime;

pub struct World {
    pub h: Harness,
    pub entry: ChannelId,
    pub queue: ChannelId,
    pub journal: quack_tickets::Journal,
}

pub fn entry_permissions() -> Permissions {
    [
        Permission::ViewChannel,
        Permission::SendMessages,
        Permission::ReadMessageHistory,
        Permission::CreatePrivateThreads,
        Permission::SendMessagesInThreads,
        Permission::ManageThreads,
    ]
    .into_iter()
    .collect()
}

pub fn admin(h: &Harness) -> Principal {
    h.principal(ADMIN, Origin::Dashboard)
}

/// A server with the two channels saved but tickets still off.
pub async fn configured() -> Result<World> {
    let h = Harness::new().await?;
    h.register(quack_tickets::jobs(h.engine.clone()));
    let (entry, queue) = h.fake.world(|w| {
        let entry = w.add_text_channel(h.guild, false);
        let queue = w.add_text_channel(h.guild, true);
        if let Some(c) = w
            .guilds
            .get_mut(&h.guild)
            .and_then(|g| g.channels.get_mut(&entry))
        {
            c.quack_permissions = entry_permissions();
        }
        (entry, queue)
    });
    let current = quack_tickets::settings(&h.engine, &admin(&h)).await?;
    quack_tickets::update_settings(
        &h.engine,
        &admin(&h),
        &TicketSettingsInput {
            expected_version: current.version,
            entry_channel_id: Some(entry),
            queue_channel_id: Some(queue),
            transcript_retention_days: 30,
        },
        &h.key("PUT settings"),
    )
    .await?;
    Ok(World {
        h,
        entry,
        queue,
        journal: quack_tickets::Journal::new(),
    })
}

/// A server with tickets on.
pub async fn world() -> Result<World> {
    let w = configured().await?;
    set_enabled(
        &w.h.engine,
        &admin(&w.h),
        &quack_tickets::hooks(),
        true,
        &w.h.key("enable"),
    )
    .await?;
    w.h.settle().await?;
    Ok(w)
}

impl World {
    pub async fn open(&self, member: u64) -> Result<OpenAnswer> {
        Ok(quack_tickets::open(&self.h.engine, self.h.guild, sf(member)).await?)
    }

    /// Opens a ticket for `member` and runs the queue: returns it, open.
    pub async fn opened(&self, member: u64) -> Result<Ticket> {
        let OpenAnswer::Opening { ticket, .. } = self.open(member).await? else {
            bail!("expected the ticket to open");
        };
        self.h.settle().await?;
        self.ticket(ticket).await
    }

    pub async fn ticket(&self, id: TicketId) -> Result<Ticket> {
        let mut conn = self.h.db.db.conn().await?;
        tickets::get(&mut conn, self.h.guild, id)
            .await?
            .context("ticket exists")
    }

    pub async fn live(&self, member: u64) -> Result<Option<Ticket>> {
        let mut conn = self.h.db.db.conn().await?;
        Ok(tickets::live_for_member(&mut conn, self.h.guild, sf::<MemberId>(member)).await?)
    }

    /// `author` posts `text` in the thread: on Discord, and through the
    /// gateway into the journal.
    pub async fn say(&self, t: &Ticket, author: u64, text: &str) -> Result<()> {
        let thread = t.thread.context("thread")?;
        let id = self
            .h
            .fake
            .world(|w| w.post(self.h.guild, thread, sf(author), text, Vec::new()));
        self.journal
            .on_message(&self.h.engine, &self.entry_for(thread, id, author, text))
            .await;
        Ok(())
    }

    pub fn entry_for(
        &self,
        thread: ChannelId,
        message: quack_core::ids::MessageId,
        author: u64,
        text: &str,
    ) -> JournalEntry {
        JournalEntry {
            guild: self.h.guild,
            channel: thread,
            parent: Some(self.entry),
            message,
            author: sf(author),
            author_name: format!("user{author}"),
            text: text.to_owned(),
            files: Vec::new(),
            sent_at: OffsetDateTime::now_utc(),
        }
    }

    pub fn member_principal(&self, member: u64) -> Principal {
        self.h.principal(member, Origin::Discord)
    }

    pub async fn close_as(&self, t: &Ticket, who: u64) -> Result<()> {
        quack_tickets::close(&self.h.engine, &self.member_principal(who), t.id).await?;
        Ok(())
    }

    pub async fn transcript(&self, t: &Ticket) -> Result<tickets::Transcript> {
        let mut conn = self.h.db.db.conn().await?;
        tickets::transcript(&mut conn, self.h.guild, t.id)
            .await?
            .context("transcript")
    }

    pub async fn log_kinds(&self) -> Result<Vec<ModLogKind>> {
        let mut conn = self.h.db.db.conn().await?;
        let entries =
            modlog::list(&mut conn, self.h.guild, &ModLogFilter::default(), None, 100).await?;
        Ok(entries.into_iter().rev().map(|e| e.entry.kind()).collect())
    }

    pub fn threads(&self) -> usize {
        self.h.fake.world(|w| {
            w.guilds.get(&self.h.guild).map_or(0, |g| {
                g.channels
                    .values()
                    .filter(|c| c.parent_id == Some(self.entry))
                    .count()
            })
        })
    }
}

pub const M: u64 = MEMBER;

/// A second server where the standard moderator is also a moderator;
/// returns it and that moderator's principal there.
pub async fn second_guild(h: &Harness) -> Result<(quack_core::ids::GuildId, Principal)> {
    use quack_testkit::fake::{GuildSetup, MemberSetup};
    use quack_testkit::{MODERATOR, OWNER, quack_permissions};
    let other: quack_core::ids::GuildId = sf(2002);
    let (facts, member) = h.fake.world(|w| {
        w.add_guild(&GuildSetup {
            id: other,
            owner: sf(OWNER),
            quack_position: 50,
            quack_permissions: quack_permissions(),
            mfa_required: false,
        });
        w.add_member(
            other,
            &MemberSetup::new(sf(MODERATOR), 30).permissions(Permission::ModerateMembers),
        );
        let facts = w.guilds[&other].facts.clone();
        let member = w.member(other, sf(MODERATOR)).cloned();
        (facts, member)
    });
    h.install(other).await?;
    let principal = Principal::from_facts(
        &facts,
        sf(MODERATOR),
        Origin::Dashboard,
        member.as_ref(),
        false,
    );
    Ok((other, principal))
}
