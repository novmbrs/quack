//! §12.6 evidence.

use quack_core::evidence::{EvidenceFailure, EvidenceKind, EvidenceState, FileCopyState};
use quack_core::ids::{ChannelId, EvidenceId};

use super::{Out, Sample, covering, msg, sample, text};
use crate::evidence::{self, Added, EvidenceItem, PickerCase, Surrounding, SurroundingLine};
use crate::tests::fixtures::*;

fn evidence_id() -> EvidenceId {
    EvidenceId::from_uuid(uuid::Uuid::from_u128(0x1234))
}

fn channel_id() -> ChannelId {
    ChannelId::new(444).unwrap_or_else(|| unreachable!())
}

fn saved_message() -> EvidenceItem {
    EvidenceItem {
        case: case_id(),
        number: 12,
        evidence: evidence_id(),
        kind: EvidenceKind::Message,
        item: 2,
        total: 3,
        state: EvidenceState::Saved,
        failure: None,
        added_by: Some(mod_ana()),
        added_at: t0(),
        author: Some(sam()),
        channel: Some(channel("general")),
        sent_at: Some(t0()),
        edited_at: Some(later()),
        url: Some("https://discord.com/channels/111/444/777".to_owned()),
        content: Some("Join my server discord.gg/spam".to_owned()),
        embeds: vec!["Spam Server".to_owned(), "image".to_owned()],
        files: FileCopyState::ALL
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("screenshot-{i}.png"), *s))
            .collect(),
        files_not_saved: 2,
        channel_not_staff_only: true,
        note: None,
        visible_to_member: true,
        surrounding: None,
        gallery: Vec::new(),
    }
}

fn upload() -> EvidenceItem {
    EvidenceItem {
        kind: EvidenceKind::Upload,
        author: None,
        channel: None,
        sent_at: None,
        edited_at: None,
        url: None,
        content: None,
        embeds: Vec::new(),
        files: vec![
            ("report.png".to_owned(), FileCopyState::Copied),
            ("clip.mp4".to_owned(), FileCopyState::Copied),
            ("notes.txt".to_owned(), FileCopyState::CopyFailed),
        ],
        files_not_saved: 0,
        channel_not_staff_only: false,
        note: Some("Sent to us by a member in a ticket.".to_owned()),
        visible_to_member: false,
        gallery: vec![
            (
                "report.png".to_owned(),
                "https://cdn.discordapp.com/attachments/9/10/SPOILER_report.png?ex=1".to_owned(),
            ),
            (
                "clip.mp4".to_owned(),
                "https://cdn.discordapp.com/attachments/9/11/SPOILER_clip.mp4?ex=1".to_owned(),
            ),
        ],
        ..saved_message()
    }
}

fn added_samples(v: &mut Vec<Sample>) {
    let added = |new, retrying, restored, already_saved, files| Added {
        number: 12,
        new,
        retrying,
        restored,
        already_saved,
        files,
    };
    for (name, a) in [
        ("one", added(1, 0, 0, 0, 0)),
        ("other", added(3, 0, 0, 0, 0)),
        ("with already saved", added(2, 0, 0, 1, 0)),
        ("all already saved", added(0, 0, 0, 3, 0)),
        ("retrying one", added(0, 1, 0, 0, 0)),
        ("retrying several", added(1, 2, 0, 0, 0)),
        ("files", added(0, 0, 0, 0, 3)),
        ("one file and a message", added(1, 0, 0, 0, 1)),
        ("restored one", added(0, 0, 1, 0, 0)),
        ("restored several", added(0, 0, 2, 1, 0)),
    ] {
        v.push(covering(
            "evidence.added",
            &[
                "evidence.added.already_saved",
                "evidence.added.retrying",
                "evidence.added.restored",
                "evidence.added.files",
            ],
            name,
            Out::Text(evidence::added(&a)),
        ));
    }
    v.push(covering(
        "evidence.added",
        &["button.add_evidence", "button.view_evidence"],
        "reply with buttons",
        Out::Msg(evidence::added_reply(case_id(), &added(1, 0, 0, 0, 2))),
    ));
}

fn view_samples(v: &mut Vec<Sample>) {
    let item = saved_message();
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
            "evidence.button.add_note",
            "evidence.button.remove",
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
    v.push(covering(
        "evidence.view",
        &[
            "evidence.view.note",
            "evidence.button.edit_note",
            "evidence.button.surrounding",
        ],
        "with a note and messages around it",
        Out::Msg(evidence::view(&EvidenceItem {
            note: Some("The first of three spam posts **that day**.".to_owned()),
            surrounding: Some(6),
            files: vec![],
            files_not_saved: 0,
            channel_not_staff_only: false,
            ..item.clone()
        })),
    ));
    v.push(covering(
        "evidence.view",
        &[
            "evidence.view.upload",
            "evidence.view.staff_only",
            "evidence.view.gallery",
            "evidence.button.show_to_member",
        ],
        "uploaded files, staff only, with a gallery",
        Out::Msg(evidence::view(&upload())),
    ));
    v.push(covering(
        "evidence.view",
        &[
            "evidence.view.visible_to_member",
            "evidence.button.hide_from_member",
        ],
        "uploaded files the member sees",
        Out::Msg(evidence::view(&EvidenceItem {
            visible_to_member: true,
            note: None,
            gallery: Vec::new(),
            ..upload()
        })),
    ));
    v.push(sample(
        "evidence.view",
        "uploaded files, still copying",
        Out::Msg(evidence::view(&EvidenceItem {
            state: EvidenceState::Saving,
            files: vec![("report.png".to_owned(), FileCopyState::Pending)],
            gallery: Vec::new(),
            ..upload()
        })),
    ));
    v.push(covering(
        "evidence.view",
        &["evidence.file.upload_failed"],
        "uploaded files, one not copied",
        Out::Msg(evidence::view(&EvidenceItem {
            files: vec![
                ("report.png".to_owned(), FileCopyState::Copied),
                ("clip.mp4".to_owned(), FileCopyState::CopyFailed),
            ],
            gallery: Vec::new(),
            ..upload()
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
            note: Some(long(500)),
            surrounding: Some(10),
            ..item.clone()
        })),
    ));
    for f in EvidenceFailure::ALL {
        let base = if *f == EvidenceFailure::NoFileCopied {
            upload()
        } else {
            item.clone()
        };
        v.push(covering(
            "evidence.failure",
            &[
                "evidence.failure.message_deleted",
                "evidence.failure.moderator_no_access",
                "evidence.failure.quack_no_access",
                "evidence.failure.other_server",
                "evidence.failure.discord_error",
                "evidence.failure.no_file_copied",
                "evidence.failure.evidence_copy",
            ],
            format!("{f:?}"),
            Out::Msg(evidence::view(&EvidenceItem {
                state: EvidenceState::Failed,
                failure: Some(*f),
                gallery: Vec::new(),
                ..base
            })),
        ));
    }
    v.push(text("evidence.view.none", "", evidence::view_none(12)));
    v.push(msg(
        "evidence.view.none",
        "with Add evidence",
        evidence::view_none_reply(case_id(), 12),
    ));
    for s in EvidenceState::ALL {
        v.push(text(
            "evidence.state_text",
            format!("{s:?}"),
            evidence::state_text(*s, Some(EvidenceFailure::QuackNoAccess)),
        ));
    }
}

fn surrounding_samples(v: &mut Vec<Sample>) {
    let line = |before: bool, content: Option<&str>, files: u32| SurroundingLine {
        author: sam(),
        sent_at: t0(),
        content: content.map(str::to_owned),
        files,
        before,
    };
    let around = Surrounding {
        case: case_id(),
        number: 12,
        evidence: evidence_id(),
        author: Some(sam()),
        lines: vec![
            line(true, Some("anyone want free nitro?"), 0),
            line(true, None, 2),
            line(false, Some("@everyone check my profile"), 0),
        ],
    };
    v.push(covering(
        "evidence.surrounding",
        &[
            "evidence.surrounding.heading",
            "evidence.surrounding.saved_message",
        ],
        "before and after",
        Out::Msg(evidence::surrounding(&around)),
    ));
    v.push(sample(
        "evidence.surrounding",
        "only before",
        Out::Msg(evidence::surrounding(&Surrounding {
            lines: vec![line(true, Some("hi"), 0)],
            author: None,
            ..around.clone()
        })),
    ));
    v.push(covering(
        "evidence.surrounding",
        &["evidence.surrounding.none"],
        "none",
        Out::Msg(evidence::surrounding(&Surrounding {
            lines: vec![],
            ..around.clone()
        })),
    ));
    v.push(sample(
        "evidence.surrounding",
        "maximal",
        Out::Msg(evidence::surrounding(&Surrounding {
            lines: (0..10).map(|i| line(i < 5, Some(&long(500)), 10)).collect(),
            ..around
        })),
    ));
}

fn edit_samples(v: &mut Vec<Sample>) {
    v.push(msg(
        "evidence.remove.confirm",
        "",
        evidence::remove_confirm(12, evidence_id()),
    ));
    v.push(covering(
        "evidence.removed",
        &["evidence.removed.already"],
        "removed",
        Out::Text(evidence::removed(12, true)),
    ));
    v.push(sample(
        "evidence.removed",
        "already removed",
        Out::Text(evidence::removed(12, false)),
    ));
    v.push(text(
        "evidence.removed_by",
        "",
        evidence::removed_by(&mod_ana(), later()),
    ));
    v.push(text(
        "evidence.visibility.shown",
        "",
        evidence::visibility_changed(12, true),
    ));
    v.push(text(
        "evidence.visibility.hidden",
        "",
        evidence::visibility_changed(12, false),
    ));
    v.push(covering(
        "evidence.note.modal",
        &["evidence.note.label", "evidence.note.description"],
        "with a note",
        Out::Modal(evidence::note_modal(
            evidence_id(),
            12,
            3,
            Some("From the ticket."),
        )),
    ));
    v.push(sample(
        "evidence.note.modal",
        "without a note",
        Out::Modal(evidence::note_modal(evidence_id(), 12, 0, None)),
    ));
    v.push(text(
        "evidence.note.saved",
        "",
        evidence::note_saved(12, false),
    ));
    v.push(text(
        "evidence.note.cleared",
        "",
        evidence::note_saved(12, true),
    ));
}

fn picker_samples(v: &mut Vec<Sample>) {
    let case = |number, rule: Option<&str>, voided| PickerCase {
        case: case_id(),
        number,
        rule: rule.map(str::to_owned),
        created_at: t0(),
        voided,
    };
    v.push(covering(
        "evidence.picker",
        &[
            "evidence.picker.heading",
            "evidence.picker.choose",
            "evidence.picker.option",
            "evidence.picker.voided",
            "evidence.picker.by_number",
        ],
        "cases",
        Out::Msg(evidence::picker(
            channel_id(),
            message_id(777),
            &[
                case(14, Some("Spam"), false),
                case(12, Some("Harassment"), true),
                case(3, None, false),
            ],
        )),
    ));
    v.push(covering(
        "evidence.picker",
        &["evidence.picker.none"],
        "no cases",
        Out::Msg(evidence::picker(channel_id(), message_id(777), &[])),
    ));
    v.push(sample(
        "evidence.picker",
        "maximal",
        Out::Msg(evidence::picker(
            channel_id(),
            message_id(777),
            &(0..30)
                .map(|n| case(9_007_199_254_740_991 - n, Some(&long(100)), n % 2 == 0))
                .collect::<Vec<_>>(),
        )),
    ));
    v.push(covering(
        "evidence.picker.modal",
        &["evidence.picker.modal.case_number"],
        "",
        Out::Modal(evidence::picker_modal(channel_id(), message_id(777))),
    ));
}

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    v.push(covering(
        "evidence.modal",
        &[
            "evidence.modal.title",
            "evidence.modal.label",
            "evidence.modal.description",
            "evidence.modal.files",
            "evidence.modal.note",
        ],
        "",
        Out::Modal(evidence::modal(case_id(), 12, false)),
    ));
    added_samples(&mut v);
    view_samples(&mut v);
    surrounding_samples(&mut v);
    edit_samples(&mut v);
    picker_samples(&mut v);
    v
}
