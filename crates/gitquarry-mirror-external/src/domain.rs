//! Thin `AppContext`-level wrapper over [`crate::access`] — same `domain`/
//! `web` boundary as `gitquarry_core::domain::pull_request`: transport-agnostic,
//! `anyhow::Result`, no CSRF/session/audit concerns (those stay in
//! `webui::repo_settings::mirrors`).

use crate::access::{self, ExternalMirror};
use anyhow::{Result, bail};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::{Cas, uid::Uid};

/// Creates a brand-new mirror (mints its id) or, if `mirror.id` is already
/// set and `expected_etag` is `Some`, edits an existing one — both paths
/// validate before writing.
pub async fn create(ctx: &AppContext, uid: &Uid, mut mirror: ExternalMirror) -> Result<ExternalMirror> {
    mirror.id = access::generate_mirror_id();
    match access::set_external_mirror(ctx.backend().as_ref(), &ctx.cipher, uid, &mirror, None).await? {
        Cas::Ok => {
            refresh_mirrors_enabled_index(ctx, uid).await?;
            Ok(mirror)
        }
        Cas::Conflict => bail!("mirror id collision, try again"),
    }
}

/// `Ok(Cas::Conflict)`, not an `Err`, on a lost CAS race — same contract as
/// `domain::repo::set_topics`/`set_perm` and every other settings-tab save,
/// so the web layer can render the shared "edited concurrently, reload and
/// retry" flash instead of a generic error page.
pub async fn update(ctx: &AppContext, uid: &Uid, mirror: &ExternalMirror, expected_etag: &str) -> Result<Cas> {
    match access::set_external_mirror(ctx.backend().as_ref(), &ctx.cipher, uid, mirror, Some(expected_etag)).await? {
        Cas::Ok => {
            refresh_mirrors_enabled_index(ctx, uid).await?;
            Ok(Cas::Ok)
        }
        Cas::Conflict => Ok(Cas::Conflict),
    }
}

pub async fn get(ctx: &AppContext, uid: &Uid, mirror_id: &str) -> Result<Option<(ExternalMirror, String)>> {
    let (mirror, etag) = access::get_external_mirror(ctx.backend().as_ref(), &ctx.cipher, uid, mirror_id).await?;
    Ok(mirror.zip(etag))
}

pub async fn list(ctx: &AppContext, uid: &Uid) -> Result<Vec<ExternalMirror>> {
    access::list_external_mirrors(ctx.backend().as_ref(), &ctx.cipher, uid).await
}

/// Same listing, with each mirror's etag — see
/// `access::list_external_mirrors_with_etags`'s own doc comment for why
/// this is a separate function rather than [`list`] itself changing shape.
pub async fn list_with_etags(ctx: &AppContext, uid: &Uid) -> Result<Vec<(ExternalMirror, String)>> {
    access::list_external_mirrors_with_etags(ctx.backend().as_ref(), &ctx.cipher, uid).await
}

pub async fn delete(ctx: &AppContext, uid: &Uid, mirror_id: &str) -> Result<()> {
    access::delete_external_mirror(ctx.backend().as_ref(), &ctx.cipher, uid, mirror_id).await;
    refresh_mirrors_enabled_index(ctx, uid).await
}

/// Re-derives `.conf/index/mirrors-enabled/<uid>` from this repo's *actual*
/// current mirrors, rather than tracking "did this specific write turn a
/// mirror on or off" at each call site — simpler, and correct regardless of
/// which field changed or how many mirrors this repo has. The index itself
/// lives in `gitquarry_core::storage::settings` now, not here — see
/// `docs/mirror-external-extraction-concept.md` Phase 1: `mirror::hook`'s
/// pre-receive path reads the same index directly, and it can't depend on
/// this crate without a cycle.
async fn refresh_mirrors_enabled_index(ctx: &AppContext, uid: &Uid) -> Result<()> {
    let any_enabled = list(ctx, uid).await?.iter().any(|m| m.enabled);
    gitquarry_core::storage::settings::set_mirrors_enabled_index(ctx.backend().as_ref(), &ctx.cipher, uid, any_enabled)
        .await;
    Ok(())
}
