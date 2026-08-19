//! One binary, one unified server, plus one entry point that only git
//! invokes (not run by a human), plus three operator maintenance commands
//! (see [`gitquarry_domain::commands`]):
//! - `gitquarry --bucket NAME [--web-bind ...] [--ssh-bind ...]`: starts
//!   the ssh:// server and the web UI (browsing + git smart-HTTP) together,
//!   in the same process. [`serve`] builds that from [`gitquarry_core`]
//!   (ssh://, git-smart-HTTP, domain/storage) and [`gitquarry_webui`]
//!   (the browsing UI) — the one place in the workspace that depends on
//!   both, since neither of those two can depend on the other (see
//!   `docs/webui-crate-split-concept.md`).
//! - `pre-receive-hook`: internal use, invoked by the hook installed on each
//!   mirror (not run by a human). Spawns `gitquarry-mirror-external sync`
//!   detached right after a push is accepted, to sync that repo's external
//!   mirrors without making the push itself wait on it — see
//!   `docs/mirror-external-extraction-concept.md`.
//! - `fsck`: human-run, checks for stray objects — see [`gitquarry_domain::commands::fsck`].
//! - `repack`: human-run, consolidates a repo's packs — see [`gitquarry_domain::commands::repack`].
//! - `backfill-repo-index`: human-run, one-time backfill for repos/grants
//!   that existed before the owner/visibility/permission indexes did — see
//!   [`gitquarry_domain::commands::backfill_repo_index`].

use anyhow::Result;
use gitquarry::serve;
use gitquarry_core::mirror;
use gitquarry_domain::commands;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match dispatch().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gitquarry: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn dispatch() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("pre-receive-hook") => run_hook().await,
        Some("fsck") => commands::fsck::run(&args[2..]).await,
        Some("repack") => commands::repack::run(&args[2..]).await,
        Some("backfill-repo-index") => commands::backfill_repo_index::run(&args[2..]).await,
        _ => serve::run(&args[1..]).await,
    }
}

async fn run_hook() -> Result<()> {
    if mirror::run_pre_receive_hook().await? {
        Ok(())
    } else {
        // The reason was already printed with detail; exit != 0 so git
        // rejects the push.
        std::process::exit(1);
    }
}
