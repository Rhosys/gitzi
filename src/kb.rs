//! Embedded knowledge base for the main agent.
//! Content is compiled into the binary and available at runtime for answering
//! user questions about gitzi.

/// All KB articles, compiled into the binary.
pub const KB_ARTICLES: &[(&str, &str)] = &[
    ("overview", include_str!("../kb/overview.md")),
    ("configuration", include_str!("../kb/configuration.md")),
    ("forks", include_str!("../kb/forks.md")),
];

/// Search KB articles for content matching a query (case-insensitive substring).
/// Returns matching articles with their titles.
pub fn search(query: &str) -> Vec<(&'static str, &'static str)> {
    let lower = query.to_lowercase();
    KB_ARTICLES
        .iter()
        .filter(|(_, content)| content.to_lowercase().contains(&lower))
        .copied()
        .collect()
}

/// Get a specific KB article by name.
pub fn get(name: &str) -> Option<&'static str> {
    KB_ARTICLES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}
