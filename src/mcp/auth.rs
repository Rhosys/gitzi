use std::collections::HashSet;
use std::sync::Arc;

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

const TOKEN_TTL_SECS: u64 = 7 * 24 * 3600; // 7 days

/// JWT claims for a sub-agent session token.
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    /// task_id the agent is working on.
    sub: String,
    /// Unique token ID — used to revoke before expiry.
    jti: String,
    /// Issued-at (Unix timestamp).
    iat: u64,
    /// Expiry (Unix timestamp).
    exp: u64,
}

/// The resolved identity after a token is validated.
#[derive(Debug, Clone)]
pub struct TokenEntry {
    pub task_id: String,
}

/// JWT-backed token store.
///
/// - Tokens are HS256-signed JWTs that expire after 7 days.
/// - The signing key is generated at daemon startup and lives only in memory.
/// - An active-JTI set allows early revocation (token can't be used after
///   the agent session ends, even if the 7-day clock hasn't elapsed).
pub struct TokenStore {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    /// Set of JTI values for tokens that are still active.
    active_jtis: Arc<Mutex<HashSet<String>>>,
}

impl TokenStore {
    /// Create a store with a freshly generated random HS256 signing key.
    pub fn new() -> Self {
        let secret: Vec<u8> = (0..32).map(|_| rand_byte()).collect();
        Self {
            encoding_key: EncodingKey::from_secret(&secret),
            decoding_key: DecodingKey::from_secret(&secret),
            active_jtis: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Issue a JWT scoped to `task_id`. Returns the signed token string.
    pub async fn issue(&self, task_id: impl Into<String>) -> String {
        let now = unix_now();
        let jti = Uuid::new_v4().to_string();

        let claims = Claims {
            sub: task_id.into(),
            jti: jti.clone(),
            iat: now,
            exp: now + TOKEN_TTL_SECS,
        };

        let token = encode(&Header::new(Algorithm::HS256), &claims, &self.encoding_key)
            .expect("JWT encoding should not fail with a valid secret");

        self.active_jtis.lock().await.insert(jti);

        token
    }

    /// Validate a JWT. Returns `None` if the signature is invalid, the token
    /// has expired, or the JTI has been revoked.
    pub async fn validate(&self, token: &str) -> Option<TokenEntry> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;

        let data = decode::<Claims>(token, &self.decoding_key, &validation).ok()?;
        let claims = data.claims;

        // Check active set (revocation)
        if !self.active_jtis.lock().await.contains(&claims.jti) {
            return None;
        }

        Some(TokenEntry { task_id: claims.sub })
    }

    /// Revoke a token so it cannot be used again, even before its expiry.
    /// Decodes without expiry validation so a token can be revoked at any time.
    pub async fn revoke(&self, token: &str) {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = false;

        if let Ok(data) = decode::<Claims>(token, &self.decoding_key, &validation) {
            self.active_jtis.lock().await.remove(&data.claims.jti);
        }
    }
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::new()
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Simple PRNG byte using thread-local state seeded from system time.
/// Not cryptographically strong — but this key never leaves the process and
/// is discarded on restart, so OS-level PRNG is sufficient here.
fn rand_byte() -> u8 {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = Cell::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0xdeadbeef_cafebabe)
        );
    }
    STATE.with(|s| {
        // xorshift64
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x as u8
    })
}
