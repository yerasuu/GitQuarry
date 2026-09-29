//! Fixtures every crate's `tests/` shares: a local redb bucket and the
//! `AppContext` over it, real git history, and HTTP helpers for routers.
//! Built only on the crates' public APIs, so a test using this still tests
//! exactly the shipped surface. Never a dependency of anything but tests.

mod git;
mod web;

pub use git::{Work, git};
pub use web::{Page, Session, send};

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user::{self, NewUser};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Every password user [`user`] creates logs in with this.
pub const PASSWORD: &str = "correct horse battery staple";

pub fn backend(root: &Path) -> Arc<dyn Backend> {
    Arc::new(s3db::LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }))
}

/// Argon2id is deliberately slow, so this is derived once per process.
pub fn cipher() -> Arc<Cipher> {
    static CIPHER: std::sync::LazyLock<Arc<Cipher>> = std::sync::LazyLock::new(|| {
        Arc::new(Cipher::derive("test passphrase", &[7u8; s3db_crypto::SALT_LEN]).expect("deriving the test cipher"))
    });
    Arc::clone(&CIPHER)
}

/// An instance over a fresh bucket under `root`, with every optional feature
/// off. A test that needs one on overrides just that field:
/// `AppContext { federation: Some(..), ..ctx(root) }`.
pub fn ctx(root: &Path) -> AppContext {
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.join("mirrors"),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend: backend(&root.join("bucket")),
        locks: Default::default(),
        cipher: cipher(),
        mirror_max_age: Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

/// A user who logs in with [`PASSWORD`]. The first one switches login
/// enforcement on for the whole instance.
pub async fn user(ctx: &AppContext, username: &str, is_admin: bool) {
    let new_user = NewUser {
        username,
        email: None,
        pubkey: None,
        password_plain: Some(PASSWORD),
        is_admin,
        repos_field: "",
        locale: gitquarry_i18n::Locale::En,
    };
    user::create(ctx, new_user)
        .await
        .expect("creating the test user")
        .expect("test username already taken");
}

/// A federation process over a fresh bucket under `root`, whose `core`
/// client points at `core_url` (an unreachable port when the test never
/// calls core).
pub fn federation_state(root: &Path, core_url: &str) -> Arc<gitquarry_federation::state::FederationState> {
    let bucket = root.join("bucket");
    Arc::new(gitquarry_federation::state::FederationState {
        backend: backend(&bucket),
        cipher: cipher(),
        core: gitquarry_federation::core_client::CoreClient::new(core_url.to_string(), "unused".to_string()),
        backend_config: s3db::BackendConfig::Local(LocalParams {
            bucket: bucket.to_string_lossy().to_string(),
            engine: LocalEngine::Redb,
        }),
        gossip: Default::default(),
        self_announce: None,
        rate_limiter: Default::default(),
        rate_limit_public_max: 60,
        rate_limit_public_window: Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: Duration::from_secs(60),
        rate_limit_peer_override_by_ip: true,
    })
}
