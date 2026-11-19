//! Where sign-in returns the browser (§14.4 step 1): a path on Quack's
//! own origin, never anywhere else.

use crate::config::PublicUrl;

/// Where sign-in lands when `redirect_to` is missing or refused.
pub(crate) const DEFAULT_LANDING: &str = "/guilds";

/// The longest `redirect_to` accepted.
const MAX_LEN: usize = 2_048;

/// Resolves `requested` against the public URL and keeps only its path,
/// query, and fragment when it stays on the public origin. Anything else
/// (another origin, a scheme change, a path that a browser would read as
/// another host, an over-long value) becomes [`DEFAULT_LANDING`].
pub(crate) fn landing(public: &PublicUrl, requested: Option<&str>) -> String {
    let Some(requested) = requested.filter(|r| !r.is_empty() && r.len() <= MAX_LEN) else {
        return DEFAULT_LANDING.to_owned();
    };
    let Ok(url) = public.url().join(requested) else {
        return DEFAULT_LANDING.to_owned();
    };
    if url.origin().ascii_serialization() != public.origin() {
        return DEFAULT_LANDING.to_owned();
    }
    let path = url.path();
    // `//host` or `/\host` in a Location header is another origin.
    if !path.starts_with('/') || path.starts_with("//") || path.starts_with("/\\") {
        return DEFAULT_LANDING.to_owned();
    }
    let mut out = path.to_owned();
    if let Some(query) = url.query() {
        out.push('?');
        out.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn land(requested: &str) -> String {
        let public = PublicUrl::parse("https://quack.example").unwrap_or_else(|_| unreachable!());
        landing(&public, Some(requested))
    }

    #[test]
    fn keeps_same_origin_paths() {
        assert_eq!(
            land("/guilds/1/cases?state=valid#top"),
            "/guilds/1/cases?state=valid#top"
        );
        assert_eq!(land("https://quack.example/guilds/1"), "/guilds/1");
        assert_eq!(land("guilds/1"), "/guilds/1");
    }

    #[test]
    fn refuses_other_origins() {
        for hostile in [
            "https://evil.example/",
            "//evil.example/x",
            "/\\evil.example",
            "\\\\evil.example",
            "http://quack.example/guilds",
            "javascript:alert(1)",
            "https://quack.example@evil.example/",
            "https://quack.example.evil.example/",
            "https://quack.example//evil.example/x",
        ] {
            assert_eq!(land(hostile), DEFAULT_LANDING, "{hostile}");
        }
        // Odd but same-origin: kept as paths a browser can't read as a host.
        for odd in ["https:evil.example", "/%2F%2Fevil.example"] {
            let landed = land(odd);
            assert!(
                landed.starts_with('/') && !landed.starts_with("//"),
                "{odd} → {landed}"
            );
        }
        let long = format!("/{}", "a".repeat(2_048));
        assert_eq!(land(&long), DEFAULT_LANDING);
    }
}
