//! Backend-owned bootstrap "setup or use" gate (ADR-002).
//!
//! The experience is binary: **setup an LLM** or **use an LLM**. There is no
//! degraded in-between. The daemon evaluates the gate on startup; until a valid
//! control-plane provider exists it stays in *setup mode* (dispatcher idle, no
//! pipeline agents signalled) and publishes the current [`SetupState`] for the
//! frontend to render. The frontend is a thin renderer — all discovery,
//! activation, and validation logic lives here, so it can be reused by any
//! frontend and by the post-bootstrap agent tools alike.

use serde::{Deserialize, Serialize};

use crate::bootstrap::{discover_providers, DiscoveredProvider};
use crate::config::{Config, ProviderDef, ProviderKind};

/// A provider candidate offered to the user during setup. This is the
/// render-ready projection of a [`DiscoveredProvider`] plus its current
/// enabled state in config — everything the frontend needs to draw the picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCandidate {
    /// Provider name (the `[providers.<name>]` key).
    pub name: String,
    /// `"openai-compatible"` or `"bedrock"`.
    pub kind: String,
    /// Human-readable status, e.g. `"running, model loaded"`.
    pub status: String,
    /// True when this candidate can be activated in a single step right now
    /// (an OpenAI-compatible server that's running with a model loaded). Bedrock
    /// and not-yet-running servers still need follow-up steps.
    pub ready: bool,
}

impl ProviderCandidate {
    fn from_discovered(p: &DiscoveredProvider) -> Self {
        let status = if p.model_loaded {
            "running, model loaded".to_string()
        } else if p.running {
            "running, no model loaded".to_string()
        } else if p.installed {
            "installed, not running".to_string()
        } else {
            "not installed".to_string()
        };
        let kind = match p.kind {
            ProviderKind::OpenaiCompatible => "openai-compatible",
            ProviderKind::Bedrock => "bedrock",
        };
        ProviderCandidate {
            name: p.name.clone(),
            kind: kind.to_string(),
            status,
            ready: p.kind == ProviderKind::OpenaiCompatible && p.model_loaded,
        }
    }
}

/// The backend-owned setup state, published to the frontend over the event bus
/// and queryable on demand. The frontend switches on this: splash / picker /
/// error+rescan / app.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SetupState {
    /// Scan / validation in progress — the frontend shows a loading splash.
    Loading,
    /// No valid provider yet; offer discovered candidates to activate. The only
    /// human-facing setup step.
    NeedsProvider { candidates: Vec<ProviderCandidate> },
    /// The scan found nothing, or an activation failed. Every error is listed;
    /// `can_rescan` tells the frontend to offer a re-scan.
    Error { messages: Vec<String>, can_rescan: bool },
    /// A valid control-plane provider exists; the app is usable.
    Ready,
}

/// Does the main chat agent resolve to an enabled provider?
fn main_provider_enabled(config: &Config) -> bool {
    let def = config.resolve_agent("main");
    match def.provider {
        Some(name) => config.providers.get(&name).map(|d| d.enabled).unwrap_or(false),
        None => false,
    }
}

/// Is the distinguished fallback provider set and enabled?
fn fallback_enabled(config: &Config) -> bool {
    match &config.fallback_provider {
        Some(name) => config.providers.get(name).map(|d| d.enabled).unwrap_or(false),
        None => false,
    }
}

/// The gate: is there a working control-plane LLM? True when either the main
/// agent is bound to an enabled provider, or the distinguished fallback
/// provider is enabled. This is re-evaluated on every load — file existence is
/// never the signal (ADR-002).
pub fn gate_ready(config: &Config) -> bool {
    config.onboarding_complete && (main_provider_enabled(config) || fallback_enabled(config))
}

/// Run the (blocking) environment scan, merging anything new into `config` as
/// disabled entries, and project the results into the frontend-facing
/// [`SetupState`] the daemon should publish next. Returns `NeedsProvider` when
/// candidates were found, or `Error` (with a rescannable "nothing found"
/// message) when the machine has no LLM to offer. `config` is mutated with the
/// merged providers; the caller persists.
///
/// Only providers that are actually installed/configured on this machine are
/// surfaced — we never suggest providers the user doesn't have.
pub fn scan_to_state(config: &mut Config) -> SetupState {
    let discovered = discover_providers();
    // Filter to only providers that are actually installed on this machine.
    let usable: Vec<_> = discovered.iter().filter(|p| p.installed).collect();
    if usable.is_empty() {
        return SetupState::Error {
            messages: vec![
                "No LLM provider found on this machine. Install a provider \
                 (LM Studio, Ollama, or configure AWS Bedrock in ~/.aws/config), \
                 then rescan."
                    .to_string(),
            ],
            can_rescan: true,
        };
    }
    let candidates: Vec<_> = usable
        .iter()
        .map(|p| ProviderCandidate::from_discovered(p))
        .collect();
    merge_discovered(config, &discovered);
    SetupState::NeedsProvider { candidates }
}

/// Merge freshly discovered providers into `config` as disabled entries,
/// skipping any already present. Returns the names that were added.
pub fn merge_discovered(config: &mut Config, discovered: &[DiscoveredProvider]) -> Vec<String> {
    let mut added = Vec::new();
    for provider in discovered {
        if config.providers.contains_key(&provider.name) {
            continue;
        }
        let mut def = ProviderDef {
            kind: provider.kind,
            enabled: false,
            ..ProviderDef::default()
        };
        match provider.kind {
            ProviderKind::OpenaiCompatible => {
                def.api_url = provider.api_url.clone();
            }
            ProviderKind::Bedrock => {
                def.region = provider.region.clone();
                def.sso_start_url = provider.sso_start_url.clone();
            }
        }
        def.default_model = provider.default_model.clone();
        config.providers.insert(provider.name.clone(), def);
        added.push(provider.name.clone());
    }
    added
}

/// Bind the `main` agent to `provider_name`, creating the entry if absent.
/// Also records the provider as the distinguished fallback (control-plane
/// brain) if no fallback has been chosen yet.
pub fn wire_main_agent(config: &mut Config, provider_name: &str) {
    if let Some(agent) = config.agents.iter_mut().find(|a| a.role == "main") {
        agent.provider = Some(provider_name.to_string());
        agent.api_url = None;
    } else {
        config.agents.push(crate::config::AgentDef {
            role: "main".to_string(),
            provider: Some(provider_name.to_string()),
            ..crate::config::AgentDef::default()
        });
    }
    if config.fallback_provider.is_none() {
        config.fallback_provider = Some(provider_name.to_string());
    }
}

/// Outcome of an activation attempt. Activation may need several round-trips
/// for Bedrock (SSO login → choose account → choose role), so a successful
/// call can still be a request for more input rather than a finished state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationOutcome {
    /// The provider is now enabled and wired into the main agent.
    Activated { message: String },
    /// Activation needs another call with more arguments (Bedrock multi-step).
    NeedsMoreInput { message: String },
}

/// Activate a discovered provider in `config` (mutating it in place; the caller
/// persists). For OpenAI-compatible providers this is immediate. For Bedrock it
/// drives the AWS SSO device flow and may return [`ActivationOutcome::NeedsMoreInput`]
/// to request account/role selection. Shared by the daemon setup phase and the
/// `gitzi_activate_provider` agent tool.
pub async fn activate(
    config: &mut Config,
    name: &str,
    account_id: Option<String>,
    role_name: Option<String>,
) -> anyhow::Result<ActivationOutcome> {
    let mut provider = config.providers.get(name).cloned().ok_or_else(|| {
        anyhow::anyhow!("no provider named '{name}' — run a rescan first")
    })?;

    if provider.kind == ProviderKind::OpenaiCompatible {
        provider.enabled = true;
        config.providers.insert(name.to_string(), provider);
        wire_main_agent(config, name);
        // Auto-discover repos if none configured yet
        if config.repo_paths.is_empty() {
            config.repo_paths = crate::bootstrap::discover_repo_paths();
        }
        config.onboarding_complete = true;
        return Ok(ActivationOutcome::Activated {
            message: format!("Activated '{name}' and wired it into the main agent."),
        });
    }

    // Bedrock: AWS SSO login → account → role → validate.
    let region = provider.region.clone().ok_or_else(|| {
        anyhow::anyhow!("provider '{name}' has no AWS region configured")
    })?;
    let start_url = provider.sso_start_url.clone().ok_or_else(|| {
        anyhow::anyhow!("provider '{name}' has no sso_start_url configured")
    })?;

    let token = match crate::aws_sso::load_token(&start_url) {
        Some(t) => t,
        None => {
            let pending = crate::aws_sso::start_device_login(&region, &start_url).await?;
            crate::aws_sso::poll_for_token(&pending, &start_url).await?
        }
    };

    let Some(account_id) = account_id else {
        let accounts = crate::aws_sso::list_accounts(&region, &token.access_token).await?;
        if accounts.is_empty() {
            anyhow::bail!("AWS SSO login succeeded but no accounts are assigned to this user");
        }
        let listing = accounts
            .iter()
            .map(|a| format!("- {} ({}) <{}>", a.account_id, a.account_name, a.email_address))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(ActivationOutcome::NeedsMoreInput {
            message: format!(
                "AWS SSO login confirmed. Choose an account and activate '{name}' again with account_id set:\n{listing}"
            ),
        });
    };

    let Some(role_name) = role_name else {
        let roles =
            crate::aws_sso::list_account_roles(&region, &token.access_token, &account_id).await?;
        if roles.is_empty() {
            anyhow::bail!("no SSO roles assigned to account '{account_id}'");
        }
        let listing = roles
            .iter()
            .map(|r| format!("- {}", r.role_name))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(ActivationOutcome::NeedsMoreInput {
            message: format!(
                "Choose a role and activate '{name}' again with account_id='{account_id}' and role_name set:\n{listing}"
            ),
        });
    };

    // Validate the chosen account/role actually exchange for credentials.
    crate::aws_sso::get_role_credentials(&region, &token.access_token, &account_id, &role_name)
        .await?;

    provider.enabled = true;
    provider.sso_account_id = Some(account_id.clone());
    provider.sso_role_name = Some(role_name.clone());
    if provider.model_id.is_none() {
        provider.model_id = Some("anthropic.claude-sonnet-4-6-v1:0".to_string());
    }
    config.providers.insert(name.to_string(), provider);
    wire_main_agent(config, name);
    // Auto-discover repos if none configured yet
    if config.repo_paths.is_empty() {
        config.repo_paths = crate::bootstrap::discover_repo_paths();
    }
    config.onboarding_complete = true;

    Ok(ActivationOutcome::Activated {
        message: format!(
            "Activated Bedrock provider '{name}' (account {account_id}, role {role_name}) and wired it into the main agent."
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentDef, ProviderDef};
    use std::collections::HashMap;

    fn provider(enabled: bool) -> ProviderDef {
        ProviderDef {
            api_url: "http://localhost:1234/v1".to_string(),
            enabled,
            ..ProviderDef::default()
        }
    }

    #[test]
    fn gate_not_ready_on_default_config() {
        // Default config has a disabled-by-discovery model but no activated
        // fallback and no main binding — must require setup.
        let config = Config {
            providers: HashMap::from([("lmstudio".to_string(), provider(false))]),
            agents: Vec::new(),
            fallback_provider: None,
            ..Config::default()
        };
        assert!(!gate_ready(&config));
    }

    #[test]
    fn gate_ready_when_main_bound_to_enabled_provider() {
        let config = Config {
            providers: HashMap::from([("lmstudio".to_string(), provider(true))]),
            agents: vec![AgentDef {
                role: "main".to_string(),
                provider: Some("lmstudio".to_string()),
                ..AgentDef::default()
            }],
            fallback_provider: None,
            onboarding_complete: true,
            ..Config::default()
        };
        assert!(gate_ready(&config));
    }

    #[test]
    fn gate_ready_when_fallback_enabled_even_without_main_binding() {
        let config = Config {
            providers: HashMap::from([("lmstudio".to_string(), provider(true))]),
            agents: Vec::new(),
            fallback_provider: Some("lmstudio".to_string()),
            onboarding_complete: true,
            ..Config::default()
        };
        assert!(gate_ready(&config));
    }

    #[test]
    fn gate_not_ready_when_provider_bound_but_disabled() {
        let config = Config {
            providers: HashMap::from([("lmstudio".to_string(), provider(false))]),
            agents: vec![AgentDef {
                role: "main".to_string(),
                provider: Some("lmstudio".to_string()),
                ..AgentDef::default()
            }],
            fallback_provider: Some("lmstudio".to_string()),
            ..Config::default()
        };
        assert!(!gate_ready(&config));
    }

    #[tokio::test]
    async fn activate_openai_provider_enables_wires_and_sets_fallback() {
        let mut config = Config {
            providers: HashMap::from([("lmstudio".to_string(), provider(false))]),
            agents: Vec::new(),
            fallback_provider: None,
            ..Config::default()
        };

        let outcome = activate(&mut config, "lmstudio", None, None).await.unwrap();
        assert!(matches!(outcome, ActivationOutcome::Activated { .. }));
        assert!(config.providers["lmstudio"].enabled);
        assert_eq!(config.fallback_provider.as_deref(), Some("lmstudio"));
        assert_eq!(
            config.resolve_agent("main").provider.as_deref(),
            Some("lmstudio")
        );
        // The gate now passes — the app is usable.
        assert!(gate_ready(&config));
        assert!(config.onboarding_complete);
    }

    #[tokio::test]
    async fn activate_unknown_provider_errors() {
        let mut config = Config::default();
        let err = activate(&mut config, "does-not-exist", None, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does-not-exist"));
    }

    #[test]
    fn merge_discovered_adds_only_new_as_disabled() {
        let mut config = Config {
            providers: HashMap::from([("lmstudio".to_string(), provider(true))]),
            ..Config::default()
        };
        let discovered = vec![
            DiscoveredProvider {
                name: "lmstudio".to_string(),
                kind: ProviderKind::OpenaiCompatible,
                api_url: "http://localhost:1234/v1".to_string(),
                region: None,
                sso_start_url: None,
                running: true,
                model_loaded: true,
                installed: true,
                default_model: Some("qwen3-8b".to_string()),
            },
            DiscoveredProvider {
                name: "ollama".to_string(),
                kind: ProviderKind::OpenaiCompatible,
                api_url: "http://localhost:11434/v1".to_string(),
                region: None,
                sso_start_url: None,
                running: false,
                model_loaded: false,
                installed: true,
                default_model: Some("qwen3:8b".to_string()),
            },
        ];
        let added = merge_discovered(&mut config, &discovered);
        assert_eq!(added, vec!["ollama".to_string()]);
        // Existing enabled provider untouched; new one disabled.
        assert!(config.providers["lmstudio"].enabled);
        assert!(!config.providers["ollama"].enabled);
    }
}
