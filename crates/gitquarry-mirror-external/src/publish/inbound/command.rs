//! `gitquarry publish-inbound-dry-run`: the operator command around
//! [`super::dry_run`]. Here rather than in `gitquarry-domain` for the same
//! reason as `publish-gc`: only this crate reads publish mirror data.
//!
//! The private repo is read from a throwaway copy downloaded from the
//! bucket, as `repack` does, so the command needs no running server and
//! leaves no mirror behind.

use super::{DryRun, PrNumber, PublicBranch, Request, Verdict};
use crate::access::ExternalMirrorProvider;
use crate::publish::{MirrorToken, publish_mirrors_collection};
use crate::validate::{mirror_host, validate_mirror_id};
use anyhow::{Context, Result, bail};
use gitquarry_core::storage::remote::Remote;
use gitquarry_core::{config, storage, validate};
use std::fmt::Write as _;
use std::sync::Arc;

const USAGE: &str = "\
usage:
  gitquarry publish-inbound-dry-run --repo OWNER/NAME --mirror ID --pr N --base BRANCH
                                    [--provider github|gitlab|codeberg] [--push-token] [--bucket NAME]

Says whether public pull request N of a history publish mirror translates
into private commits, and if not, which rule refuses it: fetches it from the
mirror's public remote into a temporary quarantine repo, resolves its base
through the epoch's inverse index, overlays each commit on the private tree,
runs every inbound gate and checks that publishing the result gives back the
pull request's own commits. Writes nothing to the bucket or the private repo.

  --repo OWNER/NAME  the private repo
  --mirror ID        the publish mirror (16 lowercase hex digits)
  --pr N             the public pull request number
  --base BRANCH      the public branch the pull request is opened against;
                     must be one the mirror publishes
  --provider NAME    where the forge keeps pull request heads; inferred for
                     github.com, gitlab.com and codeberg.org
  --push-token       read with the mirror's push token when it has no
                     inbound token (a broader credential than reading needs)";

#[derive(Debug)]
pub struct Args {
    pub bucket: Option<String>,
    /// Already sanitized.
    pub repo: String,
    /// Already validated: it becomes part of bucket keys.
    pub mirror: String,
    pub pr: PrNumber,
    pub base: PublicBranch,
    pub provider: Option<ExternalMirrorProvider>,
    pub push_token: bool,
}

/// `Ok(None)` when `--help` was asked for and already printed.
pub fn parse_args(rest: &[String]) -> Result<Option<Args>> {
    let (mut bucket, mut repo, mut mirror, mut pr, mut base, mut provider) = (None, None, None, None, None, None);
    let mut push_token = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--bucket" => bucket = Some(value()?.clone()),
            "--repo" => repo = Some(validate::sanitize_path(value()?)?),
            "--mirror" => {
                let id = value()?;
                validate_mirror_id(id)?;
                mirror = Some(id.clone());
            }
            "--pr" => pr = Some(value()?.parse::<PrNumber>()?),
            "--base" => base = Some(value()?.parse::<PublicBranch>()?),
            "--provider" => provider = Some(parse_provider(value()?)?),
            "--push-token" => push_token = true,
            "--help" | "-h" => {
                println!("{USAGE}"); // stdout, not tracing, because --help output is the CLI's answer
                return Ok(None);
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    let missing = |name: &str| format!("{name} is required\n\n{USAGE}");
    Ok(Some(Args {
        bucket,
        repo: repo.with_context(|| missing("--repo"))?,
        mirror: mirror.with_context(|| missing("--mirror"))?,
        pr: pr.with_context(|| missing("--pr"))?,
        base: base.with_context(|| missing("--base"))?,
        provider,
        push_token,
    }))
}

fn parse_provider(raw: &str) -> Result<ExternalMirrorProvider> {
    Ok(match raw {
        "github" => ExternalMirrorProvider::GitHub,
        "gitlab" => ExternalMirrorProvider::GitLab,
        "codeberg" => ExternalMirrorProvider::Codeberg,
        other => bail!("unknown provider {other:?} (github, gitlab or codeberg)"),
    })
}

/// The provider of a mirror's public remote, for the three public forges
/// whose host says it.
pub fn provider_for(remote_url: &str) -> ExternalMirrorProvider {
    match mirror_host(remote_url).as_deref() {
        Some("github.com") => ExternalMirrorProvider::GitHub,
        Some("gitlab.com") => ExternalMirrorProvider::GitLab,
        Some("codeberg.org") => ExternalMirrorProvider::Codeberg,
        _ => ExternalMirrorProvider::Other,
    }
}

/// The credential that reads the public side (§4): the inbound token, and
/// the push token only when the operator asked for it. Never anonymous.
fn credential<'a>(inbound: &'a MirrorToken, push: &'a MirrorToken, allow_push: bool) -> Result<&'a MirrorToken> {
    if !inbound.is_empty() {
        return Ok(inbound);
    }
    if allow_push && !push.is_empty() {
        return Ok(push);
    }
    bail!("the mirror has no inbound token; pass --push-token to read with its push token")
}

pub async fn run(rest: &[String]) -> Result<()> {
    let Some(args) = parse_args(rest)? else {
        return Ok(());
    };
    let config_path = config::Config::resolve_path()?;
    let conf =
        config::Config::load_from(&config_path).with_context(|| format!("couldn't read {}", config_path.display()))?;
    let bucket = match args.bucket {
        Some(b) => b,
        None => conf
            .default_bucket_name()
            .map(str::to_string)
            .context("--bucket NAME required (config has more than one bucket and no default_bucket)")?,
    };
    let bucket_cfg = conf.bucket(&bucket)?.clone();
    let backend = storage::build_backend(&bucket_cfg)?.backend;
    let cipher = Arc::new(
        gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?,
    );
    let uid = storage::settings::resolve_repo_name(backend.as_ref(), &cipher, &args.repo)
        .await?
        .with_context(|| format!("repo '{}' doesn't exist in bucket '{bucket}'", args.repo))?;
    let (mirror, _etag) = publish_mirrors_collection(&uid)
        .get::<crate::publish::PublishMirror>(backend.as_ref(), &cipher, &args.mirror)
        .await?;
    let mirror = mirror.with_context(|| format!("repo '{}' has no publish mirror {}", args.repo, args.mirror))?;
    let token = credential(&mirror.inbound_token, &mirror.token, args.push_token)?;
    let request = Request {
        pr: args.pr,
        base: args.base,
        provider: args.provider.unwrap_or_else(|| provider_for(&mirror.remote_url)),
    };

    let private = gitquarry_core::git::scratch_dir(&std::env::temp_dir())?;
    gitquarry_core::git::init_bare(private.path()).await?;
    let remote = Remote::new(
        Arc::clone(&backend),
        format!("{}/{uid}", storage::REPOS_PREFIX),
        Arc::clone(&cipher),
    );
    gitquarry_core::mirror::sync_bare_repo_from_s3(&remote, private.path()).await?;
    let dry_run = super::dry_run(
        backend.as_ref(),
        &cipher,
        &uid,
        private.path(),
        &mirror,
        &request,
        token,
    )
    .await?;
    print!("{}", render(&dry_run)); // stdout, not tracing, because the verdict is the CLI's answer
    Ok(())
}

/// The verdict as the command prints it: one `key: value` per line, then
/// the translated commits, so a script can read it as easily as a person.
pub fn render(dry_run: &DryRun) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "pr: {}", dry_run.pr);
    let _ = writeln!(out, "epoch: {}", dry_run.epoch);
    let _ = writeln!(out, "head: {}", dry_run.head.as_deref().unwrap_or("-"));
    let _ = writeln!(out, "base: {}", dry_run.base.as_deref().unwrap_or("-"));
    match &dry_run.verdict {
        Verdict::Translatable(translation) => {
            let _ = writeln!(out, "verdict: translatable");
            let _ = writeln!(out, "private base: {}", translation.private_base);
            let _ = writeln!(out, "collapsed onto base: {}", translation.collapsed_onto_base);
            for commit in &translation.commits {
                let _ = writeln!(out, "  {} -> {}", commit.public, commit.private);
            }
        }
        Verdict::Refused(refusal) => {
            let _ = writeln!(out, "verdict: refused");
            let _ = writeln!(out, "reason: {:?}: {}", refusal.reason, refusal.reason);
            if let Some(commit) = &refusal.public_commit {
                let _ = writeln!(out, "commit: {commit}");
            }
            if let Some(path) = &refusal.path {
                let _ = writeln!(out, "path: {path:?}");
            }
        }
    }
    out
}
