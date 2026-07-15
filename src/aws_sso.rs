//! AWS IAM Identity Center (SSO) login, mirroring `aws sso login`'s device
//! authorization flow, plus the account/role listing and role-credential
//! exchange needed to turn a logged-in SSO session into temporary AWS
//! credentials for Bedrock.
//!
//! The SSO access token is cached in the OS keyring (via `crate::secrets`)
//! keyed by the SSO start URL, so gitzi only needs to re-open a browser when
//! that token actually expires — its lifetime is set by the org's IAM
//! Identity Center session-duration setting (15 minutes to 90 days, 8 hours
//! by default), not by gitzi. The OIDC client registration (`client_id`/
//! `client_secret` from `register_client`) is cached the same way, keyed by
//! start URL, since AWS fixes its lifetime at 90 days and recommends reusing
//! it rather than re-registering on every login. Short-lived role credentials
//! are never cached on disk — [`SsoCredentialsProvider`] re-exchanges them via
//! `get_role_credentials` whenever the AWS SDK's own identity cache decides
//! the previous set has expired.
//!
//! gitzi never writes to `~/.aws/config`/`~/.aws/credentials` or any other
//! cloud CLI's config files — [`SsoCredentialsProvider`] hands credentials to
//! the AWS SDK directly, in-process, via `aws_config`'s
//! `credentials_provider()` builder method.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::{Duration, UNIX_EPOCH};

use aws_credential_types::Credentials as AwsCredentials;
use aws_credential_types::provider::{ProvideCredentials, error::CredentialsError, future};

use crate::error::{GitziError, Result};
use crate::secrets;

const CLIENT_NAME: &str = "gitzi";
// `secrets::service_name` isn't `const fn`, so these mirror its output
// ("gitzi/<domain>/<kind>") literally rather than calling it.
const SSO_TOKEN_SERVICE: &str = "gitzi/aws/sso-token";
const SSO_CLIENT_SERVICE: &str = "gitzi/aws/sso-client";

/// A cached SSO access token plus its expiry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsoToken {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

impl SsoToken {
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

/// A cached OIDC client registration from `register_client`. AWS fixes
/// `clientSecretExpiresAt` at 90 days regardless of the org's session-duration
/// setting, and recommends persisting it for reuse rather than re-registering
/// on every login.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientRegistration {
    client_id: String,
    client_secret: String,
    expires_at: DateTime<Utc>,
}

impl ClientRegistration {
    fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

/// Account assigned to the logged-in user, as returned by `list_accounts`.
#[derive(Debug, Clone)]
pub struct SsoAccount {
    pub account_id: String,
    pub account_name: String,
    pub email_address: String,
}

/// A permission-set role assigned to the user within one account.
#[derive(Debug, Clone)]
pub struct SsoRole {
    pub role_name: String,
    pub account_id: String,
}

/// Temporary AWS credentials exchanged for an SSO role.
#[derive(Debug, Clone)]
pub struct RoleCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    /// Milliseconds since the Unix epoch, matching the SSO API's wire format.
    pub expiration_ms: i64,
}

/// Device-authorization state returned by `start_device_login`; complete the
/// flow by passing it to `poll_for_token`.
pub struct PendingLogin {
    client_id: String,
    client_secret: String,
    device_code: String,
    interval_secs: u64,
    expires_at: DateTime<Utc>,
    region: String,
    pub user_code: String,
    pub verification_uri_complete: String,
}

async fn sdk_config_no_creds(region: &str) -> aws_config::SdkConfig {
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .no_credentials()
        .load()
        .await
}

async fn ssooidc_client(region: &str) -> aws_sdk_ssooidc::Client {
    aws_sdk_ssooidc::Client::new(&sdk_config_no_creds(region).await)
}

async fn sso_client(region: &str) -> aws_sdk_sso::Client {
    aws_sdk_sso::Client::new(&sdk_config_no_creds(region).await)
}

/// Start the SSO device-authorization flow: register an OIDC client with
/// the service, request a device code for `start_url`, and open the
/// verification URL in the user's default browser. Returns a `PendingLogin`
/// to complete via `poll_for_token`; show `user_code` to the user in case
/// the browser doesn't open automatically.
pub async fn start_device_login(region: &str, start_url: &str) -> Result<PendingLogin> {
    let client = ssooidc_client(region).await;

    let (client_id, client_secret) = match load_client_registration(start_url) {
        Some(reg) => (reg.client_id, reg.client_secret),
        None => {
            let registered = client
                .register_client()
                .client_name(CLIENT_NAME)
                .client_type("public")
                .scopes("sso:account:access")
                .send()
                .await
                .map_err(|e| GitziError::Config(format!("AWS SSO register_client failed: {e}")))?;

            let client_id = registered
                .client_id()
                .ok_or_else(|| {
                    GitziError::Config("AWS SSO register_client returned no client_id".into())
                })?
                .to_string();
            let client_secret = registered
                .client_secret()
                .ok_or_else(|| {
                    GitziError::Config("AWS SSO register_client returned no client_secret".into())
                })?
                .to_string();
            let expires_at = DateTime::from_timestamp(registered.client_secret_expires_at(), 0)
                .ok_or_else(|| {
                    GitziError::Config("AWS SSO register_client returned an invalid expiry".into())
                })?;

            store_client_registration(
                start_url,
                &ClientRegistration {
                    client_id: client_id.clone(),
                    client_secret: client_secret.clone(),
                    expires_at,
                },
            )?;

            (client_id, client_secret)
        }
    };

    let device_auth = client
        .start_device_authorization()
        .client_id(&client_id)
        .client_secret(&client_secret)
        .start_url(start_url)
        .send()
        .await
        .map_err(|e| {
            GitziError::Config(format!("AWS SSO start_device_authorization failed: {e}"))
        })?;

    let device_code = device_auth
        .device_code()
        .ok_or_else(|| {
            GitziError::Config("AWS SSO start_device_authorization returned no device_code".into())
        })?
        .to_string();
    let user_code = device_auth.user_code().unwrap_or_default().to_string();
    let verification_uri_complete = device_auth
        .verification_uri_complete()
        .or_else(|| device_auth.verification_uri())
        .unwrap_or_default()
        .to_string();
    let interval_secs = device_auth.interval().max(1) as u64;
    let expires_at = Utc::now() + chrono::Duration::seconds(device_auth.expires_in() as i64);

    let _ = webbrowser::open(&verification_uri_complete);

    Ok(PendingLogin {
        client_id,
        client_secret,
        device_code,
        interval_secs,
        expires_at,
        region: region.to_string(),
        user_code,
        verification_uri_complete,
    })
}

/// Poll `create_token` until the user approves the device in their browser,
/// respecting the server's requested polling interval and backing off on
/// `SlowDownException`. On success, caches the access token in the OS
/// keyring under `start_url` and returns it.
pub async fn poll_for_token(pending: &PendingLogin, start_url: &str) -> Result<SsoToken> {
    use aws_sdk_ssooidc::operation::create_token::CreateTokenError;

    let client = ssooidc_client(&pending.region).await;
    let mut interval = pending.interval_secs;

    loop {
        if Utc::now() >= pending.expires_at {
            return Err(GitziError::Config(
                "AWS SSO device code expired before login was approved".into(),
            ));
        }

        let result = client
            .create_token()
            .client_id(&pending.client_id)
            .client_secret(&pending.client_secret)
            .grant_type("urn:ietf:params:oauth:grant-type:device_code")
            .device_code(&pending.device_code)
            .send()
            .await;

        match result {
            Ok(output) => {
                let access_token = output
                    .access_token()
                    .ok_or_else(|| {
                        GitziError::Config("AWS SSO create_token returned no access_token".into())
                    })?
                    .to_string();
                let expires_at = Utc::now() + chrono::Duration::seconds(output.expires_in() as i64);
                let token = SsoToken {
                    access_token,
                    expires_at,
                };
                store_token(start_url, &token)?;
                return Ok(token);
            }
            Err(err) => match err.into_service_error() {
                CreateTokenError::AuthorizationPendingException(_) => {
                    tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                }
                CreateTokenError::SlowDownException(_) => {
                    interval += 5;
                    tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                }
                CreateTokenError::ExpiredTokenException(_) => {
                    return Err(GitziError::Config(
                        "AWS SSO device code expired before login was approved".into(),
                    ));
                }
                other => {
                    return Err(GitziError::Config(format!(
                        "AWS SSO create_token failed: {other}"
                    )));
                }
            },
        }
    }
}

/// Cache an SSO access token in the OS keyring, keyed by SSO start URL.
pub fn store_token(start_url: &str, token: &SsoToken) -> Result<()> {
    let serialized = serde_json::to_string(token)
        .map_err(|e| GitziError::Config(format!("failed to serialize SSO token: {e}")))?;
    secrets::store_secret(SSO_TOKEN_SERVICE, start_url, &serialized)?;
    Ok(())
}

/// Load a previously cached SSO access token for `start_url`, if any and if
/// not expired.
pub fn load_token(start_url: &str) -> Option<SsoToken> {
    let pointer = format!("keyring:{SSO_TOKEN_SERVICE}/{start_url}");
    let raw = secrets::resolve_secret(&pointer).ok()?;
    let token: SsoToken = serde_json::from_str(&raw).ok()?;
    if token.is_expired() {
        None
    } else {
        Some(token)
    }
}

/// Cache an OIDC client registration in the OS keyring, keyed by SSO start URL.
fn store_client_registration(start_url: &str, reg: &ClientRegistration) -> Result<()> {
    let serialized = serde_json::to_string(reg).map_err(|e| {
        GitziError::Config(format!("failed to serialize SSO client registration: {e}"))
    })?;
    secrets::store_secret(SSO_CLIENT_SERVICE, start_url, &serialized)?;
    Ok(())
}

/// Load a previously cached OIDC client registration for `start_url`, if any
/// and if not expired.
fn load_client_registration(start_url: &str) -> Option<ClientRegistration> {
    let pointer = format!("keyring:{SSO_CLIENT_SERVICE}/{start_url}");
    let raw = secrets::resolve_secret(&pointer).ok()?;
    let reg: ClientRegistration = serde_json::from_str(&raw).ok()?;
    if reg.is_expired() { None } else { Some(reg) }
}

/// List the AWS accounts assigned to the user behind `access_token`.
pub async fn list_accounts(region: &str, access_token: &str) -> Result<Vec<SsoAccount>> {
    let client = sso_client(region).await;
    let mut accounts = Vec::new();
    let mut next_token: Option<String> = None;

    loop {
        let mut req = client.list_accounts().access_token(access_token);
        if let Some(token) = &next_token {
            req = req.next_token(token);
        }
        let output = req
            .send()
            .await
            .map_err(|e| GitziError::Config(format!("AWS SSO list_accounts failed: {e}")))?;

        for account in output.account_list() {
            accounts.push(SsoAccount {
                account_id: account.account_id().unwrap_or_default().to_string(),
                account_name: account.account_name().unwrap_or_default().to_string(),
                email_address: account.email_address().unwrap_or_default().to_string(),
            });
        }

        next_token = output.next_token().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }

    Ok(accounts)
}

/// List the permission-set roles the user has within `account_id`.
pub async fn list_account_roles(
    region: &str,
    access_token: &str,
    account_id: &str,
) -> Result<Vec<SsoRole>> {
    let client = sso_client(region).await;
    let mut roles = Vec::new();
    let mut next_token: Option<String> = None;

    loop {
        let mut req = client
            .list_account_roles()
            .access_token(access_token)
            .account_id(account_id);
        if let Some(token) = &next_token {
            req = req.next_token(token);
        }
        let output = req
            .send()
            .await
            .map_err(|e| GitziError::Config(format!("AWS SSO list_account_roles failed: {e}")))?;

        for role in output.role_list() {
            roles.push(SsoRole {
                role_name: role.role_name().unwrap_or_default().to_string(),
                account_id: role.account_id().unwrap_or_default().to_string(),
            });
        }

        next_token = output.next_token().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }

    Ok(roles)
}

/// Exchange the SSO session for temporary credentials scoped to one role in
/// one account. Call this fresh every time credentials are needed — the
/// result is short-lived (typically ~1 hour) and not cached.
pub async fn get_role_credentials(
    region: &str,
    access_token: &str,
    account_id: &str,
    role_name: &str,
) -> Result<RoleCredentials> {
    let client = sso_client(region).await;

    let output = client
        .get_role_credentials()
        .access_token(access_token)
        .account_id(account_id)
        .role_name(role_name)
        .send()
        .await
        .map_err(|e| GitziError::Config(format!("AWS SSO get_role_credentials failed: {e}")))?;

    let creds = output.role_credentials().ok_or_else(|| {
        GitziError::Config("AWS SSO get_role_credentials returned no credentials".into())
    })?;

    Ok(RoleCredentials {
        access_key_id: creds.access_key_id().unwrap_or_default().to_string(),
        secret_access_key: creds.secret_access_key().unwrap_or_default().to_string(),
        session_token: creds.session_token().unwrap_or_default().to_string(),
        expiration_ms: creds.expiration(),
    })
}

/// Hands AWS-Bedrock-bound SDK clients temporary role credentials directly,
/// in-process — no `credential_process` subprocess, no `~/.aws/config` entry.
/// Pass this to `aws_config`'s `.credentials_provider()`; the SDK's own
/// identity cache calls `provide_credentials` again once the previous
/// credentials approach their `expiration_ms`, so callers don't need to
/// re-fetch or cache role credentials themselves.
///
/// Reuses the cached SSO access token (see module docs) and falls back to a
/// fresh device-authorization login (opening a browser) only if that token is
/// missing or expired — same as every other AWS SSO entry point in this
/// module.
#[derive(Debug, Clone)]
pub struct SsoCredentialsProvider {
    region: String,
    start_url: String,
    account_id: String,
    role_name: String,
}

impl SsoCredentialsProvider {
    pub fn new(
        region: impl Into<String>,
        start_url: impl Into<String>,
        account_id: impl Into<String>,
        role_name: impl Into<String>,
    ) -> Self {
        Self {
            region: region.into(),
            start_url: start_url.into(),
            account_id: account_id.into(),
            role_name: role_name.into(),
        }
    }

    async fn load(&self) -> Result<AwsCredentials> {
        let token = match load_token(&self.start_url) {
            Some(token) => token,
            None => {
                let pending = start_device_login(&self.region, &self.start_url).await?;
                poll_for_token(&pending, &self.start_url).await?
            }
        };

        let creds = get_role_credentials(
            &self.region,
            &token.access_token,
            &self.account_id,
            &self.role_name,
        )
        .await?;

        let expires_after = UNIX_EPOCH + Duration::from_millis(creds.expiration_ms.max(0) as u64);
        Ok(AwsCredentials::new(
            creds.access_key_id,
            creds.secret_access_key,
            Some(creds.session_token),
            Some(expires_after),
            "gitzi-sso",
        ))
    }
}

impl ProvideCredentials for SsoCredentialsProvider {
    fn provide_credentials<'a>(&'a self) -> future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        future::ProvideCredentials::new(async move {
            self.load()
                .await
                .map_err(|e| CredentialsError::provider_error(e.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sso_token_is_expired_detects_past_expiry() {
        let token = SsoToken {
            access_token: "tok".to_string(),
            expires_at: Utc::now() - chrono::Duration::seconds(1),
        };
        assert!(token.is_expired());
    }

    #[test]
    fn sso_token_is_expired_false_for_future_expiry() {
        let token = SsoToken {
            access_token: "tok".to_string(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
        };
        assert!(!token.is_expired());
    }
}
