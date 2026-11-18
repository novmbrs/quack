//! A configuration value that must never be printed.

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
