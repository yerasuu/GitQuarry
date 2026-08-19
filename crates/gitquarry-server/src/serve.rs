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
use gitquarry_core::git::ssh;
use gitquarry_core::mirror::{deleted_repo_sweep, gc};
use gitquarry_core::{config, storage};
use gitquarry_webui::webui;
use std::path::PathBuf;
use std::sync::Arc;

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
and http. If the config has only one bucket, --bucket is optional.

Local mirrors (`--mirrors-dir`) are a cache: a periodic sweep deletes the
ones not queried within `--mirrors-max-age` (default 3d) and, if the
directory is still above `--mirrors-max-size` (default 5G), the
least-used ones until back under the limit. They get recreated from S3
on the next query.

'gitquarry fsck [--bucket NAME] [--apply]' looks for objects that gitquarry
doesn't write inside a repo's prefix -- see that command with --help.

'gitquarry repack <repo> [--bucket NAME] [--apply]' consolidates a repo's
accumulated packs into one -- see that command with --help.";

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
    let (bucket, _bucket_cfg, backend, cipher, allow_push_create) = if config_path.is_file() {
        let conf = config::Config::load_from(&config_path)?;
        let allow_push_create = conf.allow_push_create();
        config::warn_if_world_readable(&config_path);
        // Without --bucket: if the config only has one configured, there's no
        // ambiguity.
        let bucket = match args.bucket {
            Some(b) => b,
            None => conf
                .only_bucket_name()
                .map(str::to_string)
                .context("--bucket NAME required (config has more than one bucket)")?,
        };
        // Validates early, with a good error message, before binding ports.
        let bucket_cfg = conf.bucket(&bucket)?.clone();
        let backend = storage::build_backend(&bucket_cfg);
        // Derives (or creates, if the bucket is new) this bucket's cipher.
        // Fails hard if the passphrase doesn't match: starting with the
        // wrong key would leave every operation failing to decrypt, better
        // to cut it off here with a clear message than after binding any
        // port.
        let cipher =
            gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?;
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
                allow_push_create,
            )
        } else {
            (bucket, bucket_cfg, backend, cipher, allow_push_create)
        }
    } else {
        // First run, no `gitquarry.toml` at all yet — the setup wizard
        // writes a bucket-only config, so `allow_push_create` is off too
        // until an operator opts in by hand.
        let web_bind = args.web_bind.clone().unwrap_or_else(|| DEFAULT_WEB_BIND.to_string());
        let outcome = setup::run(&config_path, &web_bind, None).await?;
        (
            outcome.bucket,
            outcome.bucket_cfg,
            outcome.backend,
            outcome.cipher,
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
            eprintln!("gitquarry: warning: couldn't read the bucket config, using defaults: {e:#}");
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
            eprintln!("gitquarry: warning: OIDC unavailable, continuing without login: {e:#}");
            None
        }
    };

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
        Ok(n) => eprintln!("gitquarry: recovered {n} stale transaction(s) left by a previous crash"),
        Err(e) => eprintln!("gitquarry: warning: stale-transaction recovery sweep failed, continuing: {e:#}"),
    }

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
        allow_push_create,
        events: Default::default(),
    });

    // Detached, not part of the `select!` below: a sweep failure logs and
    // retries next tick, it must never take ssh:///web down with it.
    tokio::spawn(gc::run_periodic_sweep(Arc::clone(&ctx)));
    tokio::spawn(deleted_repo_sweep::run_periodic_sweep(Arc::clone(&ctx)));
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
            eprintln!("gitquarry: shutdown signal received, closing");
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
