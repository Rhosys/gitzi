// Property: repo_path() returns the current working directory (no session indirection).
// The old session-based repo_path is gone — repos are discovered via config.toml globs.

use std::path::PathBuf;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 10, ..Default::default() })]

    /// repo_path() returns the current working directory.
    /// It never returns a fabricated path — it reflects actual cwd.
    #[test]
    fn repo_path_returns_cwd(_seed in 0u32..100) {
        let result = gitzi::state::home::repo_path();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        prop_assert_eq!(result, cwd);
    }
}
