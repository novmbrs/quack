//! The session and OAuth cookies (§14.4): `__Host-` names on `https`,
//! plain names on loopback `http`; always `HttpOnly; SameSite=Lax;
//! Path=/`, `Secure` on `https`, never a `Domain`.

use axum::http::{HeaderMap, HeaderValue, header};

use crate::config::PublicUrl;

/// Which of Quack's two cookies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cookie {
    /// The signed-in session's ID.
    Session,
    /// The browser nonce of a sign-in in progress.
    Oauth,
}

impl Cookie {
    /// The cookie's name on this deployment.
    pub(crate) fn name(self, public: &PublicUrl) -> &'static str {
        match (self, public.is_https()) {
            (Self::Session, true) => "__Host-quack_session",
            (Self::Session, false) => "quack_session",
            (Self::Oauth, true) => "__Host-quack_oauth",
            (Self::Oauth, false) => "quack_oauth",
        }
    }

    /// The `Set-Cookie` value that stores `value` for `max_age` seconds.
    pub(crate) fn set(self, public: &PublicUrl, value: &str, max_age: i64) -> Option<HeaderValue> {
        let secure = if public.is_https() { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{}={value}; Max-Age={}; Path=/; HttpOnly; SameSite=Lax{secure}",
            self.name(public),
            max_age.max(0)
        ))
        .ok()
    }

    /// The `Set-Cookie` value that deletes the cookie.
    pub(crate) fn clear(self, public: &PublicUrl) -> Option<HeaderValue> {
        self.set(public, "", 0)
    }

    /// The cookie's value in the request, if sent (the first one, when a
    /// browser sends duplicates).
    pub(crate) fn read(self, public: &PublicUrl, headers: &HeaderMap) -> Option<String> {
        let name = self.name(public);
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_cookies_are_host_prefixed_and_secure() {
        let public = PublicUrl::parse("https://quack.example").unwrap_or_else(|_| unreachable!());
        let set = Cookie::Session.set(&public, "abc", 60);
        assert_eq!(
            set.as_ref().and_then(|v| v.to_str().ok()),
            Some("__Host-quack_session=abc; Max-Age=60; Path=/; HttpOnly; SameSite=Lax; Secure")
        );
        let local = PublicUrl::parse("http://localhost:8080").unwrap_or_else(|_| unreachable!());
        assert_eq!(Cookie::Oauth.name(&local), "quack_oauth");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; __Host-quack_session=xyz; b=2"),
        );
        assert_eq!(
            Cookie::Session.read(&public, &headers).as_deref(),
            Some("xyz")
        );
        assert_eq!(Cookie::Session.read(&local, &headers), None);
    }
}
