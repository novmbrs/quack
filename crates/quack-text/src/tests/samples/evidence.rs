//! §12.6 evidence.

use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};

use super::{Out, Sample, covering, sample, text};
use crate::evidence::{self, EvidenceItem};
use crate::tests::fixtures::*;

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    v.push(covering(
        "evidence.modal",
        &[
            "evidence.modal.title",
            "evidence.modal.label",
            "evidence.modal.description",
        ],
        "",
        Out::Modal(evidence::modal(case_id(), 12)),
    ));
    for (name, new, retrying, already) in [
        ("one", 1, 0, 0),
        ("other", 3, 0, 0),
        ("with already saved", 2, 0, 1),
        ("all already saved", 0, 0, 3),
        ("retrying one", 0, 1, 0),
        ("retrying several", 1, 2, 0),
    ] {
        v.push(covering(
            "evidence.added",
            &["evidence.added.already_saved", "evidence.added.retrying"],
            name,
            Out::Text(evidence::added(12, new, retrying, already)),
        ));
    }
    let item = EvidenceItem {
        case: case_id(),
        number: 12,
        item: 2,
        total: 3,
        state: EvidenceState::Saved,
        failure: None,
        author: Some(sam()),
        channel: channel("general"),
        sent_at: Some(t0()),
        edited_at: Some(later()),
        url: "https://discord.com/channels/111/444/777".to_owned(),
        content: Some("Join my server discord.gg/spam".to_owned()),
        embeds: vec!["Spam Server".to_owned(), "image".to_owned()],
        files: FileCopyState::ALL
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("screenshot-{i}.png"), *s))
            .collect(),
        files_not_saved: 2,
        channel_not_staff_only: true,
    };
    v.push(covering(
        "evidence.view",
        &[
            "evidence.view.heading",
            "evidence.view.message",
            "evidence.view.edited",
            "evidence.view.text",
            "evidence.view.embeds",
            "evidence.files_not_saved",
            "evidence.channel_not_staff_only",
            "evidence.file.copied",
            "evidence.file.pending",
            "evidence.file.link_only",
            "evidence.file.too_large",
            "evidence.file.unsupported_type",
            "evidence.file.copy_failed",
        ],
        "saved, every file state",
        Out::Msg(evidence::view(&item)),
    ));
    v.push(covering(
        "evidence.view",
        &["evidence.view.saving"],
        "saving",
        Out::Msg(evidence::view(&EvidenceItem {
            state: EvidenceState::Saving,
            author: None,
            sent_at: None,
            item: 1,
            ..item.clone()
        })),
    ));
    v.push(sample(
        "evidence.view",
        "one file not saved",
        Out::Msg(evidence::view(&EvidenceItem {
            files_not_saved: 1,
            files: vec![],
            channel_not_staff_only: false,
            item: 3,
            ..item.clone()
        })),
    ));
    v.push(sample(
        "evidence.view",
        "maximal",
        Out::Msg(evidence::view(&EvidenceItem {
            content: Some(long(4_000)),
            embeds: vec![long(256); 10],
            files: vec![(long(200), FileCopyState::Copied); 10],
            ..item.clone()
        })),
    ));
    for f in EvidenceFailure::ALL {
        v.push(covering(
            "evidence.failure",
            &[
                "evidence.failure.message_deleted",
                "evidence.failure.moderator_no_access",
                "evidence.failure.quack_no_access",
                "evidence.failure.other_server",
                "evidence.failure.discord_error",
            ],
            format!("{f:?}"),
            Out::Msg(evidence::view(&EvidenceItem {
                state: EvidenceState::Failed,
                failure: Some(*f),
                ..item.clone()
            })),
        ));
    }
    v.push(text("evidence.view.none", "", evidence::view_none(12)));
    for s in EvidenceState::ALL {
        v.push(text(
            "evidence.state_text",
            format!("{s:?}"),
            evidence::state_text(*s, Some(EvidenceFailure::QuackNoAccess)),
        ));
    }
    v
}
