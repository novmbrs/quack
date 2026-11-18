//! `multipart/form-data` bodies for messages with files: `payload_json` plus
//! one `files[n]` part per file.

use std::fmt::Write as _;

use super::transport::Body;
use crate::model::OutgoingFile;

/// A file name safe inside a `Content-Disposition` header: no quotes,
/// backslashes, or control characters.
fn header_safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '"' || c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    if cleaned.is_empty() {
        "file".to_owned()
    } else {
        cleaned
    }
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
             filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
            header_safe(&file.name)
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
            bytes: b"hello".to_vec(),
        };
        let Body::Multipart { boundary: used, bytes } = form("XYZ", br#"{"a":1}"#, &[&file]) else {
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
        assert_eq!(header_safe(""), "file");
    }
}
