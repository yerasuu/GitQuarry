//! Bootstraps and runs the unified server (`gitquarry [--bucket ...] ...`):
//! CLI flags, config/bucket resolution (falling back to the setup wizard on
//! first run), `AppContext` assembly, and ssh:// + web running together
//! until shutdown. Split out of `main` so the entry-point dispatcher there
//! stays just that — routing argv to the right entry point — instead of
//! also carrying ~150 lines of startup wiring.

use crate::setup;
use anyhow::{Context, Result, bail};
use gitquarry_auth_crypto::oidc;
use gitquarry_core::app::{AppContext, Locks};
use gitquarry_core::auth::access;
use gitquarry_core::backup;
use gitquarry_core::events::RepoEvent;
use gitquarry_core::git::ssh;
use gitquarry_core::mirror::{deleted_repo_sweep, gc};
use gitquarry_core::{config, storage};
use gitquarry_webui::webui;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

const DEFAULT_WEB_BIND: &str = "0.0.0.0:8080";
const DEFAULT_SSH_BIND: &str = "0.0.0.0:2222";
/// A local mirror not queried in this long gets evicted by
/// `gitquarry_core::mirror::gc`.
const DEFAULT_MIRRORS_MAX_AGE: &str = "3d";
/// Soft cap on the mirrors dir's total size; see `gitquarry_core::mirror::gc`.
const DEFAULT_MIRRORS_MAX_SIZE: &str = "5G";

const USAGE: &str = "\
usage:
  gitquarry [--bucket NAME] [--web-bind 0.0.0.0:8080] [--ssh-bind 0.0.0.0:2222]
              [--mirrors-dir PATH] [--mirrors-max-age 3d] [--mirrors-max-size 5G]

Starts, in the same process, the ssh:// server (push/clone/fetch with
pubkey auth and per-repo permissions), git smart-HTTP over the same web
port (push/clone/fetch with basic auth and per-repo permissions), and the
read-only web UI -- the only two publicly supported transports are ssh://
and http. --bucket is optional when the config has only one bucket or
names a `default_bucket`.

Local mirrors (`--mirrors-dir`) are a cache: a periodic sweep deletes the
ones not queried within `--mirrors-max-age` (default 3d) and, if the
directory is still above `--mirrors-max-size` (default 5G), the
least-used ones until back under the limit. They get recreated from S3
on the next query.

'gitquarry fsck [--bucket NAME] [--apply]' looks for objects that gitquarry
doesn't write inside a repo's prefix -- see that command with --help.

'gitquarry repack <repo> [--bucket NAME] [--apply]' consolidates a repo's
accumulated packs into one -- see that command with --help.

'gitquarry backup [--bucket NAME] [--dest DEST]' runs one backup now, the
same one /_settings/backup schedules -- see that command with --help.

'gitquarry restore <snapshot-id> [--apply]' writes a snapshot back over the
live data -- see that command with --help.";

#[derive(Debug, Default)]
struct Args {
    web_bind: Option<String>,
    ssh_bind: Option<String>,
    mirrors_dir: Option<PathBuf>,
    mirrors_max_age: Option<String>,
    mirrors_max_size: Option<String>,
    bucket: Option<String>,
}

fn parse_args(rest: &[String]) -> Result<Option<Args>> {
    let mut args = Args::default();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let mut value =
            |flag: &str| -> Result<String> { it.next().cloned().with_context(|| format!("{flag} needs a value")) };
        match arg.as_str() {
            "--web-bind" => args.web_bind = Some(value("--web-bind")?),
            "--ssh-bind" => args.ssh_bind = Some(value("--ssh-bind")?),
            "--mirrors-dir" => args.mirrors_dir = Some(PathBuf::from(value("--mirrors-dir")?)),
            "--mirrors-max-age" => args.mirrors_max_age = Some(value("--mirrors-max-age")?),
            "--mirrors-max-size" => args.mirrors_max_size = Some(value("--mirrors-max-size")?),
            "--bucket" => args.bucket = Some(value("--bucket")?),
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "--version" | "-V" => {
                println!("gitquarry {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    Ok(Some(args))
}

/// Starts the ssh:// server and the web UI (browsing + git smart-HTTP)
/// together, in the same process: a single command manages everything it
/// needs, no invoking two separate binaries/subcommands by hand.
pub async fn run(rest: &[String]) -> Result<()> {
    let Some(args) = parse_args(rest)? else {
        return Ok(()); // --help / --version
    };

    let config_path = config::Config::resolve_path()?;
    // No `gitquarry.toml` anywhere in the resolution order at all (as
    // opposed to one that exists but is broken/misconfigured, which still
    // hard-fails below exactly as before): first run, hand off to the setup
    // wizard instead of erroring out. It serves one form on `--web-bind`,
    // verifies everything against S3 for real, writes the config, and
    // returns exactly what the branch below would've loaded from it — same
    // process, no restart needed to pick up what it just wrote.
    let (bucket, _bucket_cfg, backend, cipher, tracker_cfg, allow_push_create, trust_forwarded_for) = if config_path
        .is_file()
    {
        let conf = config::Config::load_from(&config_path)?;
        let tracker_cfg = conf.tracker().cloned();
        let allow_push_create = conf.allow_push_create();
        let trust_forwarded_for = conf.trust_forwarded_for();
        config::warn_if_world_readable(&config_path);
        // Without --bucket: whatever `default_bucket` names, or the single
        // configured bucket. Buckets that exist only as a backup destination
        // or a mirror target are never served, so a config with several is
        // ordinary and the default has to be written down.
        let bucket = match args.bucket {
            Some(b) => b,
            None => conf
                .default_bucket_name()
                .map(str::to_string)
                .context("--bucket NAME required (config has more than one bucket and no default_bucket)")?,
        };
        // Validates early, with a good error message, before binding ports.
        let bucket_cfg = conf.bucket(&bucket)?.clone();
        let backend = storage::build_backend(&bucket_cfg)?;
        // Derives (or creates, if the bucket is new) this bucket's cipher.
        // Fails hard if the passphrase doesn't match: starting with the
        // wrong key would leave every operation failing to decrypt, better
        // to cut it off here with a clear message than after binding any
        // port.
        let cipher = gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase)
            .await
            .context(
                "couldn't reach the storage backend to read/create the bucket's encryption config \
                 (retries already exhausted -- check the endpoint is up and reachable, not just this process)",
            )?;
        // Config exists and connects fine, but the bucket itself has never
        // had an admin user created (infra-as-code wrote `gitquarry.toml`
        // ahead of the first real deploy, or it points at a brand-new
        // bucket) — same first-run wizard as a missing config, just
        // pre-filled with what's already known instead of starting blank.
        if access::list_usernames(backend.as_ref()).await?.is_empty() {
            let web_bind = args.web_bind.clone().unwrap_or_else(|| DEFAULT_WEB_BIND.to_string());
            let outcome = setup::run(
                &config_path,
                &web_bind,
                Some(setup::Prefill {
                    name: bucket,
                    bucket_cfg,
                }),
            )
            .await?;
            (
                outcome.bucket,
                outcome.bucket_cfg,
                outcome.backend,
                outcome.cipher,
                tracker_cfg,
                allow_push_create,
                trust_forwarded_for,
            )
        } else {
            (
                bucket,
                bucket_cfg,
                backend,
                cipher,
                tracker_cfg,
                allow_push_create,
                trust_forwarded_for,
            )
        }
    } else {
        // First run, no `gitquarry.toml` at all yet — `[tracker]` can't
        // exist either, and the setup wizard writes a bucket-only config, so
        // `allow_push_create`/`trust_forwarded_for` are both off too until an
        // operator opts in by hand.
        let web_bind = args.web_bind.clone().unwrap_or_else(|| DEFAULT_WEB_BIND.to_string());
        let outcome = setup::run(&config_path, &web_bind, None).await?;
        (
            outcome.bucket,
            outcome.bucket_cfg,
            outcome.backend,
            outcome.cipher,
            None,
            false,
            false,
        )
    };
    let cipher = Arc::new(cipher);

    // Remote config (.conf/global.json in the bucket itself, editable from
    // the UI). Order: explicit CLI flag > config in S3 > default.
    // A failure here isn't fatal (can start with defaults) but is worth
    // warning about: it's almost always bad credentials or a bad endpoint.
    let settings = match storage::settings::get_settings(backend.as_ref(), &cipher).await {
        Ok((settings, _etag)) => settings,
        Err(e) => {
            warn!("couldn't read the bucket config, using defaults: {e:#}");
            storage::entities::ServerSettings::default()
        }
    };

    // Before any of `settings`'s fields get partially moved out below: OIDC
    // discovery needs its own 4 fields read out. A misconfigured or
    // unreachable IdP isn't fatal — same "warn and use a default" treatment
    // as the settings fetch itself just above, ssh:// and the web server
    // shouldn't go down because of it.
    let oidc_settings = oidc::OidcSettings {
        issuer_url: settings.oidc_issuer_url.clone(),
        client_id: settings.oidc_client_id.clone(),
        client_secret: settings.oidc_client_secret.clone(),
        redirect_base_url: settings.oidc_redirect_base_url.clone(),
    };
    let oidc = match oidc::discover(&oidc_settings).await {
        Ok(client) => client,
        Err(e) => {
            warn!("OIDC unavailable, continuing without login: {e:#}");
            None
        }
    };

    // Resolved once here, same "restart to apply" contract every other
    // `ServerSettings` field already has (`settings/web.rs`'s own flash
    // message) — see `webui::init_source_link`'s own doc comment on why
    // this isn't re-read per page render.
    webui::init_source_link(webui::resolve_source_link_href(settings.source_link.as_deref()));

    let web_bind = args
        .web_bind
        .or(settings.web_bind)
        .unwrap_or_else(|| DEFAULT_WEB_BIND.to_string());
    let ssh_bind = args
        .ssh_bind
        .or(settings.ssh_bind)
        .unwrap_or_else(|| DEFAULT_SSH_BIND.to_string());
    let mirrors_root = args
        .mirrors_dir
        .or(settings.mirrors_dir.map(PathBuf::from))
        .unwrap_or_else(|| gitquarry_core::app::default_mirrors_root(&bucket));
    let mirror_max_age = gc::parse_age(
        &args
            .mirrors_max_age
            .or(settings.mirrors_max_age)
            .unwrap_or_else(|| DEFAULT_MIRRORS_MAX_AGE.to_string()),
    )
    .context("invalid --mirrors-max-age")?;
    let mirror_max_size_bytes = gc::parse_size(
        &args
            .mirrors_max_size
            .or(settings.mirrors_max_size)
            .unwrap_or_else(|| DEFAULT_MIRRORS_MAX_SIZE.to_string()),
    )
    .context("invalid --mirrors-max-size")?;
    let exe_path = std::env::current_exe()
        .context("couldn't resolve the current binary's path")?
        .canonicalize()
        .context("couldn't canonicalize the current binary's path")?;

    tokio::fs::create_dir_all(&mirrors_root)
        .await
        .with_context(|| format!("creating mirrors dir {}", mirrors_root.display()))?;
    // Absolute so it can be compared against mirror path prefixes and so
    // subprocesses with a different cwd resolve it the same way.
    let mirrors_root = mirrors_root
        .canonicalize()
        .with_context(|| format!("resolving {}", mirrors_root.display()))?;

    // Rolls back any `s3db::txn::run` transaction left `Preparing` by a
    // previous instance that crashed mid-way (a create-only namespace/
    // pubkey/email/user-object batch, see `auth::domain::user::create`) — a
    // sweep failure here is never fatal to startup, same "log and move on"
    // treatment as the settings/OIDC fetches just above.
    match s3db::txn::recover_stale(backend.as_ref(), &cipher, std::time::Duration::from_secs(600)).await {
        Ok(0) => {}
        Ok(n) => info!("recovered {n} stale transaction(s) left by a previous crash"),
        Err(e) => warn!("stale-transaction recovery sweep failed, continuing: {e:#}"),
    }

    // Bucket data migrations, before either server accepts a connection:
    // this instance is about to serve pages that read the *migrated* shape
    // (a group's member map, the user-group index), so a bucket written by
    // an older release gets brought forward first rather than rendering as
    // though its groups had no members. See `gitquarry_core::migrations`.
    //
    // A real failure here is fatal, deliberately: same reasoning as the
    // wrong-passphrase check above — starting anyway would leave the
    // operator diagnosing wrong-looking pages instead of reading one clear
    // error. A pass that merely couldn't finish every item (a concurrent
    // edit won a CAS) is not fatal: the pre-migration data still grants
    // exactly what it did before, so the server starts and the next
    // startup, or `gitquarry migrate --apply`, picks up the rest.
    for (migration, outcome) in gitquarry_core::migrations::run_pending(&backend, &cipher).await? {
        info!(
            "applied data migration {} ({}): {}",
            migration.version, migration.name, outcome.summary
        );
        if outcome.incomplete > 0 {
            warn!(
                "data migration {} left {} item(s) alone (changed concurrently) -- \
                 rerun with 'gitquarry migrate --apply' or restart",
                migration.version, outcome.incomplete
            );
        }
    }

    // Built before `ctx` (its `tracker` field isn't behind interior
    // mutability) but from the same `tracker_cfg` the internal API server
    // spawns below also consumes — `.as_ref()` here so that move stays
    // available.
    let tracker_link = tracker_cfg.as_ref().map(|t| gitquarry_core::app::TrackerLink {
        base_url: t.tracker_url.clone(),
        internal_token: t.internal_token.clone(),
        http: reqwest::Client::new(),
    });

    let ctx = Arc::new(AppContext {
        bucket,
        ssh_bind: ssh_bind.clone(),
        mirrors_root,
        config_path,
        exe_path,
        backend,
        locks: Locks::default(),
        cipher,
        mirror_max_age,
        mirror_max_size_bytes,
        oidc,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        tracker: tracker_link,
        allow_push_create,
        trust_forwarded_for,
        events: Default::default(),
        mirror_sync: Some(Arc::new(crate::mirror_sync::MirrorExternalHook::default())),
        visible_repo_uids_cache: Default::default(),
    });

    // Turns a real push into a `repo.push` audit entry — the audit log
    // otherwise only ever sees webui-driven actions (settings, releases,
    // merges, ...), never the actual git activity the index page's own
    // "recent activity" rail is supposed to be showing. `git::http`/
    // `git::ssh` already emit `RepoEvent::Push` on every successful push;
    // nothing consumed it until now (`events::EventBus`'s own doc comment —
    // webhooks Phase 1 scaffolding). The closure itself must stay
    // synchronous (`events::Consumer`'s signature), so the actual write is
    // a detached task, same "must never make the pusher wait" reasoning
    // `git::http`/`git::ssh` already apply to the emit call itself.
    ctx.events.register(Arc::new({
        let ctx = Arc::clone(&ctx);
        move |event| {
            let RepoEvent::Push { repo_uid, refs, pusher } = event else {
                return;
            };
            let ctx = Arc::clone(&ctx);
            tokio::spawn(async move {
                let Ok((config, _etag)) =
                    storage::settings::get_repo_config(ctx.backend().as_ref(), repo_uid.as_str(), &ctx.cipher).await
                else {
                    return;
                };
                let detail = refs.iter().map(|r| r.refname.as_str()).collect::<Vec<_>>().join(", ");
                storage::audit::record(
                    ctx.backend().as_ref(),
                    &ctx.cipher,
                    pusher.as_deref().unwrap_or("unknown"),
                    "repo.push",
                    &config.full_name,
                    Some(detail),
                )
                .await;
            });
        }
    }));

    // Warms `ctx.visible_repo_uids_cache` before either server accepts a
    // connection, so the very first hit on the home page (this deployment's
    // highest-traffic view) doesn't pay for a cold `list_prefix` — it reads
    // whatever's already there instead. Best-effort: a failure here just
    // means the first real request pays for it instead, same as today.
    if let Err(e) = ctx.visible_repo_uids().await {
        warn!("couldn't warm the visible-repo cache at startup, continuing: {e:#}");
    }

    // Before either server accepts a connection: an instance with no login
    // method yet keeps its admin pages closed, and this is the only place
    // the token that opens them is ever shown.
    gitquarry_webui::webui::bootstrap::print_hint(&ctx, &web_bind).await;

    // Detached, same reasoning as the periodic sweep just below: a repo
    // that was actively in use before a restart shouldn't make its next
    // request pay for a cold sync, but with many mirrors this can mean
    // real pack downloads, and startup accepting connections must never
    // wait on that.
    tokio::spawn(gc::prewarm_recent_mirrors(Arc::clone(&ctx)));

    // Detached, not part of the `select!` below: a sweep failure logs and
    // retries next tick, it must never take ssh:///web down with it.
    tokio::spawn(gc::run_periodic_sweep(Arc::clone(&ctx)));
    tokio::spawn(deleted_repo_sweep::run_periodic_sweep(Arc::clone(&ctx)));
    // `[tracker]` absent (the common case today) means no
    // `gitquarry-tracker` process will ever call in — don't bind a second
    // port nobody's using. A bind failure here is fatal (unlike the sweeps
    // above): an operator who configured `[tracker]` expecting it to work
    // should learn immediately, not have the tracker silently never come
    // up.
    if let Some(tracker) = tracker_cfg {
        let internal_ctx = Arc::clone(&ctx);
        let internal_bind = tracker.internal_bind.clone();
        tokio::spawn(async move {
            if let Err(e) =
                gitquarry_core::internal_api::run_server(&internal_bind, internal_ctx, tracker.internal_token).await
            {
                eprintln!("gitquarry: fatal: internal API server (tracker) failed: {e:#}");
                std::process::exit(1);
            }
        });
    }

    tokio::spawn(gitquarry_mirror_external::engine::run_periodic_sync(Arc::clone(&ctx)));
    // Same detached treatment, and it stays a no-op on every tick until an
    // admin configures a destination and turns it on (`backup_enabled`).
    tokio::spawn(backup::schedule::run_periodic(Arc::clone(&ctx)));
    // The two servers are infinite loops: if either returns, it's an error
    // and there's no point serving just the other. `shutdown_signal`
    // resolving is not an error: it's how `docker stop`/`podman stop` (SIGTERM)
    // and Ctrl-C (SIGINT) ask for a clean exit instead of the 10s SIGKILL
    // timeout container runtimes fall back to.
    tokio::select! {
        r = ssh::run_server(&ssh_bind, Arc::clone(&ctx)) => {
            r.context("the ssh:// server failed")
        }
        r = webui::run_server(&web_bind, ctx) => {
            r.context("the web UI failed")
        }
        () = shutdown_signal() => {
            info!("shutdown signal received, closing");
            Ok(())
        }
    }
}

/// Resolves on SIGINT (Ctrl-C) or, on unix, SIGTERM — the signal container
/// runtimes send on `stop`/`down`. Without this the process ignores SIGTERM
/// entirely and the runtime waits out its grace period before SIGKILL-ing it.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("couldn't register the SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
