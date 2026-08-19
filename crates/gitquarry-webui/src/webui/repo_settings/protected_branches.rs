//! Protected branches — reject force-push/delete, gate direct pushes to
//! owners-only (enforcement lives in `mirror::hook::can_push_protected`;
//! this only edits which branches count).

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_core::storage::remote::Manifest;
use maud::{Markup, PreEscaped, html};

/// `form.protected` is comma-separated (same convention as
/// `groups::parse_team_repos`), each name checked against the repo's
/// actual branches so a typo can't silently "protect" a branch that
/// doesn't exist.
pub(super) async fn save_protected_branches(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<FlashOutcome, AppError> {
    let (cas, branches) = match domain::repo::set_protected_branches(ctx, resolved, &form.protected, locale).await {
        Ok(v) => v,
        Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
    };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_protected_branches",
                name,
                non_empty(branches.iter().cloned().collect::<Vec<_>>().join(", ")),
            )
            .await;
            FlashOutcome::Ok("protected branches updated.")
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

/// A protected branch rejects force-push/delete and only an owner (repo
/// owner / group owner / admin) may push directly to it at all — see
/// `mirror::hook::can_push_protected` for the actual enforcement. Lists
/// every branch (not just `manifest.head`'s): unlike the default branch,
/// more than one can be protected at once (`main` + a `release/*` branch,
/// say), so this is a text field, not a `<select>`.
pub fn render_protected_branches_form(
    repo_name: &str,
    config: &RepoConfig,
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
    let stored = config.protected_branches.iter().cloned().collect::<Vec<_>>().join(", ");
    let current = retry.map_or(stored.as_str(), |f| f.protected.as_str());
    html! {
        h4 { (gitquarry_i18n::web::repo_settings::protected_branches_heading(locale)) }
        p class="muted" {
            (gitquarry_i18n::web::repo_settings::protected_branches_prose(locale, &branches.join(", ")))
        }
        form method="post" action=(repo_link(repo_name, "settings", "").to_string()) {
            (PreEscaped(csrf))
            input type="hidden" name="conf_action" value="set_protected_branches";
            input name="protected" value=(current) size="60" placeholder=(gitquarry_i18n::web::repo_settings::protected_branches_placeholder(locale));
            button type="submit" { (gitquarry_i18n::web::repo_settings::save_button(locale)) }
        }
    }
}
