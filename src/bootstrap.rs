//! First-run bootstrapper: discovers LLM providers, scans for repos, generates config.

use std::path::PathBuf;
use tracing::info;

use crate::config::{AgentDef, Config, ProviderDef, WipLimits, atomic_write};
use crate::state::home;

/// A discovered LLM provider.
#[derive(Debug, Clone)]
pub struct DiscoveredProvider {
    pub name: String,
    pub api_url: String,
    /// Whether the server process is running (port is open).
    pub running: bool,
    /// Whether at least one model is loaded and ready to serve.
    pub model_loaded: bool,
    /// Whether the binary/CLI is installed on the system.
    pub installed: bool,
}

/// Run the full bootstrap: discover providers, discover repos, generate config.
/// Returns the generated config.
pub fn run() -> crate::error::Result<Config> {
    info!("bootstrapping gitzi — discovering environment...");

    // 1. Discover LLM providers
    let providers = discover_providers();
    info!("found {} LLM provider(s)", providers.len());

    // 2. Pick the best provider (first running one, or first installed)
    let chosen = providers.first().cloned();

    // 3. Discover repos
    let repo_paths = discover_repo_paths();
    info!("found {} repo path(s)", repo_paths.len());

    // 4. Build config
    let mut config = Config::default();

    // Set WIP limits to 1 for all agent columns
    let mut overrides = std::collections::HashMap::new();
    overrides.insert("designing".to_string(), 1);
    overrides.insert("coding".to_string(), 1);
    overrides.insert("reviewing".to_string(), 1);
    overrides.insert("testing".to_string(), 1);
    overrides.insert("auditing".to_string(), 1);
    overrides.insert("deploying".to_string(), 1);
    config.wip_limits = WipLimits { overrides };

    // Set providers
    config.providers.clear();
    for provider in &providers {
        config.providers.insert(
            provider.name.clone(),
            ProviderDef {
                api_url: provider.api_url.clone(),
                api_key: String::new(),
            },
        );
    }

    // Set agents (all roles pointing to the chosen provider)
    let provider_name = chosen.as_ref().map(|p| p.name.clone());
    let roles = [
        "main", "prioritizer", "designer", "coder",
        "reviewer", "tester", "auditor", "infrarian",
    ];
    config.agents = roles.iter().map(|role| AgentDef {
        role: role.to_string(),
        model: "local-model".to_string(),
        api_url: None,
        provider: provider_name.clone(),
    }).collect();

    // Set repo_paths
    config.repo_paths = repo_paths;

    // 5. Write config
    let path = home::global_config_file();
    home::ensure_dirs()?;
    write_config_with_comments(&path, &config)?;

    info!("config written to {}", path.display());
    Ok(config)
}

/// Discover available LLM providers on this machine.
/// Priority: running processes first, then installed-but-not-running.
pub fn discover_providers() -> Vec<DiscoveredProvider> {
    let mut providers = Vec::new();

    // LM Studio
    let lms_path = dirs::home_dir().map(|h| h.join(".lmstudio/bin/lms"));
    if let Some(ref path) = lms_path {
        if path.exists() {
            let running = check_port_open(1234);
            let model_loaded = running && has_models_loaded("http://localhost:1234/v1");
            providers.push(DiscoveredProvider {
                name: "lmstudio".to_string(),
                api_url: "http://localhost:1234/v1".to_string(),
                running,
                model_loaded,
                installed: true,
            });
        }
    }

    // Ollama
    if which("ollama") {
        let running = check_port_open(11434);
        let model_loaded = running && has_models_loaded("http://localhost:11434/v1");
        providers.push(DiscoveredProvider {
            name: "ollama".to_string(),
            api_url: "http://localhost:11434/v1".to_string(),
            running,
            model_loaded,
            installed: true,
        });
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

/// Check if a provider has at least one model loaded via the /v1/models endpoint.
fn has_models_loaded(base_url: &str) -> bool {
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
                    .map(|arr| !arr.is_empty())
                    .unwrap_or(false)
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Attempt to load a model for the given provider.
/// Returns true if a model is now available.
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
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join(".git").is_dir() {
            if let Some(parent) = cwd.parent() {
                if !found_parents.iter().any(|p| p == parent) {
                    found_parents.push(parent.to_path_buf());
                }
            }
        }
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
        "# LLM endpoints. api_key is plaintext — never commit this file."
    ).unwrap();
    writeln!(out, "# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
        .unwrap();
    writeln!(out).unwrap();
    for (name, provider) in &config.providers {
        writeln!(out, "[providers.{name}]").unwrap();
        writeln!(out, "api_url = \"{}\"", provider.api_url).unwrap();
        if !provider.api_key.is_empty() {
            writeln!(out, "api_key = \"{}\"", provider.api_key).unwrap();
        }
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
