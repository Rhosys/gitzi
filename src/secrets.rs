//! Secure secret storage for config values that would otherwise sit in
//! plaintext in `~/.gitzi/config.toml` (provider API keys, AWS SSO tokens).
//!
//! Secrets are stored in the OS keyring (Keychain on macOS, Credential
//! Manager on Windows, Secret Service on Linux via the `keyring` crate) and
//! referenced from config.toml by a pointer string of the form
//! `keyring:<service>/<account>`. `resolve_secret` transparently follows the
//! pointer; anything that isn't a `keyring:` pointer is treated as a raw
//! value (so plaintext values from older configs still work until migrated).

use crate::error::{GitziError, Result};

const POINTER_PREFIX: &str = "keyring:";

/// Store `value` in the OS keyring under `service`/`account` and return the
/// `keyring:<service>/<account>` pointer to put in config.toml in its place.
pub fn store_secret(service: &str, account: &str, value: &str) -> Result<String> {
    let entry = keyring::Entry::new(service, account)
        .map_err(|e| GitziError::Config(format!("keyring error for {service}/{account}: {e}")))?;
    entry
        .set_password(value)
        .map_err(|e| GitziError::Config(format!("failed to store secret {service}/{account}: {e}")))?;
    Ok(format!("{POINTER_PREFIX}{service}/{account}"))
}

/// Resolve a config value to its real secret. If `value` is a `keyring:`
/// pointer, looks it up in the OS keyring. Otherwise returns it unchanged
/// (covers plaintext values from configs written before secure storage).
pub fn resolve_secret(value: &str) -> Result<String> {
    match value.strip_prefix(POINTER_PREFIX) {
        Some(rest) => {
            let (service, account) = rest.split_once('/').ok_or_else(|| {
                GitziError::Config(format!("malformed keyring pointer: {value}"))
            })?;
            let entry = keyring::Entry::new(service, account).map_err(|e| {
                GitziError::Config(format!("keyring error for {service}/{account}: {e}"))
            })?;
            entry.get_password().map_err(|e| {
                GitziError::Config(format!("keyring lookup failed for {value}: {e}"))
            })
        }
        None => Ok(value.to_string()),
    }
}

/// True if `value` is already a `keyring:` pointer (as opposed to plaintext).
pub fn is_pointer(value: &str) -> bool {
    value.starts_with(POINTER_PREFIX)
}

/// Remove the secret a pointer refers to. No-op (and no error) for plaintext
/// values or pointers that don't resolve to an existing entry.
pub fn delete_secret(value: &str) {
    if let Some(rest) = value.strip_prefix(POINTER_PREFIX)
        && let Some((service, account)) = rest.split_once('/')
        && let Ok(entry) = keyring::Entry::new(service, account)
    {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_secret_passes_through_plaintext() {
        assert_eq!(resolve_secret("sk-plain").unwrap(), "sk-plain");
    }

    #[test]
    fn is_pointer_detects_keyring_prefix() {
        assert!(is_pointer("keyring:gitzi-provider-foo/api-key"));
        assert!(!is_pointer("sk-plain"));
    }

    #[test]
    fn resolve_secret_rejects_malformed_pointer() {
        let err = resolve_secret("keyring:no-slash-here").unwrap_err();
        assert!(err.to_string().contains("malformed"));
    }
}
