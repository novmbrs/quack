//! Reading route 60's `multipart/form-data` body part by part (§14.8): up
//! to 10 `files` parts and one `note`. A file is buffered only while it can
//! still be accepted: past 10 MiB, of a type Quack doesn't copy, or beyond
//! the tenth, its bytes are read and dropped and the refusal is recorded,
//! and once anything is refused every buffer is dropped. The rest of the
//! body is still read (within the request cap), so the browser gets the
//! answer that names every refused file instead of a reset connection.
//! Everything the client names is bounded before it is kept: each part's
//! headers, unknown field names, and file names.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use quack_core::evidence::{FileRefusal, RefusedFile, copy_eligibility};
use quack_core::limits::{MAX_COPY_BYTES, MAX_UPLOAD_BODY_BYTES, MAX_UPLOAD_FILES, truncate_chars};
use quack_engine::evidence::NewUpload;
use quack_text::api::ApiCode;

use crate::error::ApiError;

/// The longest a `note` part may be, in bytes (500 characters of 4 bytes
/// each, with room to spare); longer is `note_too_long`.
const MAX_NOTE_BYTES: usize = 4_096;

/// How long the body may send nothing before the upload is given up
/// (`503 unavailable`; nothing was saved).
pub(super) const IDLE: Duration = Duration::from_secs(30);

/// How long reading the whole body may take, however steadily it arrives
/// (`503 unavailable`; nothing was saved), so a slow body can't hold the
/// upload budget for long: 15 minutes, which a full 104,923,136-byte body
/// still makes at about 117 KB/s (under 1 Mbit/s).
pub(super) const DEADLINE: Duration = Duration::from_mins(15);

/// The most bytes one part's headers (with the boundary before them) may
/// take; browsers send a few hundred.
const MAX_PART_HEADER_BYTES: usize = 8_192;

/// The body reaches the parser in pieces of at most this many bytes, so
/// what it reads past a part's headers is bounded too.
const PIECE: usize = 8_192;

/// The longest file name kept from a part, in characters (stored names are
/// cut to 255 later, after their last path segment is taken).
const MAX_FILENAME_CHARS: usize = 1_024;

/// The longest unknown field name an error repeats; a longer or unusual
/// one is reported as `form`.
const MAX_FIELD_NAME_CHARS: usize = 64;

/// What the form held.
#[derive(Debug, Default)]
pub(super) struct Form {
    /// The accepted files, in order.
    pub files: Vec<NewUpload>,
    /// The `note` part's text.
    pub note: Option<String>,
}

/// Why the form can't be used; each maps to one §14.6 answer.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum FormError {
    /// More than 10 `files` parts (`422 too_many_files`).
    TooManyFiles,
    /// Files Quack won't copy, every one named (`400`, `files`).
    Refused(Vec<RefusedFile>),
    /// The `note` part is longer than a note can be.
    NoteTooLong,
    /// A part other than `files` or `note`, or a second `note`.
    Unexpected(String),
    /// The body went past the upload cap (`413 body_too_large`).
    TooLarge,
    /// The body sent nothing for [`IDLE`] (`503 unavailable`).
    Stalled,
    /// Not a readable multipart body (`400 invalid_json`).
    Unreadable,
}

impl From<multer::Error> for FormError {
    fn from(e: multer::Error) -> Self {
        match e {
            multer::Error::StreamSizeExceeded { .. } => Self::TooLarge,
            multer::Error::UnknownField { field_name } => {
                Self::Unexpected(field_name.as_deref().map_or_default(field_label))
            }
            _ => Self::Unreadable,
        }
    }
}

/// An unknown field's name as an error repeats it: short names of letters,
/// digits, `_`, `-`, `[`, `]`, and `.`, else `form`.
fn field_label(name: &str) -> String {
    let plain = name.chars().count() <= MAX_FIELD_NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '[' | ']' | '.'));
    if plain && !name.is_empty() {
        name.to_owned()
    } else {
        "form".to_owned()
    }
}

/// Watches what the parser reads while it looks for a part's headers.
#[derive(Debug, Default)]
struct Gate {
    /// The parser is reading a boundary and the headers after it.
    headers: AtomicBool,
    /// Bytes read since it started.
    read: AtomicUsize,
    /// A part's headers went past [`MAX_PART_HEADER_BYTES`].
    tripped: AtomicBool,
}

impl Gate {
    fn open(&self, headers: bool) {
        self.read.store(0, Ordering::Relaxed);
        self.headers.store(headers, Ordering::Relaxed);
    }
}

/// A part's headers were longer than [`MAX_PART_HEADER_BYTES`].
#[derive(Debug, thiserror::Error)]
#[error("part headers too large")]
struct HeadersTooLarge;

/// The body as the parser reads it: one piece of at most [`PIECE`] bytes
/// per poll (`multer` reads whatever is ready, so after each piece this
/// answers "not yet" once and wakes itself), failing once a part's headers
/// (one piece of slack included) pass [`MAX_PART_HEADER_BYTES`]. `multer`
/// buffers headers until they end, so this is what bounds them.
struct Guarded {
    inner: BodyDataStream,
    gate: Arc<Gate>,
    rest: Bytes,
    yielded: bool,
}

impl Stream for Guarded {
    type Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if std::mem::take(&mut this.yielded) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        while this.rest.is_empty() {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => this.rest = bytes,
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(Box::new(e)))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
        let piece = this.rest.split_to(this.rest.len().min(PIECE));
        if this.gate.headers.load(Ordering::Relaxed) {
            let read = this.gate.read.fetch_add(piece.len(), Ordering::Relaxed) + piece.len();
            if read > MAX_PART_HEADER_BYTES + PIECE {
                this.gate.tripped.store(true, Ordering::Relaxed);
                return Poll::Ready(Some(Err(Box::new(HeadersTooLarge))));
            }
        }
        this.yielded = true;
        Poll::Ready(Some(Ok(piece)))
    }
}

impl FormError {
    /// The API error for routes that don't need the case to word it.
    pub(super) fn api(self) -> Option<ApiError> {
        match self {
            Self::Unexpected(name) => Some(super::invalid(if name.is_empty() {
                "files"
            } else {
                &name
            })),
            Self::TooLarge => Some(ApiError::api(ApiCode::BodyTooLarge)),
            Self::Stalled => Some(ApiError::api(ApiCode::Unavailable)),
            Self::Unreadable => Some(ApiError::api(ApiCode::InvalidJson)),
            Self::TooManyFiles | Self::Refused(_) | Self::NoteTooLong => None,
        }
    }
}

/// The boundary of a `multipart/form-data` content type.
pub(super) fn boundary(content_type: Option<&str>) -> Option<String> {
    let content_type = content_type?;
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    if !essence.eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }
    multer::parse_boundary(content_type).ok()
}

/// One chunk of a part, or the end of it, within [`IDLE`].
async fn chunk(field: &mut multer::Field<'_>) -> Result<Option<Bytes>, FormError> {
    match tokio::time::timeout(IDLE, field.chunk()).await {
        Ok(read) => Ok(read?),
        Err(_) => Err(FormError::Stalled),
    }
}

/// Reads the rest of a part without keeping it.
async fn drain(field: &mut multer::Field<'_>) -> Result<(), FormError> {
    while chunk(field).await?.is_some() {}
    Ok(())
}

/// Reads the rest of a part without keeping it, counting its size.
async fn measure(field: &mut multer::Field<'_>) -> Result<u64, FormError> {
    let mut size: u64 = 0;
    while let Some(bytes) = chunk(field).await? {
        size += bytes.len() as u64;
    }
    Ok(size)
}

/// Reads one file part: its bytes while it stays within 10 MiB, else
/// `None` (the rest is read and dropped).
async fn file_bytes(field: &mut multer::Field<'_>) -> Result<Option<Bytes>, FormError> {
    let mut parts: Vec<Bytes> = Vec::new();
    let mut size: u64 = 0;
    while let Some(bytes) = chunk(field).await? {
        size += bytes.len() as u64;
        if size > MAX_COPY_BYTES {
            drop(parts);
            drain(field).await?;
            return Ok(None);
        }
        parts.push(bytes);
    }
    // One chunk is kept as it is; several are joined once, at their size.
    Ok(Some(match parts.len() {
        0 => Bytes::new(),
        1 => parts.swap_remove(0),
        _ => {
            let mut joined = BytesMut::with_capacity(usize::try_from(size).unwrap_or(0));
            for p in parts {
                joined.extend_from_slice(&p);
            }
            joined.freeze()
        }
    }))
}

/// Reads the `note` part, at most [`MAX_NOTE_BYTES`].
async fn note_text(field: &mut multer::Field<'_>) -> Result<String, FormError> {
    let mut text = Vec::new();
    while let Some(bytes) = chunk(field).await? {
        if text.len() + bytes.len() > MAX_NOTE_BYTES {
            drain(field).await?;
            return Err(FormError::NoteTooLong);
        }
        text.extend_from_slice(&bytes);
    }
    // A browser sends a form's line breaks as CRLF; a note counts them as one.
    String::from_utf8(text)
        .map(|t| t.replace("\r\n", "\n"))
        .map_err(|_| FormError::Unexpected("note".to_owned()))
}

/// Reads the whole form (§14.8) within [`DEADLINE`]. Refusals that name
/// files are collected to the end of the body; the others end reading at
/// once.
pub(super) async fn read(body: Body, boundary: &str) -> Result<Form, FormError> {
    read_within(body, boundary, DEADLINE).await
}

async fn read_within(body: Body, boundary: &str, deadline: Duration) -> Result<Form, FormError> {
    let limit = multer::SizeLimit::new().whole_stream(MAX_UPLOAD_BODY_BYTES as u64);
    let constraints = multer::Constraints::new()
        .allowed_fields(vec!["files", "note"])
        .size_limit(limit);
    let gate = Arc::new(Gate::default());
    let stream = Guarded {
        inner: body.into_data_stream(),
        gate: Arc::clone(&gate),
        rest: Bytes::new(),
        yielded: false,
    };
    let multipart = multer::Multipart::with_constraints(stream, boundary, constraints);
    tokio::time::timeout(deadline, parts(multipart, &gate))
        .await
        .unwrap_or(Err(FormError::Stalled))
}

/// The next part, with its headers bounded by `gate`.
async fn next_part<'r>(
    multipart: &mut multer::Multipart<'r>,
    gate: &Gate,
) -> Result<Option<multer::Field<'r>>, FormError> {
    gate.open(true);
    let next = tokio::time::timeout(IDLE, multipart.next_field()).await;
    gate.open(false);
    match next {
        Ok(Ok(field)) => Ok(field),
        Ok(Err(_)) if gate.tripped.load(Ordering::Relaxed) => Err(FormError::Unreadable),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(FormError::Stalled),
    }
}

async fn parts(mut multipart: multer::Multipart<'_>, gate: &Gate) -> Result<Form, FormError> {
    let mut form = Form::default();
    let mut seen_files = 0usize;
    let mut refused = Vec::new();
    let mut note_seen = false;
    let mut note_too_long = false;
    while let Some(mut field) = next_part(&mut multipart, gate).await? {
        if field.name() == Some("note") {
            if note_seen {
                return Err(FormError::Unexpected("note".to_owned()));
            }
            note_seen = true;
            match note_text(&mut field).await {
                Ok(text) => form.note = Some(text),
                Err(FormError::NoteTooLong) => note_too_long = true,
                Err(e) => return Err(e),
            }
            continue;
        }
        seen_files += 1;
        let filename =
            truncate_chars(field.file_name().unwrap_or_default(), MAX_FILENAME_CHARS).to_owned();
        let content_type = field.content_type().map(|m| m.essence_str().to_owned());
        let wanted = seen_files <= MAX_UPLOAD_FILES && refused.is_empty();
        // Past the tenth the answer is `too_many_files`, which names none.
        let named = seen_files <= MAX_UPLOAD_FILES;
        if copy_eligibility(0, content_type.as_deref()).is_err() {
            if named {
                refused.push(RefusedFile {
                    filename,
                    why: FileRefusal::UnsupportedType,
                });
            }
            drain(&mut field).await?;
        } else if !wanted {
            // Not kept, but still checked for size so the answer names it.
            if measure(&mut field).await? > MAX_COPY_BYTES && named {
                refused.push(RefusedFile {
                    filename,
                    why: FileRefusal::TooLarge,
                });
            }
        } else {
            match file_bytes(&mut field).await? {
                Some(bytes) => form
                    .files
                    .push(NewUpload::bytes(filename, content_type, bytes)),
                None => refused.push(RefusedFile {
                    filename,
                    why: FileRefusal::TooLarge,
                }),
            }
        }
        if !refused.is_empty() || seen_files > MAX_UPLOAD_FILES {
            // Nothing will be saved: keep no bytes.
            form.files.clear();
        }
    }
    if seen_files > MAX_UPLOAD_FILES {
        return Err(FormError::TooManyFiles);
    }
    if note_too_long {
        return Err(FormError::NoteTooLong);
    }
    if !refused.is_empty() {
        return Err(FormError::Refused(refused));
    }
    Ok(form)
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: &str = "XyZ";

    fn part(name: &str, filename: Option<&str>, content_type: &str, data: &[u8]) -> Vec<u8> {
        let file = filename.map_or_default(|f| format!("; filename=\"{f}\""));
        let mut out = format!(
            "--{B}\r\nContent-Disposition: form-data; name=\"{name}\"{file}\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
        out
    }

    fn body(parts: &[Vec<u8>]) -> Body {
        let mut out: Vec<u8> = parts.concat();
        out.extend_from_slice(format!("--{B}--\r\n").as_bytes());
        Body::from(out)
    }

    #[test]
    fn boundaries() {
        assert_eq!(
            boundary(Some("multipart/form-data; boundary=abc")).as_deref(),
            Some("abc")
        );
        assert_eq!(boundary(Some("application/json")), None);
        assert_eq!(boundary(Some("multipart/form-data")), None);
        assert_eq!(boundary(None), None);
    }

    #[tokio::test]
    async fn files_and_a_note() {
        let form = read(
            body(&[
                part("files", Some("a.png"), "image/png", b"png!"),
                part("note", None, "text/plain", "Reported by DM".as_bytes()),
                part(
                    "files",
                    Some("b.txt"),
                    "text/plain; charset=utf-8",
                    b"hello",
                ),
            ]),
            B,
        )
        .await
        .unwrap();
        assert_eq!(form.note.as_deref(), Some("Reported by DM"));
        assert_eq!(form.files.len(), 2);
        assert_eq!(form.files[0].filename, "a.png");
        assert_eq!(form.files[1].content_type.as_deref(), Some("text/plain"));
        assert_eq!(form.files[1].size_bytes, 5);
    }

    #[tokio::test]
    async fn every_refused_file_is_named() {
        let big = vec![0u8; usize::try_from(MAX_COPY_BYTES).unwrap() + 1];
        let err = read(
            body(&[
                part("files", Some("clip.mp4"), "video/mp4", &big),
                part("files", Some("ok.png"), "image/png", b"ok"),
                part(
                    "files",
                    Some("setup.exe"),
                    "application/octet-stream",
                    b"MZ",
                ),
            ]),
            B,
        )
        .await
        .unwrap_err();
        assert_eq!(
            err,
            FormError::Refused(vec![
                RefusedFile {
                    filename: "clip.mp4".to_owned(),
                    why: FileRefusal::TooLarge
                },
                RefusedFile {
                    filename: "setup.exe".to_owned(),
                    why: FileRefusal::UnsupportedType
                },
            ])
        );
    }

    #[tokio::test]
    async fn eleven_files_are_too_many() {
        let parts: Vec<Vec<u8>> = (0..11)
            .map(|i| part("files", Some(&format!("{i}.png")), "image/png", b"x"))
            .collect();
        assert_eq!(
            read(body(&parts), B).await.unwrap_err(),
            FormError::TooManyFiles
        );
    }

    #[tokio::test]
    async fn notes_and_unknown_parts() {
        let long = "n".repeat(MAX_NOTE_BYTES + 1);
        let err = read(
            body(&[
                part("note", None, "text/plain", long.as_bytes()),
                part("files", Some("a.png"), "image/png", b"x"),
            ]),
            B,
        )
        .await
        .unwrap_err();
        assert_eq!(err, FormError::NoteTooLong);
        let err = read(body(&[part("links", None, "text/plain", b"x")]), B)
            .await
            .unwrap_err();
        assert_eq!(err, FormError::Unexpected("links".to_owned()));
        let err = read(Body::from("not multipart"), B).await.unwrap_err();
        assert_eq!(err, FormError::Unreadable);
    }

    /// A body that arrives through a channel, a piece at a time.
    struct Trickle(tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>);

    impl Stream for Trickle {
        type Item = Result<Bytes, std::io::Error>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.0.poll_recv(cx)
        }
    }

    /// Review E: a body that keeps sending, never idle for long, is still
    /// given up at the overall deadline.
    #[tokio::test]
    async fn a_slow_body_is_cut_at_the_deadline() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let head = format!(
            "--{B}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n"
        );
        tokio::spawn(async move {
            if tx.send(Ok(Bytes::from(head))).await.is_err() {
                return;
            }
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if tx.send(Ok(Bytes::from_static(b"x"))).await.is_err() {
                    return;
                }
            }
        });
        let started = std::time::Instant::now();
        let err = read_within(
            Body::from_stream(Trickle(rx)),
            B,
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
        assert_eq!(err, FormError::Stalled);
        assert!(started.elapsed() < IDLE);
    }

    /// Review F: part headers, unknown field names, and file names are
    /// bounded before anything keeps or repeats them.
    #[tokio::test]
    async fn names_and_headers_are_bounded() {
        // Headers longer than the bound (here a long file name).
        let long = "a".repeat(MAX_PART_HEADER_BYTES + PIECE + 1);
        let err = read(body(&[part("files", Some(&long), "image/png", b"x")]), B)
            .await
            .unwrap_err();
        assert_eq!(err, FormError::Unreadable);
        // An unknown field with a long or odd name isn't repeated.
        let name = "n".repeat(MAX_FIELD_NAME_CHARS + 1);
        let err = read(body(&[part(&name, None, "text/plain", b"x")]), B)
            .await
            .unwrap_err();
        assert_eq!(err, FormError::Unexpected("form".to_owned()));
        let err = read(body(&[part("<b>", None, "text/plain", b"x")]), B)
            .await
            .unwrap_err();
        assert_eq!(err, FormError::Unexpected("form".to_owned()));
        // A long file name is cut before it is named in an answer.
        let named = "f".repeat(MAX_FILENAME_CHARS + 500);
        let err = read(
            body(&[part(
                "files",
                Some(&named),
                "application/x-msdownload",
                b"MZ",
            )]),
            B,
        )
        .await
        .unwrap_err();
        let FormError::Refused(files) = err else {
            panic!("{err:?}");
        };
        assert_eq!(files[0].filename.chars().count(), MAX_FILENAME_CHARS);
        // Past the tenth file nothing more is named (the answer is
        // `too_many_files`).
        let parts: Vec<Vec<u8>> = (0..40)
            .map(|i| {
                part(
                    "files",
                    Some(&format!("{i}.exe")),
                    "application/x-msdownload",
                    b"x",
                )
            })
            .collect();
        assert_eq!(
            read(body(&parts), B).await.unwrap_err(),
            FormError::TooManyFiles
        );
    }
}
