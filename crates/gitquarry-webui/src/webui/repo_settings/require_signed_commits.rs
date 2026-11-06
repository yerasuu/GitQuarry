//! Branches that reject any commit not GPG-signed by a key registered to an
//! account with push permission — enforcement lives in
//! `mirror::hook::check_signed_commits` (see `RepoConfig::require_signed_commits`'s
//! own doc comment); this only edits which branches count.

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link, settings_section};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_core::storage::remote::Manifest;
use maud::{Markup, PreEscaped, html};

/// `form.signed_required` is comma-separated (same convention as
/// `protected_branches::save_protected_branches`), each name checked
/// against the repo's actual branches.
pub(super) async fn save_require_signed_commits(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<FlashOutcome, AppError> {
    let (cas, branches) =
        match domain::repo::set_require_signed_commits(ctx, resolved, &form.signed_required, locale).await {
            Ok(v) => v,
            Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
        };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_require_signed_commits",
                name,
                non_empty(branches.iter().cloned().collect::<Vec<_>>().join(", ")),
            )
            .await;
            FlashOutcome::Ok("signed-commit requirement updated.".to_string())
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

pub fn render_require_signed_commits_form(
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
    let stored = config
        .require_signed_commits
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let current = retry.map_or(stored.as_str(), |f| f.signed_required.as_str());
    html! {
        (settings_section(&gitquarry_i18n::web::repo_settings::require_signed_commits_heading(locale), None, html! {
            p class="muted" {
                (gitquarry_i18n::web::repo_settings::require_signed_commits_prose(locale, &branches.join(", ")))
            }
            form method="post" action=(repo_link(repo_name, "settings", "").to_string()) data-async data-async-target="#repo-settings-body" data-confirm=(gitquarry_i18n::web::repo_settings::save_require_signed_commits_confirm(locale)) {
                (PreEscaped(csrf))
                input type="hidden" name="conf_action" value="set_require_signed_commits";
                input name="signed_required" value=(current) size="60" placeholder=(gitquarry_i18n::web::repo_settings::require_signed_commits_placeholder(locale));
                button type="submit" { (gitquarry_i18n::web::repo_settings::save_button(locale)) }
            }
        }))
    }
}
