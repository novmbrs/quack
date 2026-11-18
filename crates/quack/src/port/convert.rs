//! Type mapping between `quack-discord` and the engine's port types: one
//! function per pair, every variant matched, so a new variant on either
//! side doesn't compile until it is mapped.

use quack_discord::model;
use quack_discord::{NotSentCause, Outcome, Refusal, RefusalKind, UnknownCause};
use quack_engine::port as engine;

/// A refusal's kind, variant for variant.
pub(crate) const fn refusal_kind(kind: RefusalKind) -> engine::RefusalKind {
    match kind {
        RefusalKind::MissingPermissions => engine::RefusalKind::MissingPermissions,
        RefusalKind::MissingAccess => engine::RefusalKind::MissingAccess,
        RefusalKind::CannotDm => engine::RefusalKind::CannotDm,
        RefusalKind::NoMutualGuilds => engine::RefusalKind::NoMutualGuilds,
        RefusalKind::UnknownGuild => engine::RefusalKind::UnknownGuild,
        RefusalKind::UnknownMember => engine::RefusalKind::UnknownMember,
        RefusalKind::UnknownUser => engine::RefusalKind::UnknownUser,
        RefusalKind::UnknownBan => engine::RefusalKind::UnknownBan,
        RefusalKind::UnknownMessage => engine::RefusalKind::UnknownMessage,
        RefusalKind::UnknownChannel => engine::RefusalKind::UnknownChannel,
        RefusalKind::TooLarge => engine::RefusalKind::TooLarge,
        RefusalKind::Other => engine::RefusalKind::Other,
    }
}

/// A refusal with its status and code kept.
pub(crate) const fn refusal(r: Refusal) -> engine::Refusal {
    engine::Refusal {
        status: r.status,
        code: r.code,
        kind: refusal_kind(r.kind),
    }
}

/// Why a result is unknown.
pub(crate) const fn unknown(cause: UnknownCause) -> engine::UnknownCause {
    match cause {
        UnknownCause::Timeout => engine::UnknownCause::Timeout,
        UnknownCause::ServerError(status) => engine::UnknownCause::ServerError(status),
        UnknownCause::Connection => engine::UnknownCause::Connection,
        UnknownCause::Malformed => engine::UnknownCause::Malformed,
    }
}

/// Why a request was never sent.
pub(crate) const fn not_sent(cause: NotSentCause) -> engine::NotSentCause {
    match cause {
        NotSentCause::Connection => engine::NotSentCause::Connection,
        NotSentCause::ShuttingDown => engine::NotSentCause::ShuttingDown,
        NotSentCause::Invalid => engine::NotSentCause::Invalid,
    }
}

/// An outcome with its value mapped.
pub(crate) fn outcome<T, U>(o: Outcome<T>, f: impl FnOnce(T) -> U) -> engine::Outcome<U> {
    match o {
        Outcome::Done(v) => engine::Outcome::Done(f(v)),
        Outcome::Refused(r) => engine::Outcome::Refused(refusal(r)),
        Outcome::RetryAfter(d) => engine::Outcome::RetryAfter(d),
        Outcome::NotSent(c) => engine::Outcome::NotSent(not_sent(c)),
        Outcome::Unknown(c) => engine::Outcome::Unknown(unknown(c)),
    }
}

/// A member lookup.
pub(crate) fn membership(m: quack_discord::facts::Membership) -> engine::Membership {
    match m {
        quack_discord::facts::Membership::Present(facts) => engine::Membership::Present(facts),
        quack_discord::facts::Membership::Absent => engine::Membership::Absent,
    }
}

/// Where a member read may come from.
pub(crate) const fn fresh(f: engine::Fresh) -> quack_discord::facts::Fresh {
    match f {
        engine::Fresh::Cached => quack_discord::facts::Fresh::Cached,
        engine::Fresh::Rest => quack_discord::facts::Fresh::Rest,
    }
}

/// What a create or edit returned.
pub(crate) fn sent(s: model::SentMessage) -> engine::SentMessage {
    engine::SentMessage {
        channel_id: s.channel_id,
        message_id: s.message_id,
        attachments: s.attachments,
    }
}

/// A file to upload.
pub(crate) fn file(f: &engine::OutgoingFile) -> model::OutgoingFile {
    model::OutgoingFile {
        name: f.name.clone(),
        content_type: f.content_type.clone(),
        bytes: f.bytes.clone(),
    }
}

/// Files on an edit.
pub(crate) fn file_edit(f: &engine::FileEdit) -> model::FileEdit {
    match f {
        engine::FileEdit::None => model::FileEdit::None,
        engine::FileEdit::Keep(id) => model::FileEdit::Keep(*id),
        engine::FileEdit::Replace(file_) => model::FileEdit::Replace(file(file_)),
    }
}

/// A message as the engine reads it.
pub(crate) fn fetched(m: model::Message) -> engine::FetchedMessage {
    engine::FetchedMessage {
        id: m.id,
        channel_id: m.channel_id,
        guild_id: m.guild_id,
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

/// A channel to create (the engine's specs have no category).
pub(crate) fn channel_spec(s: &engine::ChannelSpec) -> model::ChannelSpec {
    model::ChannelSpec {
        name: s.name.clone(),
        topic: s.topic.clone(),
        parent_id: None,
        overwrites: s.overwrites.clone(),
    }
}

/// A message nonce; an empty one is sent without (never happens: the
/// engine derives nonces from record ids).
pub(crate) fn nonce(n: &engine::Nonce) -> Option<model::Nonce> {
    model::Nonce::new(n.as_str())
}

/// An interaction token.
pub(crate) fn token(t: &engine::InteractionToken) -> model::InteractionToken {
    model::InteractionToken::new(t.expose())
}
