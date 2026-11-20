//! Mapping for the port types `quack-discord` shapes differently from the
//! engine; the outcome and payload types are shared (`quack_core::discord`).

use quack_discord::model;
use quack_engine::port as engine;

/// A message as the engine reads it.
pub(crate) fn fetched(m: model::Message) -> engine::FetchedMessage {
    engine::FetchedMessage {
        id: m.id,
        channel_id: m.channel_id,
        author_name: m
            .author
            .global_name
            .clone()
            .unwrap_or_else(|| m.author.username.clone()),
        author_id: m.author.id,
        author_bot: m.author.bot,
        content: m.content,
        created_at: m.created_at,
        edited_at: m.edited_at,
        embeds: m
            .embeds
            .into_iter()
            .map(|e| engine::FetchedEmbed {
                kind: e.kind,
                title: e.title,
                description: e.description,
                url: e.url,
            })
            .collect(),
        attachments: m
            .attachments
            .into_iter()
            .map(|a| engine::FetchedAttachment {
                id: a.id,
                filename: a.filename,
                content_type: a.content_type,
                size_bytes: a.size,
                url: a.url,
            })
            .collect(),
    }
}

/// A channel to create.
pub(crate) fn channel_spec(s: &engine::ChannelSpec) -> model::ChannelSpec {
    model::ChannelSpec {
        name: s.name.clone(),
        topic: s.topic.clone(),
        parent_id: s.parent,
        overwrites: s.overwrites.clone(),
        category: s.category,
    }
}

/// A message nonce; an empty one is sent without (never happens: the
/// engine derives nonces from record ids).
pub(crate) fn nonce(n: &engine::Nonce) -> Option<model::Nonce> {
    model::Nonce::new(n.as_str())
}
