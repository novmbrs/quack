//! The dashboard's built files (§14.10), served from `QUACK_DASHBOARD_DIR`:
//! hashed `/assets/*` cached for a year, other files and the SPA's
//! `index.html` revalidated, unknown paths without an extension answered
//! with `index.html`, and missing files with a plain 404. Paths never
//! leave the directory.

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::state::ApiState;

const ASSET_CACHE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";
const DASHBOARD_CSP: &str = "default-src 'self'; script-src 'self' https://cdn.visitors.now; \
style-src 'self' 'unsafe-inline'; img-src 'self' data: https://cdn.discordapp.com \
https://media.discordapp.net; font-src 'self'; connect-src 'self' https://e.visitors.now; \
frame-ancestors 'none'; base-uri 'self'; form-action 'self'";

/// The content type of a file, by extension.
fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The file a URL path names inside `root`, or `None` when the path has
/// `..`, a root, a prefix, or anything but plain names.
fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let relative = url_path.trim_start_matches('/');
    let decoded = percent_decode(relative)?;
    if decoded.contains('\\') || decoded.contains('\0') {
        return None;
    }
    let mut out = root.to_path_buf();
    for component in Path::new(&decoded).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// Decodes `%XX` escapes; `None` for malformed escapes or non-UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "Not found",
    )
        .into_response()
}

/// Serves `path` when it is a file that is really inside `root`: a symlink
/// (to the file or a directory on its way) that leads out of the root is
/// not found.
async fn file(root: &Path, path: &Path, cache: &'static str, head: bool) -> Option<Response> {
    let real = tokio::fs::canonicalize(path).await.ok()?;
    if !real.starts_with(tokio::fs::canonicalize(root).await.ok()?) {
        return None;
    }
    let path = real.as_path();
    let meta = tokio::fs::metadata(path).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let body = if head {
        Body::empty()
    } else {
        Body::from(tokio::fs::read(path).await.ok()?)
    };
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if !head {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
    }
    Some(response)
}

/// Answers a GET or HEAD outside `/api`.
pub(crate) async fn serve(state: &ApiState, request: Request) -> Response {
    let Some(root) = state.config().dashboard_dir.as_deref() else {
        return not_found();
    };
    let head = request.method() == Method::HEAD;
    let url_path = request.uri().path();
    let Some(path) = resolve(root, url_path) else {
        return not_found();
    };
    let has_extension = Path::new(url_path)
        .file_name()
        .is_some_and(|n| Path::new(n).extension().is_some());
    if has_extension {
        let cache = if url_path.starts_with("/assets/") {
            ASSET_CACHE
        } else {
            REVALIDATE
        };
        return match file(root, &path, cache, head).await {
            Some(response) => response,
            None => not_found(),
        };
    }
    match file(root, &root.join("index.html"), REVALIDATE, head).await {
        Some(mut response) => {
            response.headers_mut().insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(DASHBOARD_CSP),
            );
            response
        }
        None => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_out_of_the_root_are_not_served() {
        let base = std::env::temp_dir().join(format!("quack-static-{}", std::process::id()));
        let (root, outside) = (base.join("dashboard"), base.join("outside"));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::fs::write(root.join("assets/app.js"), "app").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("leak.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("dir")).unwrap();
        let get = |p: &str| {
            let path = resolve(&root, p).unwrap();
            let root = root.clone();
            async move { file(&root, &path, REVALIDATE, false).await.is_some() }
        };
        assert!(get("/assets/app.js").await);
        assert!(!get("/leak.txt").await, "a file symlink out");
        assert!(!get("/dir/secret.txt").await, "a directory symlink out");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn paths_stay_inside_the_root() {
        let root = Path::new("/srv/dashboard");
        assert_eq!(
            resolve(root, "/assets/app.js"),
            Some(PathBuf::from("/srv/dashboard/assets/app.js"))
        );
        for hostile in [
            "/../etc/passwd",
            "/assets/../../etc/passwd",
            "/%2e%2e/etc/passwd",
            "/assets/%2e%2e/%2e%2e/secret",
            "/..%5c..%5cwindows",
            "/%00",
            "/%zz",
        ] {
            assert_eq!(resolve(root, hostile), None, "{hostile}");
        }
    }
}
