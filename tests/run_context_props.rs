// Feature: dispatcher-audit-fixes, Property 14: RunContext repo_root from session state
// **Validates: Requirements 8.3**

use std::path::PathBuf;
use std::sync::Mutex;
use proptest::prelude::*;

/// Global mutex to serialize tests that manipulate HOME env var.
static HOME_MUTEX: Mutex<()> = Mutex::new(());

// ─── Property 14: RunContext repo_root from session state ─────────────────────

proptest! {
    #![proptest_config(ProptestConfig { cases: 100, ..Default::default() })]

    /// Property 14: RunContext repo_root from session state
    ///
    /// For any session state containing a repo path, `repo_path()` returns that
    /// path. The value is never `PathBuf::from(".")` when a valid repo file exists.
    #[test]
    fn repo_root_from_session_state(
        path_segments in prop::collection::vec("[a-z][a-z0-9_]{2,10}", 2..=5),
    ) {
        let _lock = HOME_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let random_path = format!("/{}", path_segments.join("/"));
        let expected = PathBuf::from(&random_path);

        // Create a temp HOME with session state
        let tmp = tempfile::tempdir().unwrap();
        let gitzi_dir = tmp.path().join(".gitzi");
        let session_id = "test-session-001";

        // Write the current session file
        std::fs::create_dir_all(&gitzi_dir).unwrap();
        std::fs::write(gitzi_dir.join("current"), session_id).unwrap();

        // Write the repo path file
        let session_dir = gitzi_dir.join(session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("repo"), &random_path).unwrap();

        // Override HOME so dirs::home_dir() returns our temp dir
        let old_home = std::env::var("HOME").ok();
        // SAFETY: serialized via HOME_MUTEX — no concurrent env access
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let result = gitzi::state::home::repo_path();

        // Restore HOME
        match old_home {
            Some(ref h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        // The repo_root must equal the session path
        prop_assert_eq!(
            &result,
            &expected,
            "repo_path() should return the session state path, got {:?}",
            result,
        );

        // It must NEVER be PathBuf::from(".")
        prop_assert_ne!(
            result,
            PathBuf::from("."),
            "repo_path() must never return '.' when session state has a repo path"
        );
    }

    /// Property 14 (negative): When no repo file exists, repo_path() falls back.
    /// This validates the boundary: without a repo file, the function returns ".".
    #[test]
    fn repo_root_fallback_without_session(
        session_id in "[a-z0-9]{8,16}",
    ) {
        let _lock = HOME_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Create a temp HOME with session state but NO repo file
        let tmp = tempfile::tempdir().unwrap();
        let gitzi_dir = tmp.path().join(".gitzi");

        std::fs::create_dir_all(&gitzi_dir).unwrap();
        std::fs::write(gitzi_dir.join("current"), &session_id).unwrap();

        // Create the session dir without a repo file
        let session_dir = gitzi_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir).unwrap();

        // Override HOME
        let old_home = std::env::var("HOME").ok();
        // SAFETY: serialized via HOME_MUTEX — no concurrent env access
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let result = gitzi::state::home::repo_path();

        // Restore HOME
        match old_home {
            Some(ref h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        // Without a repo file, falls back to "."
        prop_assert_eq!(
            result,
            PathBuf::from("."),
            "repo_path() should fall back to '.' when no repo file exists"
        );
    }
}
