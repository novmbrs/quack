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
    }
}

/// An event on an evidence item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceEvent {
    /// The snapshot was saved.
    Saved,
    /// Capture failed.
    Failed(EvidenceFailure),
    /// Staff added the message again after its capture failed: capture it
    /// again (§7.1).
    CaptureAgain,
}

impl EvidenceEvent {
    /// The event's name, for errors and logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Failed(_) => "failed",
            Self::CaptureAgain => "capture_again",
        }
    }
}

/// Moves an item: `saving` settles to `saved` or `failed`, and a `failed`
/// item added again goes back to `saving` with a new job.
pub fn transition(
    state: EvidenceState,
    event: EvidenceEvent,
) -> Result<EvidenceState, InvalidTransition> {
    match (state, event) {
        (EvidenceState::Saving, EvidenceEvent::Saved) => Ok(EvidenceState::Saved),
        (EvidenceState::Saving, EvidenceEvent::Failed(_)) => Ok(EvidenceState::Failed),
        (EvidenceState::Failed, EvidenceEvent::CaptureAgain) => Ok(EvidenceState::Saving),
        (_, event) => Err(InvalidTransition {
            machine: "evidence",
            from: state.as_str(),
            event: event.name(),
        }),
    }
}

/// What adding a message that already has an evidence row on the case does
/// (§4.9, §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddAgain {
    /// It is `saving` or `saved`: reported as already saved.
    AlreadySaved,
    /// Its capture `failed`: capture it again.
    CaptureAgain,
}

/// Decides what adding an existing item again does.
pub const fn add_again(state: EvidenceState) -> AddAgain {
    match state {
        EvidenceState::Failed => AddAgain::CaptureAgain,
        EvidenceState::Saving | EvidenceState::Saved => AddAgain::AlreadySaved,
    }
}

/// Whether Quack copies a file of `size_bytes` with `content_type` to the
/// evidence channel (§7.3): at most 10 MiB and an image, video, audio,
/// plain text, or PDF type (lowercased, parameters ignored). A file with no
/// content type is not eligible. The error is the copy state to record
/// instead.
pub fn copy_eligibility(size_bytes: u64, content_type: Option<&str>) -> Result<(), FileCopyState> {
    if size_bytes > limits::MAX_COPY_BYTES {
        return Err(FileCopyState::TooLarge);
    }
    let Some(content_type) = content_type else {
        return Err(FileCopyState::UnsupportedType);
    };
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

/// Checks evidence about to be admitted before any write (§7.1): at most
/// 10 links in one call, and room under the per-case cap for the new (not
/// already saved) items. Zero links is legal here (a case edit that names
/// no new message); the add-evidence entry point refuses an empty call
/// itself before calling this.
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

/// How many of a message's `files` are admitted (get `evidence_files`
/// rows), given the case already has `case_files`: at most 10 per message
/// and 100 per case. The rest are only counted in
/// `evidence.files_not_saved`.
pub fn files_within_caps(case_files: u32, files: u32) -> u32 {
    let room = limits::MAX_EVIDENCE_FILES.saturating_sub(case_files);
    files.min(limits::MAX_FILES_PER_EVIDENCE).min(room)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pair_matches_the_table() {
        use EvidenceEvent as E;
        use EvidenceState as S;
        let events = [
            E::Saved,
            E::Failed(EvidenceFailure::MessageDeleted),
            E::CaptureAgain,
        ];
        for &state in S::ALL {
            for event in events {
                let want = match (state, event) {
                    (S::Saving, E::Saved) => Some(S::Saved),
                    (S::Saving, E::Failed(_)) => Some(S::Failed),
                    (S::Failed, E::CaptureAgain) => Some(S::Saving),
                    _ => None,
                };
                assert_eq!(transition(state, event).ok(), want, "{state:?} {event:?}");
            }
        }
        assert_eq!(add_again(S::Failed), AddAgain::CaptureAgain);
        assert_eq!(add_again(S::Saving), AddAgain::AlreadySaved);
        assert_eq!(add_again(S::Saved), AddAgain::AlreadySaved);
    }

    #[test]
    fn enum_names_match_the_database() {
        let names: Vec<&str> = FileCopyState::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "pending",
                "copied",
                "link_only",
                "too_large",
                "unsupported_type",
                "copy_failed"
            ]
        );
        let names: Vec<&str> = EvidenceFailure::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "message_deleted",
                "moderator_no_access",
                "quack_no_access",
                "other_server",
                "discord_error"
            ]
        );
    }

    #[test]
    fn copy_rules() {
        assert_eq!(copy_eligibility(10_485_760, Some("image/png")), Ok(()));
        assert_eq!(
            copy_eligibility(10_485_761, Some("image/png")),
            Err(FileCopyState::TooLarge)
        );
        assert_eq!(
            copy_eligibility(1, None),
            Err(FileCopyState::UnsupportedType)
        );
        for ok in [
            "IMAGE/PNG",
            "video/mp4",
            "audio/ogg",
            "text/plain; charset=utf-8",
            "application/pdf",
        ] {
            assert_eq!(copy_eligibility(1, Some(ok)), Ok(()), "{ok}");
        }
        for bad in ["text/html", "application/zip", "", "imagex/png"] {
            assert_eq!(
                copy_eligibility(1, Some(bad)),
                Err(FileCopyState::UnsupportedType),
                "{bad}"
            );
        }
    }

    #[test]
    fn caps() {
        assert_eq!(check_room(0, 10, 10), Ok(()));
        assert_eq!(check_room(0, 1, 11), Err(DomainError::TooManyLinks));
        assert_eq!(DomainError::TooManyLinks.code(), "too_many_links");
        assert_eq!(DomainError::TooManyLinks.http_status(), 422);
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
