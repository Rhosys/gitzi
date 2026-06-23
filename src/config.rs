use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};
use crate::dispatcher::AgentRole;
use crate::error::{GitziError, Result};

/// Merge strategy for completed tasks.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MergeStrategy {
    /// Fast-forward only. If not possible, create a review item.
    #[default]
    FfOnly,
    /// Merge into a dedicated gitzi branch (no main merge).
    GitziBranch,
    /// Merge with conflict resolution (create merge commit).
    MergeCommit,
    /// Create a pull/merge request on the remote.
    PullRequest,
    /// Push branch to remote without merging.
    PushToRemote,
}

/// Per-repo configuration overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub slug: String,
    #[serde(default)]
    pub merge_strategy: MergeStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_branch: Option<String>,
}

/// Resolved configuration for a repo (defaults applied).
pub struct ResolvedRepoConfig {
    pub merge_strategy: MergeStrategy,
    pub test_command: String,
    pub main_branch: String,
}

/// An LLM provider definition (e.g. LM Studio, Ollama, OpenAI-compatible endpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDef {
    pub api_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
}

/// Per-column WIP limit overrides, as configured in `config.toml`:
///
/// ```toml
/// [wip_limits]
/// coding = 2
/// coding-buffer = 3
/// ```
///
/// Keys are column names in kebab-case (matching how `Column` serializes).
/// Columns not listed keep their built-in default — see
/// `dispatcher::board::WipLimits::default`. Unknown keys are rejected by
/// `dispatcher::board::WipLimits::from_config` at load time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WipLimits {
    pub overrides: HashMap<String, u32>,
}

/// An agent definition. Define as many as you like under `[[agents]]`.
/// The role is the identifier — reference it via `default_agent` or per-task.
///
/// ```toml
/// [[agents]]
/// role = "developer"
/// model = "claude-sonnet-4-6"
/// system_prompt = """
/// You are a disciplined coding agent. Make the smallest possible change.
/// No refactoring, no extras.
/// """
///
/// [[agents]]
/// role = "planner"
/// model = "qwen2.5-coder-32b"
/// provider = "lmstudio"
/// system_prompt = """
/// Break the epic into precise, minimal, independently shippable tasks.
/// """
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub role: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Direct API URL for this agent. Mutually exclusive with `provider`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    /// Reference a named entry in `[providers]` instead of a direct `api_url`.
    /// Omit to keep using the local `claude` CLI subprocess (the original behavior).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// System prompt sent before every task. Falls back to a sensible built-in default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

impl Default for AgentDef {
    fn default() -> Self {
        Self {
            role: String::new(),
            model: default_model(),
            api_url: None,
            provider: None,
            system_prompt: None,
        }
    }
}

impl AgentDef {
    /// Hardcoded default for the `role = "main"` chat harness agent.
    pub fn default_main() -> Self {
        Self {
            role: "main".to_string(),
            model: "local-model".to_string(),
            api_url: Some("http://localhost:1234/v1".to_string()),
            provider: None,
            system_prompt: Some(
                "You are the main coordination agent for gitzi, an AI-driven software \
                 development pipeline. Help the user manage their project through natural \
                 conversation. Create and refine epics, tasks, and work items. Surface what \
                 needs attention. Keep work moving. Never write code directly. \
                 Ask one question at a time — never more."
                    .to_string(),
            ),
        }
    }

    /// Hardcoded default for the `role = "verifier"` contract-verification agent.
    /// The verifier's system prompt lives in `src/verifier.rs` (VERIFIER_SYSTEM_PROMPT)
    /// and is injected at call time — this default only sets the model/endpoint.
    pub fn default_verifier() -> Self {
        Self {
            role: "verifier".to_string(),
            model: "local-model".to_string(),
            api_url: Some("http://localhost:1234/v1".to_string()),
            provider: None,
            system_prompt: None,
        }
    }
}

fn default_model() -> String { "claude-sonnet-4-6".to_string() }

fn default_providers() -> HashMap<String, ProviderDef> {
    HashMap::from([
        ("lmstudio".to_string(), ProviderDef {
            api_url: "http://localhost:1234/v1".to_string(),
            api_key: String::new(),
        }),
    ])
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub wip_limits: WipLimits,

    /// Name of the agent used when a task does not specify one.
    #[serde(default = "default_agent_name")]
    pub default_agent: String,

    /// Named LLM provider endpoints. Reference by name in an agent's `provider` field.
    #[serde(default = "default_providers")]
    pub providers: HashMap<String, ProviderDef>,

    /// All agent definitions. Referenced by name via `default_agent`
    /// or per-task via the `agent` field on a task.
    #[serde(default)]
    pub agents: Vec<AgentDef>,

    #[serde(default = "default_test_command")]
    pub test_command: String,
    /// When true (default), the main agent can auto-close forks via `gitzi_close_fork`.
    #[serde(default = "default_fork_auto_close")]
    pub fork_auto_close: bool,
    #[serde(default)]
    pub integrations: HashMap<String, toml::Value>,
    /// Glob patterns for discovering repos managed by gitzi.
    /// Each pattern is expanded and checked for a `.git/` directory.
    #[serde(default)]
    pub repo_paths: Vec<String>,
    /// Per-repository configuration overrides.
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
}

fn default_agent_name() -> String { "developer".to_string() }

fn default_test_command() -> String { "cargo test".to_string() }

fn default_fork_auto_close() -> bool { true }

impl Default for Config {
    fn default() -> Self {
        Self {
            wip_limits: WipLimits::default(),
            default_agent: default_agent_name(),
            providers: default_providers(),
            agents: Vec::new(),
            test_command: default_test_command(),
            fork_auto_close: default_fork_auto_close(),
            integrations: HashMap::new(),
            repo_paths: Vec::new(),
            repos: Vec::new(),
        }
    }
}

impl Config {
    /// Load from `~/.gitzi/config.toml`.
    /// On first run (file absent) writes a fully-commented scaffold and returns its values.
    /// Validates the result before returning.
    pub fn load(_repo_root: &Path) -> Result<Self> {
        let path = crate::state::home::global_config_file();
        if !path.exists() {
            let text = render_scaffold_toml();
            let config: Self = toml::from_str(&text).map_err(|e| {
                GitziError::Config(format!("scaffold TOML failed to parse: {e}"))
            })?;
            config.validate()?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match atomic_write(&path, &text) {
                Ok(()) => tracing::info!(path = %path.display(), "wrote default config.toml"),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "could not write default config.toml"),
            }
            return Ok(config);
        }
        let text = std::fs::read_to_string(&path)?;
        let config: Self = toml::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn write(&self, _repo_root: &Path) -> Result<()> {
        let path = crate::state::home::global_config_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        atomic_write(&path, &text)
    }

    /// Validate config on load. Returns error for unknown role names or
    /// unknown WIP column overrides.
    pub fn validate(&self) -> Result<()> {
        let mut valid_roles: Vec<String> =
            AgentRole::all().iter().map(|r| r.to_string()).collect();
        valid_roles.push("main".to_string());
        for agent in &self.agents {
            if !valid_roles.contains(&agent.role) {
                return Err(GitziError::Config(format!(
                    "unknown agent role '{}' in config — valid roles: {:?}",
                    agent.role, valid_roles
                )));
            }
            if let Some(provider) = &agent.provider
                && !self.providers.contains_key(provider)
            {
                return Err(GitziError::Config(format!(
                    "agent '{}' references unknown provider '{}' — define it under [providers.{}]",
                    agent.role, provider, provider
                )));
            }
        }
        crate::dispatcher::board::WipLimits::from_config(&self.wip_limits.overrides)
            .map_err(GitziError::Config)?;
        Ok(())
    }

    /// Find agent by role: config first, then hardcoded default. Never first-in-list.
    pub fn resolve_agent(&self, role: &str) -> AgentDef {
        self.agents
            .iter()
            .find(|a| a.role == role)
            .cloned()
            .unwrap_or_else(|| {
                if role == "main" {
                    return AgentDef::default_main();
                }
                if role == "verifier" {
                    return AgentDef::default_verifier();
                }
                AgentRole::all()
                    .iter()
                    .find(|r| r.to_string() == role)
                    .map(|r| r.default_agent_def())
                    .unwrap_or_else(|| AgentRole::Coder.default_agent_def())
            })
    }

    /// Resolve a provider by name, if it exists.
    pub fn resolve_provider(&self, name: &str) -> Option<&ProviderDef> {
        self.providers.get(name)
    }

    /// Resolve config for a specific repo slug. Falls back to global defaults.
    pub fn repo_config(&self, slug: &str) -> ResolvedRepoConfig {
        let repo = self.repos.iter().find(|r| r.slug == slug);
        ResolvedRepoConfig {
            merge_strategy: repo.map(|r| r.merge_strategy.clone()).unwrap_or_default(),
            test_command: repo
                .and_then(|r| r.test_command.clone())
                .unwrap_or_else(|| self.test_command.clone()),
            main_branch: repo
                .and_then(|r| r.main_branch.clone())
                .unwrap_or_else(|| "main".to_string()),
        }
    }
}

pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Build the default config.toml content with explanatory comments.
/// Pulled dynamically from AgentRole::all() and WipLimits defaults so it
/// stays in sync with the code automatically.
fn render_scaffold_toml() -> String {
    let mut out = String::new();

    out.push_str("# gitzi configuration\n");
    out.push_str("# ~/.gitzi/config.toml — edit freely, never committed to git\n");
    out.push_str("#\n");
    out.push_str("# Most settings have sensible defaults; uncomment what you want to change.\n\n");

    // ── test_command ──────────────────────────────────────────────────────────
    out.push_str("# Shell command gitzi runs to verify a task after coding.\n");
    out.push_str(&format!("test_command = {}\n\n", toml_str(&default_test_command())));

    // ── default_agent ─────────────────────────────────────────────────────────
    out.push_str("# Which agent role handles tasks that don't specify one.\n");
    out.push_str(&format!("default_agent = {}\n\n", toml_str(&default_agent_name())));

    // ── wip_limits ────────────────────────────────────────────────────────────
    out.push_str("# Per-column work-in-progress limit overrides. Built-in defaults apply to\n");
    out.push_str("# any column not listed here (see dispatcher::board::WipLimits::default).\n");
    out.push_str("# [wip_limits]\n");
    out.push_str("# coding = 2\n");
    out.push_str("# coding-buffer = 3\n\n");

    // ── providers ─────────────────────────────────────────────────────────────
    out.push_str("# ── Providers ──────────────────────────────────────────────────────────────\n");
    out.push_str("# Named LLM endpoints. Reference them in [[agents]] via provider = \"name\".\n");
    out.push_str("# api_key is stored in plain text here — this file is never committed to git.\n\n");

    for (name, provider) in &default_providers() {
        out.push_str(&format!("[providers.{name}]\n"));
        out.push_str(&format!("api_url = {}\n", toml_str(&provider.api_url)));
        out.push_str("# api_key = \"sk-...\"\n\n");
    }

    // ── agents ────────────────────────────────────────────────────────────────
    out.push_str("# ── Agents ─────────────────────────────────────────────────────────────────\n");
    out.push_str("# Each [[agents]] block overrides the built-in defaults for that role.\n");
    out.push_str("# Valid roles: ");
    let role_names: Vec<String> = AgentRole::all().iter().map(|r| r.to_string()).collect();
    out.push_str(&role_names.join(", "));
    out.push_str(", main\n");
    out.push_str("#\n");
    out.push_str("# Fields:\n");
    out.push_str("#   model       — model identifier string passed to the API\n");
    out.push_str("#   api_url     — direct OpenAI-compatible endpoint (overrides provider)\n");
    out.push_str("#   provider    — name of an entry in [providers] above\n");
    out.push_str("#   system_prompt — override the built-in system prompt for this role\n\n");

    // main agent
    {
        let def = AgentDef::default_main();
        out.push_str("[[agents]]\n");
        out.push_str(&format!("role    = {}\n", toml_str(&def.role)));
        out.push_str(&format!("model   = {}\n", toml_str(&def.model)));
        if let Some(ref url) = def.api_url {
            out.push_str(&format!("api_url = {}\n", toml_str(url)));
        }
        if let Some(ref prompt) = def.system_prompt {
            out.push_str(&format!("system_prompt = {}\n", toml_multiline(prompt)));
        }
        out.push('\n');
    }

    // pipeline roles
    for role in AgentRole::all() {
        let def = role.default_agent_def();
        out.push_str("[[agents]]\n");
        out.push_str(&format!("role    = {}\n", toml_str(&def.role)));
        out.push_str(&format!("model   = {}\n", toml_str(&def.model)));
        out.push_str("# provider = \"lmstudio\"  # use a named provider instead of api_url\n");
        if let Some(ref prompt) = def.system_prompt {
            out.push_str(&format!("system_prompt = {}\n", toml_multiline(prompt)));
        }
        out.push('\n');
    }

    out
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn toml_multiline(s: &str) -> String {
    format!("\"\"\"\n{s}\n\"\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn scaffold_toml_parses_and_validates() {
        let text = render_scaffold_toml();
        let config: Config = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("scaffold failed to parse: {e}\n\n{text}"));
        config.validate().expect("scaffold config failed validate()");
        assert!(config.providers.contains_key("lmstudio"));
        assert!(!config.agents.is_empty(), "scaffold should include agent entries");
        let roles: Vec<&str> = config.agents.iter().map(|a| a.role.as_str()).collect();
        assert!(roles.contains(&"main"));
        for role in AgentRole::all() {
            assert!(roles.contains(&role.to_string().as_str()), "missing role {role}");
        }
    }

    // Feature: dispatcher-audit-fixes, Property 12: resolve_agent returns config override or hardcoded default
    // **Validates: Requirements 7.1, 7.3, 7.4**
    proptest! {
        #[test]
        fn resolve_agent_returns_config_override_or_hardcoded_default(
            include_flags in prop::collection::vec(any::<bool>(), 7..=7),
            custom_models in prop::collection::vec("[a-z]{3,10}", 7..=7),
            custom_prompts in prop::collection::vec("[a-z ]{5,30}", 7..=7),
        ) {
            let all_roles = AgentRole::all();

            // Build config agents vec: include only roles where flag is true
            let config_agents: Vec<AgentDef> = all_roles.iter().zip(include_flags.iter())
                .filter(|&(_, &include)| include)
                .enumerate()
                .map(|(i, (role, _))| AgentDef {
                    role: role.to_string(),
                    model: custom_models[i % custom_models.len()].clone(),
                    api_url: None,
                    provider: None,
                    system_prompt: Some(custom_prompts[i % custom_prompts.len()].clone()),
                })
                .collect();

            let config = Config {
                agents: config_agents.clone(),
                ..Config::default()
            };

            for (idx, role) in all_roles.iter().enumerate() {
                let role_name = role.to_string();
                let resolved = config.resolve_agent(&role_name);

                // Never returns a mismatched role
                prop_assert_eq!(
                    &resolved.role, &role_name,
                    "resolved agent role '{}' doesn't match queried role '{}'",
                    resolved.role, role_name
                );

                if include_flags[idx] {
                    // Config had an entry — should match the config entry
                    let config_entry = config_agents.iter()
                        .find(|a| a.role == role_name).unwrap();
                    prop_assert_eq!(
                        &resolved.model, &config_entry.model,
                        "model mismatch for role '{}'", role_name
                    );
                    prop_assert_eq!(
                        &resolved.system_prompt, &config_entry.system_prompt,
                        "system_prompt mismatch for role '{}'", role_name
                    );
                } else {
                    // No config entry — should match hardcoded default
                    let default_def = role.default_agent_def();
                    prop_assert_eq!(
                        &resolved.model, &default_def.model,
                        "model should be hardcoded default for role '{}'", role_name
                    );
                    prop_assert_eq!(
                        &resolved.system_prompt, &default_def.system_prompt,
                        "system_prompt should be hardcoded default for role '{}'",
                        role_name
                    );
                }
            }
        }
    }

    // Feature: dispatcher-audit-fixes, Property 11: Config role validation rejects unknown roles
    // **Validates: Requirements 7.2**
    proptest! {
        #[test]
        fn config_role_validation_rejects_unknown_roles(
            role_name in "[a-zA-Z0-9_-]{1,30}"
                .prop_filter("must not match any valid role name", |s| {
                    let valid = [
                        "prioritizer", "designer", "coder",
                        "reviewer", "tester", "auditor", "infrarian", "main",
                    ];
                    !valid.contains(&s.as_str())
                })
        ) {
            let config = Config {
                agents: vec![AgentDef {
                    role: role_name.clone(),
                    model: "claude-sonnet-4-6".to_string(),
                    api_url: None,
                    provider: None,
                    system_prompt: None,
                }],
                ..Config::default()
            };

            let result = config.validate();
            prop_assert!(
                result.is_err(),
                "validate() should reject unknown role '{}'", role_name
            );
            let err_msg = format!("{}", result.unwrap_err());
            prop_assert!(
                err_msg.contains(&role_name),
                "error should contain invalid role '{}', got: {}",
                role_name, err_msg
            );
        }
    }

    #[test]
    fn validate_rejects_agent_referencing_unknown_provider() {
        let config = Config {
            agents: vec![AgentDef {
                role: "coder".to_string(),
                provider: Some("nonexistent".to_string()),
                ..AgentDef::default()
            }],
            ..Config::default()
        };

        let err = config.validate().expect_err("unknown provider should be rejected");
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn validate_accepts_agent_referencing_known_provider() {
        let config = Config {
            agents: vec![AgentDef {
                role: "coder".to_string(),
                provider: Some("lmstudio".to_string()),
                ..AgentDef::default()
            }],
            providers: HashMap::from([(
                "lmstudio".to_string(),
                ProviderDef {
                    api_url: "http://localhost:1234/v1".to_string(),
                    api_key: String::new(),
                },
            )]),
            ..Config::default()
        };

        config.validate().expect("known provider reference should be accepted");
    }

    #[test]
    fn providers_table_round_trips_through_toml() {
        let config = Config {
            providers: HashMap::from([(
                "lmstudio".to_string(),
                ProviderDef {
                    api_url: "http://localhost:1234/v1".to_string(),
                    api_key: "sk-test".to_string(),
                },
            )]),
            ..Config::default()
        };

        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        let provider = parsed.providers.get("lmstudio").unwrap();
        assert_eq!(provider.api_url, "http://localhost:1234/v1");
        assert_eq!(provider.api_key, "sk-test");
    }
}
