//! Default branch — `Manifest::head`, not `RepoConfig` (see [`save_default_branch`]).

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, repo_link, settings_section};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::remote::Manifest;
use maud::{Markup, PreEscaped, html};

/// `manifest.head` is the actual source of truth `sync_bare_repo_from_s3`
/// sets a mirror's actual `HEAD` symref from, so `git clone`/`ls-remote`
/// over any transport see the new default too, not just this web page —
/// this doesn't read/write anything in `RepoConfig`. Only lists branches
/// (`refs/heads/*`): a tag can't be a default branch, and picking an
/// arbitrary non-branch ref as `HEAD` is how you get a detached-HEAD-like
/// mirror that confuses clients expecting a symbolic ref, not a bug worth
/// letting the form invite.
pub(super) async fn save_default_branch(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<FlashOutcome, AppError> {
    let branch = form.branch.trim();
    let cas = match domain::repo::set_default_branch(resolved, branch, locale).await {
        Ok(cas) => cas,
        Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
    };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_default_branch",
                name,
                Some(branch.to_string()),
            )
            .await;
            FlashOutcome::Ok("default branch updated.")
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

pub fn render_default_branch_form(
    repo_name: &str,
    manifest: &Manifest,
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let branches: Vec<&str> = manifest
        .refs
        .keys()
        .filter_map(|r| r.strip_prefix("refs/heads/"))
        .collect();
    if branches.is_empty() {
        return html! {};
    }
    let retry_branch = retry.map(|f| f.branch.trim());
    html! {
        (settings_section(&gitquarry_i18n::web::repo_settings::default_branch_heading(locale), None, html! {
            form method="post" action=(repo_link(repo_name, "settings", "").to_string()) data-async data-async-target="#repo-settings-body" {
                (PreEscaped(csrf))
                input type="hidden" name="conf_action" value="set_default_branch";
                select name="branch" {
                    @for branch in branches {
                        option value=(branch) selected[retry_branch.map_or_else(
                            || manifest.head.as_deref() == Some(&format!("refs/heads/{branch}")),
                            |b| b == branch,
                        )] { (branch) }
                    }
                }
                button type="submit" { (gitquarry_i18n::web::repo_settings::save_button(locale)) }
            }
        }))
    }
}
