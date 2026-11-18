//! Hashing, random tokens, constant-time comparison, and sealing the
//! Discord access token kept in a session (§14.4).

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use quack_core::ids::AccountId;

use crate::config::SessionSecret;

/// SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// `n` random bytes from the system generator.
fn random<const N: usize>() -> Option<[u8; N]> {
    let mut out = [0_u8; N];
    aws_lc_rs::rand::fill(&mut out).ok()?;
    Some(out)
}

/// 32 random bytes as base64url (43 characters): session IDs, CSRF
/// tokens, OAuth states, and browser nonces.
pub(crate) fn token() -> Option<String> {
    random::<32>().map(|b| URL_SAFE_NO_PAD.encode(b))
}

/// A PKCE verifier of 64 characters (48 random bytes as base64url).
pub(crate) fn pkce_verifier() -> Option<String> {
    random::<48>().map(|b| URL_SAFE_NO_PAD.encode(b))
}

/// The SHA-256 of a base64url token as sent by the browser; `None` when
/// it isn't base64url of 32 bytes (so a garbage cookie never reaches SQL).
pub(crate) fn token_hash(token: &str) -> Option<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD.decode(token.as_bytes()).ok()?;
    (bytes.len() == 32).then(|| sha256(&bytes))
}

/// Whether two secrets are equal, in time independent of where they differ.
pub(crate) fn same(a: &str, b: &str) -> bool {
    aws_lc_rs::constant_time::verify_slices_are_equal(a.as_bytes(), b.as_bytes()).is_ok()
}

fn key(secret: &SessionSecret) -> Option<LessSafeKey> {
    UnboundKey::new(&AES_256_GCM, secret.bytes())
        .ok()
        .map(LessSafeKey::new)
}

fn aad(id_hash: &[u8; 32], account: AccountId) -> Vec<u8> {
    let mut aad = id_hash.to_vec();
    aad.extend_from_slice(&account.get().to_be_bytes());
    aad
}

/// Seals a Discord access token under the session secret: a fresh nonce,
/// then the ciphertext and tag. The session's ID hash and account are the
/// associated data, so a sealed token can't be moved to another session.
pub(crate) fn seal(
    secret: &SessionSecret,
    id_hash: &[u8; 32],
    account: AccountId,
    access_token: &str,
) -> Option<Vec<u8>> {
    let key = key(secret)?;
    let nonce_bytes = random::<NONCE_LEN>()?;
    let mut sealed = access_token.as_bytes().to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce_bytes),
        Aad::from(aad(id_hash, account)),
        &mut sealed,
    )
    .ok()?;
    let mut out = nonce_bytes.to_vec();
    out.extend_from_slice(&sealed);
    Some(out)
}

/// Opens a token sealed by [`seal`]; `None` if it was sealed under another
/// secret, for another session, or tampered with.
pub(crate) fn open(
    secret: &SessionSecret,
    id_hash: &[u8; 32],
    account: AccountId,
    sealed: &[u8],
) -> Option<String> {
    let key = key(secret)?;
    let (nonce, body) = sealed.split_at_checked(NONCE_LEN)?;
    let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut body = body.to_vec();
    let plain = key
        .open_in_place(nonce, Aad::from(aad(id_hash, account)), &mut body)
        .ok()?;
    String::from_utf8(plain.to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_tokens_open_only_in_their_session() {
        let secret = SessionSecret::new([7; 32]);
        let other = SessionSecret::new([8; 32]);
        let account = AccountId::new(42).unwrap_or_else(|| unreachable!());
        let id = sha256(b"session");
        let sealed = seal(&secret, &id, account, "discord-token").unwrap_or_default();
        assert_eq!(
            open(&secret, &id, account, &sealed).as_deref(),
            Some("discord-token")
        );
        assert_eq!(open(&other, &id, account, &sealed), None);
        assert_eq!(open(&secret, &sha256(b"other"), account, &sealed), None);
    }

    #[test]
    fn tokens_are_43_characters_and_hash_back() {
        let t = token().unwrap_or_default();
        assert_eq!(t.len(), 43);
        assert!(token_hash(&t).is_some());
        assert!(token_hash("not a token").is_none());
        assert_eq!(pkce_verifier().map(|v| v.len()), Some(64));
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
    }
}
