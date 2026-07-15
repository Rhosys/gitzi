//! Secure secret storage for config values that would otherwise sit in
//! plaintext in `~/.gitzi/config.toml` (provider API keys, cloud SSO/OAuth
//! tokens).
//!
//! Secrets are stored in the OS keyring (Keychain on macOS, Credential
//! Manager on Windows, Secret Service on Linux via the `keyring` crate) and
//! referenced from config.toml by a pointer string of the form
//! `keyring:<service>/<account>`. `resolve_secret` transparently follows the
//! pointer; anything that isn't a `keyring:` pointer is treated as a raw
//! value (so plaintext values from older configs still work until migrated).
//!
//! None of the three backing stores support real hierarchy or structured
//! values — each is just a flat `(service, account) -> secret string` map
//! (Linux's Secret Service has "collections", but the `keyring` crate doesn't
//! expose them, and Windows/macOS have no equivalent). So every entry must
//! follow one naming convention, built with [`service_name`]:
//!
//! ```text
//! service = "gitzi/<domain>/<kind>"   e.g. "gitzi/aws/sso-token", "gitzi/llm/api-key"
//! account = <natural unique resource id>   e.g. an SSO start URL, a tenant ID, a provider name
//! ```
//!
//! `domain` is the cloud or category (`aws`, `gcp`, `azure`, `llm`, ...);
//! `kind` distinguishes secrets with different lifetimes or purposes within
//! that domain (e.g. a short-lived SSO access token vs. a 90-day-lived OIDC
//! client registration). Keeping `account` tied to the resource's own
//! identity — not gitzi's local provider name — lets multiple gitzi-config
//! providers that share one underlying login (e.g. two Bedrock providers on
//! the same SSO org) share one cached credential instead of duplicating it.
//! When one logical secret has multiple fields (a token plus its expiry, a
//! client ID plus its secret), serialize them together as one JSON blob
//! under a single entry rather than splitting across several.

use crate::error::{GitziError, Result};

const POINTER_PREFIX: &str = "keyring:";

/// Build a `service` string per gitzi's keyring naming convention — see the
/// module docs. Use this instead of hand-rolling service strings so every
/// secret type stays self-describing and consistent across clouds/providers.
pub fn service_name(domain: &str, kind: &str) -> String {
    format!("gitzi/{domain}/{kind}")
}

/// Store `value` in the OS keyring under `service`/`account` and return the
/// `keyring:<service>/<account>` pointer to put in config.toml in its place.
pub fn store_secret(service: &str, account: &str, value: &str) -> Result<String> {
    let entry = keyring::Entry::new(service, account)
        .map_err(|e| GitziError::Config(format!("keyring error for {service}/{account}: {e}")))?;
    entry.set_password(value).map_err(|e| {
        GitziError::Config(format!("failed to store secret {service}/{account}: {e}"))
    })?;
    Ok(format!("{POINTER_PREFIX}{service}/{account}"))
}

/// Resolve a config value to its real secret. If `value` is a `keyring:`
/// pointer, looks it up in the OS keyring. Otherwise returns it unchanged
/// (covers plaintext values from configs written before secure storage).
pub fn resolve_secret(value: &str) -> Result<String> {
    match value.strip_prefix(POINTER_PREFIX) {
        Some(rest) => {
            let (service, account) = rest
                .split_once('/')
                .ok_or_else(|| GitziError::Config(format!("malformed keyring pointer: {value}")))?;
            let entry = keyring::Entry::new(service, account).map_err(|e| {
                GitziError::Config(format!("keyring error for {service}/{account}: {e}"))
            })?;
            entry
                .get_password()
                .map_err(|e| GitziError::Config(format!("keyring lookup failed for {value}: {e}")))
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
