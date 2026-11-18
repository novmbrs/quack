//! Shared set-up for the server activity log tests: one staff-only route
//! channel for every event, the module on, and an `ActivityLog` whose
//! queue the test drains by hand.

#![allow(dead_code, unreachable_pub, missing_docs, clippy::unused_self)]

use anyhow::Result;
use quack_activity_log::{
    ActivityLog, AuthorKind, Delivered, Limits, MessageCreated, MessageUpdated,
};
use quack_core::ids::{ChannelId, MessageId};
use quack_engine::Origin;
use quack_engine::settings::set_enabled;
use quack_testkit::{ADMIN, Harness, MEMBER, sf};
use quack_text::model::Person;

pub struct World {
    pub h: Harness,
    pub route: ChannelId,
    pub general: ChannelId,
    pub log: ActivityLog,
}

pub async fn world() -> Result<World> {
    world_with(Limits::default()).await
}

pub async fn world_with(limits: Limits) -> Result<World> {
    let h = Harness::new().await?;
    let (route, general) = h.fake.world(|w| {
        (
            w.add_text_channel(h.guild, true),
            w.add_text_channel(h.guild, false),
        )
    });
    let admin = h.principal(ADMIN, Origin::Dashboard);
    quack_activity_log::route_all(&h.engine, &admin, route, &h.key("route")).await?;
    set_enabled(
        &h.engine,
        &admin,
        &quack_activity_log::hooks(),
        true,
        &h.key("enable"),
    )
    .await?;
    h.engine.config_cache().reload(&h.db.db, h.guild).await?;
    let log = ActivityLog::with_limits(h.engine.clone(), limits);
    Ok(World {
        h,
        route,
        general,
        log,
    })
}

impl World {
    pub fn person(&self, id: u64) -> Person {
        Person {
            id: sf(id),
            name: Some(format!("user{id}")),
        }
    }

    pub fn created(&self, id: u64, text: &str) -> MessageCreated {
        self.created_by(id, MEMBER, AuthorKind::Member, text)
    }

    pub fn created_by(&self, id: u64, author: u64, kind: AuthorKind, text: &str) -> MessageCreated {
        MessageCreated {
            guild: self.h.guild,
            channel: self.general,
            id: sf(id),
            author: self.person(author),
            author_kind: kind,
            text: text.to_owned(),
            files: Vec::new(),
            embeds: Vec::new(),
        }
    }

    pub fn edit(&self, id: u64, text: Option<&str>) -> MessageUpdated {
        MessageUpdated {
            guild: self.h.guild,
            channel: self.general,
            id: sf(id),
            author: None,
            edited: true,
            text: text.map(str::to_owned),
            files: None,
            embeds: None,
        }
    }

    pub fn msg(&self, id: u64) -> MessageId {
        sf(id)
    }

    /// Delivers every queued post now.
    pub async fn drain(&self) -> Vec<Delivered> {
        let mut out = Vec::new();
        while let Some(d) = self.log.deliver_next().await {
            out.push(d);
        }
        out
    }

    /// The posts in the route channel, as debug text.
    pub fn posted(&self) -> Vec<String> {
        self.h.fake.world(|w| {
            w.sent_to(self.route)
                .iter()
                .filter_map(|m| m.msg.as_ref())
                .map(|m| format!("{m:?}"))
                .collect()
        })
    }

    pub async fn reload(&self) -> Result<()> {
        self.h
            .engine
            .config_cache()
            .reload(&self.h.db.db, self.h.guild)
            .await?;
        Ok(())
    }
}
