//! Repo discovery and cache management.
//! Discovers repos from config.toml `repo_paths` globs, generates heuristic
//! summaries, and caches them at `~/.gitzi/tmp/cache/repos/<slug>.toml`.

use std::path::Path;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::atomic_write;
use crate::state::home;

/// A cached repo summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoCache {
    pub path: String,
    pub slug: String,
    pub summary: String,
    pub labels: Vec<String>,
    pub commits_by_gitzi: u32,
    pub last_scanned: DateTime<Utc>,
}

/// Discover repos from glob patterns and populate/refresh the cache.
/// Returns the list of discovered repos.
pub fn populate(patterns: &[String]) -> Vec<RepoCache> {
    let repos = home::discover_repos(patterns);
    let cache_dir = home::repo_cache_dir();
    let _ = std::fs::create_dir_all(&cache_dir);

    let mut cached = Vec::new();
    for repo_path in &repos {
        let slug = path_to_slug(repo_path);
        let cache_path = cache_dir.join(format!("{slug}.toml"));

        // Check if cache exists and is recent (less than 24h old)
        if let Some(existing) = load_cache_entry(&cache_path) {
            let age = Utc::now() - existing.last_scanned;
            if age.num_hours() < 24 {
                cached.push(existing);
                continue;
            }
        }

        // Generate fresh summary from heuristics
        let (summary, labels) = generate_summary(repo_path);
        let entry = RepoCache {
            path: repo_path.to_string_lossy().to_string(),
            slug: slug.clone(),
            summary,
            labels,
            commits_by_gitzi: load_cache_entry(&cache_path)
                .map(|e| e.commits_by_gitzi)
                .unwrap_or(0),
            last_scanned: Utc::now(),
        };

        // Persist
        if let Ok(text) = toml::to_string_pretty(&entry) {
            let _ = atomic_write(&cache_path, &text);
        }

        info!(slug = %entry.slug, "cached repo summary");
        cached.push(entry);
    }
    cached
}

/// Increment the commits_by_gitzi counter for a repo slug.
pub fn increment_commits(slug: &str) {
    let cache_dir = home::repo_cache_dir();
    let path = cache_dir.join(format!("{slug}.toml"));
    if let Some(mut entry) = load_cache_entry(&path) {
        entry.commits_by_gitzi += 1;
        if let Ok(text) = toml::to_string_pretty(&entry) {
            let _ = atomic_write(&path, &text);
        }
    }
}

fn load_cache_entry(path: &Path) -> Option<RepoCache> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

fn path_to_slug(path: &Path) -> String {
    // Use the last 2 path components: "email-catcher/backend" -> "email-catcher-backend"
    let components: Vec<&str> = path
        .components()
        .rev()
        .take(2)
        .map(|c| c.as_os_str().to_str().unwrap_or("unknown"))
        .collect();
    components.into_iter().rev().collect::<Vec<_>>().join("-")
}

/// Generate a summary and labels from repo heuristics.
fn generate_summary(repo_path: &Path) -> (String, Vec<String>) {
    let mut labels = Vec::new();
    let mut summary_parts = Vec::new();

    // Check package.json
    let pkg_json = repo_path.join("package.json");
    if pkg_json.exists()
        && let Ok(text) = std::fs::read_to_string(&pkg_json)
        && let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&text)
    {
        labels.push("node".to_string());

        if let Some(desc) = pkg.get("description").and_then(|v| v.as_str()) {
            summary_parts.push(desc.to_string());
        }

        if let Some(deps) = pkg.get("dependencies").and_then(|v| v.as_object()) {
            if deps.contains_key("hono") { labels.push("hono".to_string()); }
            if deps.contains_key("react") { labels.push("react".to_string()); }
            if deps.contains_key("vue") { labels.push("vue".to_string()); }
            if deps.contains_key("express") { labels.push("express".to_string()); }
            if deps.contains_key("next") { labels.push("next".to_string()); }
            if deps.contains_key("@aws-sdk/client-s3")
                || deps.contains_key("@aws-sdk/client-dynamodb")
            {
                labels.push("aws".to_string());
            }
        }

        if let Some(dev_deps) =
            pkg.get("devDependencies").and_then(|v| v.as_object())
        {
            if dev_deps.contains_key("typescript") {
                labels.push("typescript".to_string());
            }
            if dev_deps.contains_key("vitest") {
                labels.push("vitest".to_string());
            }
        }
    }

    // Check Cargo.toml
    let cargo_toml = repo_path.join("Cargo.toml");
    if cargo_toml.exists()
        && let Ok(text) = std::fs::read_to_string(&cargo_toml)
        && let Ok(cargo) = toml::from_str::<toml::Value>(&text)
    {
        labels.push("rust".to_string());

        if let Some(desc) = cargo
            .get("package")
            .and_then(|p| p.get("description"))
            .and_then(|v| v.as_str())
        {
            summary_parts.push(desc.to_string());
        }

        if let Some(deps) = cargo.get("dependencies").and_then(|v| v.as_table()) {
            if deps.contains_key("tokio") { labels.push("tokio".to_string()); }
            if deps.contains_key("axum") { labels.push("axum".to_string()); }
            if deps.contains_key("ratatui") { labels.push("tui".to_string()); }
            if deps.contains_key("reqwest") { labels.push("http".to_string()); }
        }
    }

    // Check for Terraform files
    let has_tf = std::fs::read_dir(repo_path)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.path().extension().and_then(|ext| ext.to_str()) == Some("tf")
            })
        })
        .unwrap_or(false);
    if has_tf {
        labels.push("terraform".to_string());
        labels.push("infrastructure".to_string());
    }

    // Fall back to README first line for summary
    if summary_parts.is_empty() {
        let readme = repo_path.join("README.md");
        if readme.exists()
            && let Ok(text) = std::fs::read_to_string(&readme)
        {
            if let Some(first_line) =
                text.lines().find(|l| !l.is_empty() && !l.starts_with('#'))
            {
                summary_parts.push(first_line.trim().to_string());
            } else if let Some(heading) = text.lines().find(|l| l.starts_with("# ")) {
                summary_parts.push(heading.trim_start_matches("# ").to_string());
            }
        }
    }

    let summary = if summary_parts.is_empty() {
        format!(
            "Repository at {}",
            repo_path.file_name().unwrap_or_default().to_string_lossy()
        )
    } else {
        summary_parts.join(". ")
    };

    labels.sort();
    labels.dedup();
    (summary, labels)
}
