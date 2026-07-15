// MCP authentication — TokenStore JWT lifecycle tests
// **Validates: MCP security model (issue/validate/revoke roundtrip, cross-store isolation)**

use gitzi::mcp::auth::TokenStore;
use proptest::prelude::*;

// ── Unit tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn issue_validate_roundtrip() {
    let store = TokenStore::new();
    let token = store.issue("task-abc").await;
    let entry = store.validate(&token).await;
    assert!(entry.is_some(), "freshly issued token should validate");
    assert_eq!(entry.unwrap().task_id, "task-abc");
}

#[tokio::test]
async fn validate_preserves_task_id() {
    let store = TokenStore::new();
    let task_id = "epic-1/task-widget-42";
    let token = store.issue(task_id).await;
    let entry = store.validate(&token).await.expect("should be valid");
    assert_eq!(entry.task_id, task_id);
}

#[tokio::test]
async fn revoke_prevents_validate() {
    let store = TokenStore::new();
    let token = store.issue("task-to-revoke").await;
    assert!(
        store.validate(&token).await.is_some(),
        "token should be valid before revoke"
    );
    store.revoke(&token).await;
    assert!(
        store.validate(&token).await.is_none(),
        "revoked token must not validate"
    );
}

#[tokio::test]
async fn revoke_does_not_affect_other_tokens() {
    let store = TokenStore::new();
    let token_a = store.issue("task-a").await;
    let token_b = store.issue("task-b").await;
    store.revoke(&token_a).await;
    assert!(
        store.validate(&token_b).await.is_some(),
        "token_b should still be valid"
    );
    assert!(
        store.validate(&token_a).await.is_none(),
        "token_a should be revoked"
    );
}

#[tokio::test]
async fn malformed_token_is_rejected() {
    let store = TokenStore::new();
    assert!(store.validate("not.a.valid.jwt").await.is_none());
    assert!(store.validate("").await.is_none());
    assert!(
        store
            .validate("eyJhbGciOiJIUzI1NiJ9.garbage.sig")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn different_store_rejects_foreign_token() {
    let store_a = TokenStore::new();
    let store_b = TokenStore::new();
    let token = store_a.issue("task-x").await;
    assert!(
        store_b.validate(&token).await.is_none(),
        "token signed by store_a must not validate against store_b (different key)"
    );
}

#[tokio::test]
async fn multiple_tokens_independently_revocable() {
    let store = TokenStore::new();
    let tokens: Vec<String> =
        futures_or_sequential((0..5).map(|i| store.issue(format!("task-{i}"))).collect()).await;

    // Revoke only odd-indexed tokens
    for (i, token) in tokens.iter().enumerate() {
        if i % 2 == 1 {
            store.revoke(token).await;
        }
    }

    for (i, token) in tokens.iter().enumerate() {
        if i % 2 == 0 {
            assert!(
                store.validate(token).await.is_some(),
                "even-indexed token {i} should still be valid"
            );
        } else {
            assert!(
                store.validate(token).await.is_none(),
                "odd-indexed token {i} should be revoked"
            );
        }
    }
}

/// Run a Vec of futures sequentially (avoids needing futures crate).
async fn futures_or_sequential<T>(futs: Vec<impl std::future::Future<Output = T>>) -> Vec<T> {
    let mut results = Vec::with_capacity(futs.len());
    for f in futs {
        results.push(f.await);
    }
    results
}

// ── Property tests ────────────────────────────────────────────────────────────

proptest! {
    /// For any task ID, issue → validate preserves the task_id exactly.
    #[test]
    fn prop_task_id_preserved_through_roundtrip(task_id in "[a-z0-9][a-z0-9\\-]{0,48}") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let entry = rt.block_on(async {
            let store = TokenStore::new();
            let token = store.issue(&task_id).await;
            store.validate(&token).await
        });

        let entry = match entry {
            Some(e) => e,
            None => {
                prop_assert!(false, "freshly issued token should validate for task_id={task_id}");
                return Ok(());
            }
        };
        prop_assert_eq!(&entry.task_id, &task_id);
    }

    /// After revocation, tokens are always rejected regardless of task ID.
    #[test]
    fn prop_revoke_always_invalidates(task_id in "[a-z0-9][a-z0-9\\-]{0,48}") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let still_valid = rt.block_on(async {
            let store = TokenStore::new();
            let token = store.issue(&task_id).await;
            store.revoke(&token).await;
            store.validate(&token).await.is_some()
        });

        prop_assert!(!still_valid, "revoked token should never validate (task_id={task_id})");
    }

    /// Tokens from one store are always rejected by a different store (different signing key).
    #[test]
    fn prop_cross_store_tokens_rejected(task_id in "[a-z0-9][a-z0-9\\-]{0,48}") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let accepted = rt.block_on(async {
            let store_a = TokenStore::new();
            let store_b = TokenStore::new();
            let token = store_a.issue(&task_id).await;
            store_b.validate(&token).await.is_some()
        });

        prop_assert!(!accepted, "foreign token should never validate (task_id={task_id})");
    }
}
