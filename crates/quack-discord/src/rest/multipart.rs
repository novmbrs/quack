//! `multipart/form-data` bodies for messages with files: `payload_json` plus
//! one `files[n]` part per file.

use std::fmt::Write as _;

use super::transport::Body;
use crate::model::OutgoingFile;

/// A file name safe inside a `Content-Disposition` header: no quotes,
/// backslashes, or control characters. The same name goes in the
/// attachment metadata and any `attachment://` reference, so all three
/// agree on the upload's name.
pub(crate) fn upload_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "file".to_owned()
    } else {
        cleaned
    }
}

/// A MIME type safe inside a part header: `type/subtype` with optional
/// parameters, printable ASCII only; anything else is sent as
/// `application/octet-stream`.
fn content_type(file: &OutgoingFile) -> &str {
    file.content_type
        .as_deref()
        .filter(|t| {
            t.contains('/') && t.len() <= 255 && t.bytes().all(|b| (0x20..0x7f).contains(&b))
        })
        .unwrap_or("application/octet-stream")
}

/// Builds the body. `boundary` must not occur in any part; callers pass a
/// fresh random one.
pub(crate) fn form(boundary: &str, payload_json: &[u8], files: &[&OutgoingFile]) -> Body {
    let mut bytes = Vec::with_capacity(
        payload_json.len() + files.iter().map(|f| f.bytes.len() + 256).sum::<usize>() + 256,
    );
    let mut head = String::new();
    let _ = write!(
        head,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\n\
         Content-Type: application/json\r\n\r\n"
    );
    bytes.extend_from_slice(head.as_bytes());
    bytes.extend_from_slice(payload_json);
    for (i, file) in files.iter().enumerate() {
        head.clear();
        let _ = write!(
            head,
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files[{i}]\"; \
             filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            upload_name(&file.name),
            content_type(file)
        );
        bytes.extend_from_slice(head.as_bytes());
        bytes.extend_from_slice(&file.bytes);
    }
    bytes.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Body::Multipart {
        boundary: boundary.to_owned(),
        bytes,
    }
}

/// A fresh boundary.
pub(crate) fn boundary() -> String {
    format!("quack-{}", uuid::Uuid::now_v7().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_payload_and_file_parts() {
        let file = OutgoingFile {
            name: "a\"b\r.txt".into(),
            content_type: None,
            bytes: b"hello".to_vec(),
        };
        let Body::Multipart {
            boundary: used,
            bytes,
        } = form("XYZ", br#"{"a":1}"#, &[&file])
        else {
            panic!("expected multipart");
        };
        assert_eq!(used, "XYZ");
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text,
            "--XYZ\r\nContent-Disposition: form-data; name=\"payload_json\"\r\n\
             Content-Type: application/json\r\n\r\n{\"a\":1}\r\n--XYZ\r\n\
             Content-Disposition: form-data; name=\"files[0]\"; filename=\"a_b_.txt\"\r\n\
             Content-Type: application/octet-stream\r\n\r\nhello\r\n--XYZ--\r\n"
        );
        assert_ne!(boundary(), boundary());
        assert_eq!(upload_name(""), "file");
        let typed = |t: &str| OutgoingFile {
            name: "a".into(),
            content_type: Some(t.into()),
            bytes: Vec::new(),
        };
        assert_eq!(content_type(&typed("image/png")), "image/png");
        assert_eq!(
            content_type(&typed("text/plain; charset=utf-8")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            content_type(&typed("a\r\nX-Evil: 1/2")),
            "application/octet-stream"
        );
        assert_eq!(content_type(&typed("png")), "application/octet-stream");
    }
}
