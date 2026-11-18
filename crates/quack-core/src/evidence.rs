//! Evidence (§7): saved message snapshots and their files.

use crate::{DomainError, InvalidTransition, limits};

str_enum! {
    /// An evidence item's stored state (`evidence_state`).
    pub enum EvidenceState {
        /// The capture job has not settled ("Saving evidence…").
        Saving = "saving",
        /// The snapshot is saved (individual files may still have failed).
        Saved = "saved",
        /// The message could not be saved.
        Failed = "failed",
    }
}

str_enum! {
    /// Why an evidence item failed (`evidence_failure`).
    pub enum EvidenceFailure {
        /// The message or its channel is gone.
        MessageDeleted = "message_deleted",
        /// The staff member who added it can't read that channel.
        ModeratorNoAccess = "moderator_no_access",
        /// Quack can't read that channel.
        QuackNoAccess = "quack_no_access",
        /// The message belongs to another server.
        OtherServer = "other_server",
        /// Discord kept failing until the budget ran out.
        DiscordError = "discord_error",
    }
}

str_enum! {
    /// One file's copy state (`file_copy_state`).
    pub enum FileCopyState {
        /// Not yet processed.
        Pending = "pending",
        /// A copy exists in the evidence channel.
        Copied = "copied",
        /// No evidence channel is set; only the original link is kept.
        LinkOnly = "link_only",
        /// Larger than 10 MiB.
        TooLarge = "too_large",
        /// Not an eligible content type.
        UnsupportedType = "unsupported_type",
        /// Download or upload failed, or the channel is not staff-only.
        CopyFailed = "copy_failed",
        /// Over 10 files on the message or 100 on the case.
        LimitReached = "limit_reached",
    }
}

/// An event that settles an evidence item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceEvent {
    /// The snapshot was saved.
    Saved,
    /// Capture failed.
    Failed(EvidenceFailure),
}

/// Settles an item: only `saving` can move, to `saved` or `failed`.
pub fn transition(
    state: EvidenceState,
    event: EvidenceEvent,
) -> Result<EvidenceState, InvalidTransition> {
    match (state, event) {
        (EvidenceState::Saving, EvidenceEvent::Saved) => Ok(EvidenceState::Saved),
        (EvidenceState::Saving, EvidenceEvent::Failed(_)) => Ok(EvidenceState::Failed),
        (_, event) => Err(InvalidTransition {
            machine: "evidence",
            from: state.as_str(),
            event: match event {
                EvidenceEvent::Saved => "saved",
                EvidenceEvent::Failed(_) => "failed",
            },
        }),
    }
}

/// Whether Quack copies a file of `size_bytes` with `content_type` to the
/// evidence channel (§7.3): at most 10 MiB and an image, video, audio,
/// plain text, or PDF type (lowercased, parameters ignored). The error is
/// the copy state to record instead.
pub fn copy_eligibility(size_bytes: u64, content_type: &str) -> Result<(), FileCopyState> {
    if size_bytes > limits::MAX_COPY_BYTES {
        return Err(FileCopyState::TooLarge);
    }
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let eligible = ["image/", "video/", "audio/"]
        .iter()
        .any(|p| essence.starts_with(p))
        || essence == "text/plain"
        || essence == "application/pdf";
    if eligible {
        Ok(())
    } else {
        Err(FileCopyState::UnsupportedType)
    }
}

/// Checks an add-evidence call before any write (§7.1): 1–10 links, and
/// room under the per-case cap for the new (not already saved) items.
pub fn check_room(
    existing_items: u32,
    new_items: usize,
    links_in_call: usize,
) -> Result<(), DomainError> {
    if links_in_call > limits::MAX_LINKS_PER_CALL {
        return Err(DomainError::TooManyLinks);
    }
    let new_items = u32::try_from(new_items).unwrap_or(u32::MAX);
    if existing_items.saturating_add(new_items) > limits::MAX_EVIDENCE_ITEMS {
        return Err(DomainError::EvidenceLimitReached {
            count: existing_items,
        });
    }
    Ok(())
}

/// How many of a message's `files` get rows that may be copied, given the
/// case already has `case_files`; the rest are recorded `limit_reached`.
pub fn files_within_caps(case_files: u32, files: u32) -> u32 {
    let room = limits::MAX_EVIDENCE_FILES.saturating_sub(case_files);
    files.min(limits::MAX_FILES_PER_EVIDENCE).min(room)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_saving_moves() {
        let events = [
            EvidenceEvent::Saved,
            EvidenceEvent::Failed(EvidenceFailure::MessageDeleted),
        ];
        assert_eq!(
            transition(EvidenceState::Saving, events[0]),
            Ok(EvidenceState::Saved)
        );
        assert_eq!(
            transition(EvidenceState::Saving, events[1]),
            Ok(EvidenceState::Failed)
        );
        for state in [EvidenceState::Saved, EvidenceState::Failed] {
            for event in events {
                assert!(transition(state, event).is_err(), "{state:?} {event:?}");
            }
        }
    }

    #[test]
    fn copy_rules() {
        assert_eq!(copy_eligibility(10_485_760, "image/png"), Ok(()));
        assert_eq!(
            copy_eligibility(10_485_761, "image/png"),
            Err(FileCopyState::TooLarge)
        );
        for ok in [
            "IMAGE/PNG",
            "video/mp4",
            "audio/ogg",
            "text/plain; charset=utf-8",
            "application/pdf",
        ] {
            assert_eq!(copy_eligibility(1, ok), Ok(()), "{ok}");
        }
        for bad in ["text/html", "application/zip", "", "imagex/png"] {
            assert_eq!(
                copy_eligibility(1, bad),
                Err(FileCopyState::UnsupportedType),
                "{bad}"
            );
        }
    }

    #[test]
    fn caps() {
        assert_eq!(check_room(0, 10, 10), Ok(()));
        assert_eq!(check_room(0, 1, 11), Err(DomainError::TooManyLinks));
        assert_eq!(check_room(45, 5, 5), Ok(()));
        assert_eq!(
            check_room(45, 6, 6),
            Err(DomainError::EvidenceLimitReached { count: 45 })
        );
        assert_eq!(check_room(50, 0, 3), Ok(()));
        assert_eq!(files_within_caps(0, 12), 10);
        assert_eq!(files_within_caps(95, 10), 5);
        assert_eq!(files_within_caps(100, 3), 0);
        assert_eq!(files_within_caps(120, 3), 0);
    }
}
