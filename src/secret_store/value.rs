//! Secret wrapper that never prints the raw value.

use std::fmt;

/// Opaque secret string. `Debug` / `Display` always redact the contents.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the raw secret (call sites must not log this).
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Consume and return the raw secret.
    pub fn into_exposed(self) -> String {
        self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([REDACTED])")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_contain_value() {
        let secret = SecretValue::new("super-secret-token-VALUE-9f3a");
        let dbg = format!("{secret:?}");
        let disp = format!("{secret}");
        let log_line = format!("loaded secret={secret:?} display={secret}");
        assert!(!dbg.contains("super-secret"));
        assert!(!disp.contains("super-secret"));
        assert!(!log_line.contains("super-secret-token-VALUE-9f3a"));
        assert!(dbg.contains("REDACTED"));
        assert_eq!(secret.expose(), "super-secret-token-VALUE-9f3a");
    }
}
