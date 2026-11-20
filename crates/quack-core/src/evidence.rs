//! Evidence (§7): saved message snapshots, uploaded files, and the rules
//! about who sees them.

use crate::{DomainError, limits};

str_enum! {
    /// What an evidence item holds (`evidence_kind`).
    pub enum EvidenceKind {
        /// A saved Discord message and its files.
        Message = "message",
        /// Files staff uploaded directly, with no source message.
        Upload = "upload",
    }
}

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
        /// Uploaded files only: none of them ended up copied.
        NoFileCopied = "no_file_copied",
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

/// Whether a case has room for `new_files` more files of uploaded items,
/// given `existing_files` files of items that aren't removed (§7.1): at most
/// 100 per case.
pub fn check_file_room(existing_files: u32, new_files: usize) -> Result<(), DomainError> {
    let new_files = u32::try_from(new_files).unwrap_or(u32::MAX);
    if existing_files.saturating_add(new_files) > limits::MAX_EVIDENCE_FILES {
        return Err(DomainError::EvidenceFileLimitReached {
            count: existing_files,
        });
    }
    Ok(())
}

/// At most 10 files in one upload (§7.1).
pub const fn check_upload_count(files: usize) -> Result<(), DomainError> {
    if files > limits::MAX_UPLOAD_FILES {
        Err(DomainError::TooManyFiles)
    } else {
        Ok(())
    }
}

str_enum! {
    /// Why a file offered for upload is refused before anything is saved.
    pub enum FileRefusal {
        /// Larger than 10 MiB.
        TooLarge = "too_large",
        /// Not an image, video, audio, plain text, or PDF.
        UnsupportedType = "unsupported_type",
    }
}

/// A file refused up front, by its name (`error.files_not_allowed`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedFile {
    /// The file's name as it was offered.
    pub filename: String,
    /// Why.
    pub why: FileRefusal,
}

/// What [`refused_uploads`] needs to know about one offered file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfferedFile<'a> {
    /// Its name.
    pub filename: &'a str,
    /// Its size.
    pub size_bytes: u64,
    /// Its content type, if known.
    pub content_type: Option<&'a str>,
}

/// The offered files Quack would not copy (§7.3 eligibility), in order.
/// Uploading refuses them all up front instead of saving a file that can
/// never be copied.
pub fn refused_uploads(files: &[OfferedFile<'_>]) -> Vec<RefusedFile> {
    files
        .iter()
        .filter_map(|f| {
            let why = match copy_eligibility(f.size_bytes, f.content_type) {
                Ok(()) => return None,
                Err(FileCopyState::TooLarge) => FileRefusal::TooLarge,
                Err(_) => FileRefusal::UnsupportedType,
            };
            Some(RefusedFile {
                filename: f.filename.to_owned(),
                why,
            })
        })
        .collect()
}

/// The stored name of an uploaded file: the last path segment (browsers
/// and Discord send bare names, but nothing relies on it), without control
/// characters, trimmed, at most 255 characters; `file` when nothing is left.
pub fn upload_filename(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last.chars().filter(|c| !c.is_control()).collect();
    let name = limits::truncate_chars(clean.trim(), limits::EVIDENCE_FILENAME_CHARS).trim();
    if name.is_empty() || name == "." || name == ".." {
        "file".to_owned()
    } else {
        name.to_owned()
    }
}

/// A note longer than 500 characters (`error.note_too_long`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("note too long")]
pub struct NoteTooLong;

/// A staff note as stored: trimmed; empty is no note; at most 500
/// characters.
pub fn clean_note(raw: Option<&str>) -> Result<Option<String>, NoteTooLong> {
    let Some(raw) = raw.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    if limits::EVIDENCE_NOTE.allows(raw) {
        Ok(Some(raw.to_owned()))
    } else {
        Err(NoteTooLong)
    }
}

/// Whether a new item of `kind` is visible to the member (§7.4): saved
/// messages always are; uploaded files start staff-only.
pub const fn visible_by_default(kind: EvidenceKind) -> bool {
    matches!(kind, EvidenceKind::Message)
}

/// Whether the case's member sees an item on their own case page (§7.4):
/// never a removed item; a saved message always; uploaded files only once
/// staff showed them.
pub const fn member_sees(kind: EvidenceKind, visible_to_member: bool, removed: bool) -> bool {
    !removed
        && match kind {
            EvidenceKind::Message => true,
            EvidenceKind::Upload => visible_to_member,
        }
}

/// Whether staff may change an item's note or visibility (§7.5): not once
/// it is removed; visibility only on uploaded files.
pub const fn check_editable(
    kind: EvidenceKind,
    removed: bool,
    visibility: bool,
) -> Result<(), DomainError> {
    if removed {
        return Err(DomainError::EvidenceRemoved);
    }
    if visibility && matches!(kind, EvidenceKind::Message) {
        return Err(DomainError::EvidenceNotUpload);
    }
    Ok(())
}

/// How an uploaded-files item settles once none of its files is pending
/// (§7.2): saved if at least one was copied, else failed.
pub const fn upload_settles(copied: u32) -> Result<(), EvidenceFailure> {
    if copied > 0 {
        Ok(())
    } else {
        Err(EvidenceFailure::NoFileCopied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_capture_is_captured_again() {
        use EvidenceState as S;
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
                "discord_error",
                "no_file_copied"
            ]
        );
        let names: Vec<&str> = EvidenceKind::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(names, ["message", "upload"]);
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

    #[test]
    fn upload_checks() {
        let ok = OfferedFile {
            filename: "a.png",
            size_bytes: 10,
            content_type: Some("image/png"),
        };
        let big = OfferedFile {
            filename: "b.mp4",
            size_bytes: 10_485_761,
            content_type: Some("video/mp4"),
        };
        let exe = OfferedFile {
            filename: "c.exe",
            size_bytes: 1,
            content_type: None,
        };
        assert_eq!(refused_uploads(&[ok]), Vec::new());
        assert_eq!(
            refused_uploads(&[ok, big, exe]),
            vec![
                RefusedFile {
                    filename: "b.mp4".into(),
                    why: FileRefusal::TooLarge
                },
                RefusedFile {
                    filename: "c.exe".into(),
                    why: FileRefusal::UnsupportedType
                },
            ]
        );
        assert_eq!(check_upload_count(10), Ok(()));
        assert_eq!(check_upload_count(11), Err(DomainError::TooManyFiles));
        assert_eq!(check_file_room(90, 10), Ok(()));
        assert_eq!(
            check_file_room(91, 10),
            Err(DomainError::EvidenceFileLimitReached { count: 91 })
        );
    }

    #[test]
    fn upload_names() {
        assert_eq!(upload_filename("a.png"), "a.png");
        assert_eq!(upload_filename("C:\\Users\\me\\a b.png"), "a b.png");
        assert_eq!(upload_filename("../../etc/passwd"), "passwd");
        assert_eq!(upload_filename("x\n\u{0}y.txt"), "xy.txt");
        assert_eq!(upload_filename("  "), "file");
        assert_eq!(upload_filename("dir/"), "file");
        assert_eq!(upload_filename(".."), "file");
        assert_eq!(upload_filename(&"é".repeat(300)).chars().count(), 255);
    }

    #[test]
    fn notes() {
        assert_eq!(clean_note(None), Ok(None));
        assert_eq!(clean_note(Some("   ")), Ok(None));
        assert_eq!(clean_note(Some(" hi ")), Ok(Some("hi".into())));
        assert_eq!(
            clean_note(Some(&"a".repeat(500))).map(|n| n.map(|n| n.len())),
            Ok(Some(500))
        );
        assert_eq!(clean_note(Some(&"a".repeat(501))), Err(NoteTooLong));
    }

    #[test]
    fn who_sees_what() {
        use EvidenceKind::{Message, Upload};
        assert!(visible_by_default(Message));
        assert!(!visible_by_default(Upload));
        assert!(member_sees(Message, true, false));
        assert!(!member_sees(Message, true, true));
        assert!(!member_sees(Upload, false, false));
        assert!(member_sees(Upload, true, false));
        assert!(!member_sees(Upload, true, true));
        assert_eq!(check_editable(Message, false, false), Ok(()));
        assert_eq!(
            check_editable(Message, false, true),
            Err(DomainError::EvidenceNotUpload)
        );
        assert_eq!(check_editable(Upload, false, true), Ok(()));
        assert_eq!(
            check_editable(Upload, true, false),
            Err(DomainError::EvidenceRemoved)
        );
        assert_eq!(upload_settles(1), Ok(()));
        assert_eq!(upload_settles(0), Err(EvidenceFailure::NoFileCopied));
    }
}
