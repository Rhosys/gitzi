//! Environment scanner: quickly probes for available LLM infrastructure and
//! cloud credentials. [`discover_providers`] is the scan reused by the daemon's
//! bootstrap setup phase ([`crate::setup`]) and by the `gitzi_rediscover_providers`
//! agent tool.
//!
//! This is a *quick, non-blocking* scan — it never starts servers, loads
//! models, or opens a browser for SSO login. Every provider it finds is
//! surfaced as a candidate; activation (which wires a provider into the main
//! agent) is always an explicit user step driven by the setup gate (ADR-002).
//! This avoids onboarding ever getting stuck waiting on a server to start or a
//! model to load.
//!
//! [`run`] (force-regenerate a `config.toml` from a scan) remains available for
//! the `gitzi generate-config` command, but is no longer invoked by
//! `Config::load` — loading is pure read-and-report (ADR-002).

use std::path::PathBuf;
use tracing::info;

use crate::config::{Config, ProviderDef, ProviderKind, WipLimits, atomic_write};
use crate::state::home;

/// A discovered LLM/model provider, surfaced for the user to choose from.
#[derive(Debug, Clone)]
pub struct DiscoveredProvider {
    pub name: String,
    pub kind: ProviderKind,
    /// Base URL for OpenAI-compatible providers.
    pub api_url: String,
    /// AWS region, for Bedrock providers.
    pub region: Option<String>,
    /// AWS SSO start URL, for Bedrock providers (if found in `~/.aws/config`).
    pub sso_start_url: Option<String>,
    /// Whether the server process is running (port is open). N/A for Bedrock.
    pub running: bool,
    /// Whether at least one model is loaded and ready to serve. N/A for Bedrock.
    pub model_loaded: bool,
    /// Whether the binary/CLI is installed on the system.
    pub installed: bool,
    /// Pre-populated default model identifier discovered from the local system.
    pub default_model: Option<String>,
}

/// Run the full bootstrap: discover providers, discover repos, generate config.
/// Returns the generated config. Never auto-wires a provider into `[[agents]]`
/// or starts/loads anything — see module docs.
pub fn run() -> crate::error::Result<Config> {
    info!("bootstrapping gitzi — discovering environment...");

    let providers = discover_providers();
    info!("found {} provider candidate(s)", providers.len());

    let repo_paths = discover_repo_paths();
    info!("found {} repo path(s)", repo_paths.len());

    let config = build_config_from_discovery(&providers, repo_paths);

    let path = home::global_config_file();
    home::ensure_dirs()?;
    write_config_with_comments(&path, &config)?;

    info!("config written to {}", path.display());
    Ok(config)
}

/// Build a `Config` from discovery results: every provider is recorded
/// disabled, and `[[agents]]` is always left empty (every role falls back
/// to the local `claude` CLI until the user explicitly activates a
/// provider). Split out from `run()` so it can be tested without touching
/// the real `~/.gitzi/` or `~/.aws/` on disk.
fn build_config_from_discovery(providers: &[DiscoveredProvider], repo_paths: Vec<String>) -> Config {
    let mut config = Config::default();

    let mut overrides = std::collections::HashMap::new();
    overrides.insert("designing".to_string(), 1);
    overrides.insert("coding".to_string(), 1);
    overrides.insert("reviewing".to_string(), 1);
    overrides.insert("auditing".to_string(), 1);
    overrides.insert("deploying".to_string(), 1);
    config.wip_limits = WipLimits { overrides };

    config.providers.clear();
    for provider in providers {
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
    }

    config.repo_paths = repo_paths;
    config
}

/// Quickly scan this machine for LLM providers and AWS Bedrock access.
/// Every check here is fast (short timeouts, no process spawning that
/// blocks longer than a couple seconds) — this never starts a server, loads
/// a model, or initiates an SSO login.
pub fn discover_providers() -> Vec<DiscoveredProvider> {
    let mut providers = Vec::new();

    // LM Studio — prefer daemon over GUI
    let lms_path = dirs::home_dir().map(|h| h.join(".lmstudio/bin/lms"));
    if let Some(ref path) = lms_path
        && path.exists()
    {
        let running = check_port_open(1234);
        let model_loaded = running && has_models_loaded("http://localhost:1234/v1");
        let default_model = discover_lmstudio_model(path)
            .or_else(|| Some("qwen3-8b".to_string()));
        providers.push(DiscoveredProvider {
            name: "lmstudio".to_string(),
            kind: ProviderKind::OpenaiCompatible,
            api_url: "http://localhost:1234/v1".to_string(),
            region: None,
            sso_start_url: None,
            running,
            model_loaded,
            installed: true,
            default_model,
        });
    }

    // Ollama
    if which("ollama") {
        let running = check_port_open(11434);
        let model_loaded = running && has_models_loaded("http://localhost:11434/v1");
        let default_model = if running {
            discover_ollama_model()
        } else {
            None
        }.or_else(|| Some("qwen3:8b".to_string()));
        providers.push(DiscoveredProvider {
            name: "ollama".to_string(),
            kind: ProviderKind::OpenaiCompatible,
            api_url: "http://localhost:11434/v1".to_string(),
            region: None,
            sso_start_url: None,
            running,
            model_loaded,
            installed: true,
            default_model,
        });
    }

    // AWS Bedrock — surfaced if the AWS CLI is present, or if `~/.aws/config`
    // already has SSO sessions configured (one candidate per session).
    let aws_cli_installed = which("aws");
    let sso_sessions = discover_aws_sso_sessions();
    if sso_sessions.is_empty() {
        if aws_cli_installed {
            providers.push(DiscoveredProvider {
                name: "bedrock".to_string(),
                kind: ProviderKind::Bedrock,
                api_url: String::new(),
                region: None,
                sso_start_url: None,
                running: false,
                model_loaded: false,
                installed: true,
                default_model: None,
            });
        }
    } else {
        for (session_name, start_url, region) in sso_sessions {
            providers.push(DiscoveredProvider {
                name: format!("bedrock-{session_name}"),
                kind: ProviderKind::Bedrock,
                api_url: String::new(),
                region: if region.is_empty() { None } else { Some(region) },
                sso_start_url: Some(start_url),
                running: false,
                model_loaded: false,
                installed: true,
                default_model: None,
            });
        }
    }

    // Sort: running+model first, then running-no-model, then installed-not-running
    providers.sort_by(|a, b| {
        let score = |p: &DiscoveredProvider| -> u8 {
            if p.model_loaded { 2 } else if p.running { 1 } else { 0 }
        };
        score(b).cmp(&score(a)).then(a.name.cmp(&b.name))
    });

    providers
}

/// Parse `~/.aws/config` for `[sso-session NAME]` blocks, returning
/// `(session_name, sso_start_url, sso_region)` for each one that has a
/// start URL set.
fn discover_aws_sso_sessions() -> Vec<(String, String, String)> {
    match dirs::home_dir() {
        Some(h) => parse_aws_sso_sessions(&h.join(".aws/config")),
        None => Vec::new(),
    }
}

/// Parse SSO sessions out of an `~/.aws/config`-formatted file at `path`.
/// Split out from `discover_aws_sso_sessions` so it can be tested against a
/// temp file instead of mutating the process-wide `$HOME`.
fn parse_aws_sso_sessions(path: &std::path::Path) -> Vec<(String, String, String)> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    let mut current: Option<String> = None;
    let mut start_url = String::new();
    let mut region = String::new();

    let flush = |current: &mut Option<String>, start_url: &mut String, region: &mut String, sessions: &mut Vec<(String, String, String)>| {
        if let Some(name) = current.take()
            && !start_url.is_empty()
        {
            sessions.push((name, start_url.clone(), region.clone()));
        }
        start_url.clear();
        region.clear();
    };

    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("[sso-session ").and_then(|s| s.strip_suffix(']')) {
            flush(&mut current, &mut start_url, &mut region, &mut sessions);
            current = Some(name.trim().to_string());
        } else if line.starts_with('[') {
            flush(&mut current, &mut start_url, &mut region, &mut sessions);
        } else if current.is_some()
            && let Some((key, val)) = line.split_once('=')
        {
            match key.trim() {
                "sso_start_url" => start_url = val.trim().to_string(),
                "sso_region" => region = val.trim().to_string(),
                _ => {}
            }
        }
    }
    flush(&mut current, &mut start_url, &mut region, &mut sessions);

    sessions
}

/// Check if a provider has at least one model loaded via the /v1/models endpoint.
fn has_models_loaded(base_url: &str) -> bool {
    query_loaded_model(base_url).is_some()
}

/// Query the /v1/models endpoint and return the first loaded model's ID.
/// Used both for checking readiness and for runtime model resolution when
/// no explicit model is configured (e.g. LM Studio GUI mode).
pub fn query_loaded_model(base_url: &str) -> Option<String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let resp = reqwest::blocking::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(2))
        .send();

    match resp {
        Ok(r) if r.status().is_success() => {
            if let Ok(body) = r.json::<serde_json::Value>() {
                body.get("data")
                    .and_then(|d| d.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|m| m.get("id"))
                    .and_then(|id| id.as_str())
                    .map(str::to_string)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Attempt to load a model for the given provider. Not called automatically
/// during boot (that's what got onboarding stuck before) — only invoked
/// explicitly when the user activates a provider via the main agent's
/// `gitzi_activate_provider` tool. Returns true if a model is now available.
pub fn ensure_model_loaded(provider: &DiscoveredProvider) -> bool {
    if provider.model_loaded {
        return true;
    }
    if !provider.running {
        return false;
    }

    match provider.name.as_str() {
        "lmstudio" => {
            let lms = dirs::home_dir()
                .unwrap_or_default()
                .join(".lmstudio/bin/lms");

            // List downloaded models
            let output = std::process::Command::new(&lms)
                .args(["ls"])
                .output();

            if let Ok(out) = output {
                let text = String::from_utf8_lossy(&out.stdout);
                // Find first LLM model (parse the table output)
                let mut in_llm = false;
                for line in text.lines() {
                    if line.contains("LLM") && line.contains("PARAMS") {
                        in_llm = true;
                        continue;
                    }
                    if line.contains("EMBEDDING") {
                        break;
                    }
                    if in_llm && !line.trim().is_empty() {
                        // Extract model key from first column
                        let model_key = line.split_whitespace().next().unwrap_or("");
                        if !model_key.is_empty() && model_key.contains('/') {
                            info!("loading model: {model_key}");
                            let _ = std::process::Command::new(&lms)
                                .args(["load", model_key, "-y"])
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status();
                            std::thread::sleep(std::time::Duration::from_secs(5));
                            return has_models_loaded(&provider.api_url);
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Query LM Studio for downloaded text models via `lms ls`.
/// Returns the model key of the first text model found, or None.
fn discover_lmstudio_model(lms_path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(lms_path)
        .args(["ls"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // lms ls outputs a table; model keys are lines containing '/' (org/model format)
    // Skip header lines and embedding models
    let mut in_llm_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.contains("LLM")
            && (trimmed.contains("PARAMS") || trimmed.contains("Size"))
        {
            in_llm_section = true;
            continue;
        }
        if trimmed.contains("EMBEDDING") || trimmed.contains("VLM") {
            break;
        }
        if in_llm_section && !trimmed.is_empty() {
            let key = trimmed.split_whitespace().next().unwrap_or("");
            if key.contains('/') {
                return Some(key.to_string());
            }
        }
    }
    None
}

/// Query Ollama for available models via `ollama list`.
/// Returns the first model tag found, or None.
fn discover_ollama_model() -> Option<String> {
    let output = std::process::Command::new("ollama")
        .args(["list"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // ollama list outputs: NAME ID SIZE MODIFIED (header), then rows
    for line in text.lines().skip(1) {
        let name = line.split_whitespace().next().unwrap_or("");
        if !name.is_empty() && name != "NAME" {
            return Some(name.to_string());
        }
    }
    None
}

/// Scan common locations for git repositories.
/// Returns glob patterns that cover the discovered repos.
pub fn discover_repo_paths() -> Vec<String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let candidates = [
        home.join("git"),
        home.join("projects"),
        home.join("code"),
        home.join("src"),
        home.join("repos"),
        home.join("work"),
        home.join("dev"),
    ];

    let mut found_parents: Vec<PathBuf> = Vec::new();

    for candidate in &candidates {
        if !candidate.is_dir() {
            continue;
        }
        // Check if this directory contains any git repos (1 level deep)
        if let Ok(entries) = std::fs::read_dir(candidate) {
            let has_repos = entries
                .flatten()
                .any(|e| e.path().join(".git").is_dir());
            if has_repos {
                found_parents.push(candidate.clone());
            }
        }
    }

    // Also check if cwd contains a .git
    if let Ok(cwd) = std::env::current_dir()
        && cwd.join(".git").is_dir()
        && let Some(parent) = cwd.parent()
        && !found_parents.iter().any(|p| p == parent)
    {
        found_parents.push(parent.to_path_buf());
    }

    // Convert to glob patterns
    found_parents
        .iter()
        .map(|p| format!("{}/*", p.display()))
        .collect()
}

fn which(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn check_port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(200),
    ).is_ok()
}

/// Write config to disk with section headers and discovered values.
fn write_config_with_comments(
    path: &std::path::Path,
    config: &Config,
) -> crate::error::Result<()> {
    use std::fmt::Write;

    let mut out = String::new();

    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out, "# gitzi configuration").unwrap();
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out).unwrap();
    writeln!(out).unwrap();

    // Kanban
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out, "# Kanban").unwrap();
    writeln!(
        out,
        "# Controls how many tasks can be active in each pipeline stage."
    ).unwrap();
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out, "[wip_limits]").unwrap();
    for (col, val) in &config.wip_limits.overrides {
        writeln!(out, "{col} = {val}").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out).unwrap();

    // Agents
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out, "# Agents").unwrap();
    writeln!(
        out,
        "# Each role handles one pipeline stage. Set model and provider only."
    ).unwrap();
    writeln!(
        out,
        "# Empty by default: every role falls back to the local `claude` CLI."
    ).unwrap();
    writeln!(
        out,
        "# Ask the main agent to activate a discovered provider below to wire it in."
    ).unwrap();
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out).unwrap();
    for agent in &config.agents {
        writeln!(out, "[[agents]]").unwrap();
        writeln!(out, "role = \"{}\"", agent.role).unwrap();
        writeln!(out, "model = \"{}\"", agent.model).unwrap();
        if let Some(ref provider) = agent.provider {
            writeln!(out, "provider = \"{provider}\"").unwrap();
        }
        if let Some(ref url) = agent.api_url {
            writeln!(out, "api_url = \"{url}\"").unwrap();
        }
        writeln!(out).unwrap();
    }
    writeln!(out).unwrap();

    // Providers
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out, "# Providers").unwrap();
    writeln!(
        out,
        "# Discovered LLM endpoints and cloud credentials. All start disabled —"
    ).unwrap();
    writeln!(
        out,
        "# ask the main agent to activate the one(s) you want to use."
    ).unwrap();
    writeln!(
        out,
        "# api_key, if set, is a keyring: pointer, not a plaintext secret."
    ).unwrap();
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out).unwrap();
    for (name, provider) in &config.providers {
        writeln!(out, "[providers.{name}]").unwrap();
        if provider.kind == crate::config::ProviderKind::Bedrock {
            writeln!(out, "kind = \"bedrock\"").unwrap();
            if let Some(ref region) = provider.region {
                writeln!(out, "region = \"{region}\"").unwrap();
            }
            if let Some(ref start_url) = provider.sso_start_url {
                writeln!(out, "sso_start_url = \"{start_url}\"").unwrap();
            }
        } else if !provider.api_url.is_empty() {
            writeln!(out, "api_url = \"{}\"", provider.api_url).unwrap();
        }
        if !provider.api_key.is_empty() {
            writeln!(out, "api_key = \"{}\"", provider.api_key).unwrap();
        }
        writeln!(out, "enabled = {}", provider.enabled).unwrap();
        writeln!(out).unwrap();
    }
    writeln!(out).unwrap();

    // Repos
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out, "# Repos").unwrap();
    writeln!(
        out,
        "# Glob patterns for repo discovery + per-repo merge overrides."
    ).unwrap();
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out).unwrap();
    write!(out, "repo_paths = [").unwrap();
    if config.repo_paths.is_empty() {
        writeln!(out, "]").unwrap();
    } else {
        writeln!(out).unwrap();
        for rp in &config.repo_paths {
            writeln!(out, "    \"{rp}\",").unwrap();
        }
        writeln!(out, "]").unwrap();
    }
    writeln!(out).unwrap();

    // merge strategy docs
    writeln!(out, "# merge_strategy options:").unwrap();
    writeln!(out, "#   ff-only        — fast-forward merge into main.").unwrap();
    writeln!(out, "#   gitzi-branch   — merge into a local \"gitzi\" branch.").unwrap();
    writeln!(out, "#   merge-commit   — create a merge commit on main.").unwrap();
    writeln!(out, "#   pull-request   — push + create PR via gh/glab CLI.").unwrap();
    writeln!(out, "#   push-to-remote — push branch, no merge or PR.").unwrap();
    writeln!(out, "#").unwrap();
    writeln!(out, "# [[repos]]").unwrap();
    writeln!(out, "# path = \"/home/user/projects/myapp\"").unwrap();
    writeln!(out, "# merge_strategy = \"ff-only\"").unwrap();
    writeln!(out, "# main_branch = \"main\"").unwrap();
    writeln!(out).unwrap();
    writeln!(out).unwrap();

    // General
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out, "# General").unwrap();
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();

    atomic_write(path, &out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_from_discovery_never_auto_wires_agents() {
        // Bootstrap must leave `[[agents]]` empty and every provider
        // disabled regardless of what's discovered — activation is always
        // an explicit, user-driven step.
        let providers = vec![
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
                name: "bedrock-mycompany".to_string(),
                kind: ProviderKind::Bedrock,
                api_url: String::new(),
                region: Some("us-east-1".to_string()),
                sso_start_url: Some("https://mycompany.awsapps.com/start".to_string()),
                running: false,
                model_loaded: false,
                installed: true,
                default_model: None,
            },
        ];

        let config = build_config_from_discovery(&providers, vec!["/home/user/git/*".to_string()]);

        assert!(config.agents.is_empty());
        assert_eq!(config.providers.len(), 2);
        assert!(config.providers.values().all(|p| !p.enabled));
        assert_eq!(
            config.providers["bedrock-mycompany"].region.as_deref(),
            Some("us-east-1")
        );
        assert_eq!(
            config.providers["bedrock-mycompany"].sso_start_url.as_deref(),
            Some("https://mycompany.awsapps.com/start")
        );
        assert_eq!(config.providers["lmstudio"].api_url, "http://localhost:1234/v1");
    }

    #[test]
    fn parse_aws_sso_sessions_reads_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config");
        std::fs::write(
            &config_path,
            "[sso-session mycompany]\n\
             sso_start_url = https://mycompany.awsapps.com/start\n\
             sso_region = us-east-1\n\
             sso_registration_scopes = sso:account:access\n\
             \n\
             [profile dev]\n\
             sso_session = mycompany\n\
             region = us-east-1\n",
        )
        .unwrap();

        let sessions = parse_aws_sso_sessions(&config_path);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0, "mycompany");
        assert_eq!(sessions[0].1, "https://mycompany.awsapps.com/start");
        assert_eq!(sessions[0].2, "us-east-1");
    }

    #[test]
    fn parse_aws_sso_sessions_returns_empty_for_missing_file() {
        let sessions = parse_aws_sso_sessions(std::path::Path::new("/nonexistent/.aws/config"));
        assert!(sessions.is_empty());
    }
}
