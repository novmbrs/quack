//! A value that must never be printed: tokens, client secrets, database
//! URLs. Wrap it at the edge where it enters Quack and expose it only where
//! it is used.

use std::fmt;

/// A secret: tokens, client secrets, database URLs. `Debug` and `Display`
/// print `[redacted]`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a secret value.
    pub const fn new(value: String) -> Self {
        Self(value)
    }

    /// The value, for the one place that uses it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn debug_and_display_never_show_the_value() {
        let s = Secret::new("Bot sentinel-token-123".to_owned());
        assert_eq!(format!("{s:?}"), "[redacted]");
        assert_eq!(format!("{s}"), "[redacted]");
        assert_eq!(format!("{:?}", Some(&s)), "Some([redacted])");
        assert_eq!(s.expose(), "Bot sentinel-token-123");
    }
}
