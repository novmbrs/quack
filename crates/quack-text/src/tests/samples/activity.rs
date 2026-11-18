//! §12.13 server activity log.

use super::{Out, Sample, covering, msg, sample};
use crate::activity::{self, ActivityPost, Content, FileLink, MessageFacts};
use crate::tests::fixtures::*;

fn message() -> MessageFacts {
    MessageFacts {
        author: sam(),
        channel: channel("general"),
        id: message_id(777),
        at: t0(),
        files: vec![FileLink {
            name: "cat.png".to_owned(),
            url: "https://cdn.discordapp.com/attachments/1/2/cat.png".to_owned(),
        }],
        embeds: vec!["image".to_owned()],
    }
}

const ACTIVITY_IDS: &[&str] = &[
    "activity.message_edited",
    "activity.before",
    "activity.before_unknown",
    "activity.after",
    "activity.after_empty",
    "activity.message_deleted",
    "activity.files",
    "activity.embeds",
    "activity.text_hidden",
    "activity.subtext",
];

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    let plain = MessageFacts {
        files: vec![],
        embeds: vec![],
        ..message()
    };
    for (name, p) in [
        (
            "edited",
            ActivityPost::MessageEdited {
                message: message(),
                before: Content::Text("old".to_owned()),
                after: Content::Text("new".to_owned()),
            },
        ),
        (
            "edited, before unknown, after empty",
            ActivityPost::MessageEdited {
                message: plain.clone(),
                before: Content::Unknown,
                after: Content::Empty,
            },
        ),
        (
            "edited, before empty, after unknown",
            ActivityPost::MessageEdited {
                message: plain.clone(),
                before: Content::Empty,
                after: Content::Unknown,
            },
        ),
        (
            "edited, text hidden",
            ActivityPost::MessageEdited {
                message: plain.clone(),
                before: Content::Hidden,
                after: Content::Hidden,
            },
        ),
        (
            "edited, maximal",
            ActivityPost::MessageEdited {
                message: message(),
                before: Content::Text(long(4_000)),
                after: Content::Text(long(4_000)),
            },
        ),
        (
            "deleted",
            ActivityPost::MessageDeleted {
                message: message(),
                content: Content::Text(HOSTILE.to_owned()),
            },
        ),
        (
            "deleted, hidden",
            ActivityPost::MessageDeleted {
                message: plain.clone(),
                content: Content::Hidden,
            },
        ),
        (
            "deleted, no text",
            ActivityPost::MessageDeleted {
                message: message(),
                content: Content::Empty,
            },
        ),
    ] {
        v.push(covering(
            "activity.message",
            ACTIVITY_IDS,
            name,
            Out::Msg(activity::post(&p)),
        ));
    }
    let ids: Vec<_> = (1..=60).map(message_id).collect();
    v.push(covering(
        "activity.message_deleted_unknown",
        &["activity.subtext"],
        "",
        Out::Msg(activity::post(&ActivityPost::MessageDeletedUnknown {
            channel: channel("general"),
            id: message_id(777),
            at: t0(),
        })),
    ));
    v.push(covering(
        "activity.messages_deleted_unknown",
        &["activity.message_ids"],
        "60 messages",
        Out::Msg(activity::post(&ActivityPost::MessagesDeletedUnknown {
            channel: channel("general"),
            ids: ids.clone(),
            at: t0(),
        })),
    ));
    let known: Vec<_> = (0..25)
        .map(|i| {
            (
                sam(),
                if i == 1 {
                    Content::Empty
                } else {
                    Content::Text(long(300))
                },
            )
        })
        .collect();
    v.push(covering(
        "activity.messages_bulk_deleted",
        &[
            "activity.bulk_known",
            "activity.bulk_row",
            "activity.message_ids",
        ],
        "25 known of 60",
        Out::Msg(activity::post(&ActivityPost::MessagesBulkDeleted {
            channel: channel("general"),
            ids: ids.clone(),
            known,
            at: t0(),
        })),
    ));
    v.push(sample(
        "activity.messages_bulk_deleted",
        "one known",
        Out::Msg(activity::post(&ActivityPost::MessagesBulkDeleted {
            channel: channel("general"),
            ids: ids[..3].to_vec(),
            known: vec![(sam(), Content::Text("hi".to_owned()))],
            at: t0(),
        })),
    ));
    v.push(sample(
        "activity.messages_bulk_deleted",
        "none known",
        Out::Msg(activity::post(&ActivityPost::MessagesBulkDeleted {
            channel: channel("general"),
            ids: ids[..3].to_vec(),
            known: vec![],
            at: t0(),
        })),
    ));
    v.push(msg(
        "activity.member_joined",
        "",
        activity::post(&ActivityPost::MemberJoined {
            member: sam(),
            account_created: t0(),
        }),
    ));
    v.push(msg(
        "activity.member_left",
        "",
        activity::post(&ActivityPost::MemberLeft {
            username: "samtheduck".to_owned(),
            id: account(222),
        }),
    ));
    v.push(covering(
        "activity.member_banned",
        &["activity.reason"],
        "with reason",
        Out::Msg(activity::post(&ActivityPost::MemberBanned {
            member: sam(),
            by: ana(),
            reason: Some("Raiding".to_owned()),
        })),
    ));
    v.push(msg(
        "activity.member_unbanned",
        "",
        activity::post(&ActivityPost::MemberUnbanned {
            member: sam(),
            by: ana(),
            reason: None,
        }),
    ));
    v.push(msg(
        "activity.server_changed",
        "",
        activity::post(&ActivityPost::ServerChanged),
    ));
    v.push(msg(
        "activity.channel_created",
        "",
        activity::post(&ActivityPost::ChannelCreated(channel("new-channel"))),
    ));
    v.push(msg(
        "activity.channel_changed",
        "",
        activity::post(&ActivityPost::ChannelChanged(channel("general"))),
    ));
    v.push(msg(
        "activity.channel_deleted",
        "",
        activity::post(&ActivityPost::ChannelDeleted("old-channel".to_owned())),
    ));
    v
}
