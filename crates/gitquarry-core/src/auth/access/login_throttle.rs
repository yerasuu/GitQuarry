//! Running count of users with a password set — a single tiny counter
//! object instead of fetching every user just to answer one boolean.

use crate::crypto::Cipher;
use crate::storage::Backend;
use anyhow::Result;
use s3db::Singleton;
use serde::{Deserialize, Serialize};

const PASSWORD_LOGIN_COUNT: Singleton = Singleton::new(".conf/index/password-login-count.json");

#[derive(Debug, Default, Serialize, Deserialize)]
struct PasswordLoginCount {
    #[serde(default)]
    count: u32,
}

/// Reads the running count of users with a password set, for
/// `gitquarry_webui::auth::web::login::password_login_available` — a single tiny GET
/// instead of fetching every user to answer one boolean. `None` means no
/// user has ever had a password set (the counter object is only ever
/// created by the first [`bump_password_login_count`] that has anything to
/// count), same as `Some(0)` for that caller's purposes.
pub async fn password_login_count(backend: &dyn Backend, cipher: &Cipher) -> Result<Option<u32>> {
    let (count, etag) = PASSWORD_LOGIN_COUNT.get::<PasswordLoginCount>(backend, cipher).await?;
    Ok(etag.is_some().then_some(count.count))
}

/// Best-effort ±1 on the password-login counter — same single-attempt,
/// no-retry pattern as `RepoConfig::star_count`/`fork_count`: a lost race
/// just drifts the count by one, not worth a retry loop for what's only
/// ever used to decide whether to show a password field at all.
pub async fn bump_password_login_count(backend: &dyn Backend, cipher: &Cipher, delta: i32) {
    let Ok((mut count, etag)) = PASSWORD_LOGIN_COUNT.get::<PasswordLoginCount>(backend, cipher).await else {
        return;
    };
    count.count = if delta.is_negative() {
        count.count.saturating_sub(delta.unsigned_abs())
    } else {
        count.count.saturating_add(delta.unsigned_abs())
    };
    let _ = PASSWORD_LOGIN_COUNT.put(backend, cipher, &count, etag.as_deref()).await;
}
