//! Protected tags — same idea as [`super::protected_branches`] but any move
//! at all is rejected once protected, not just a non-ancestor one (see
//! `mirror::hook::check_protected`'s `allow_fast_forward`).

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link, settings_section};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_core::storage::remote::Manifest;
use maud::{Markup, PreEscaped, html};

pub(super) async fn save_protected_tags(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<FlashOutcome, AppError> {
    let (cas, tags) = match domain::repo::set_protected_tags(ctx, resolved, &form.protected_tags, locale).await {
        Ok(v) => v,
        Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
    };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_protected_tags",
                name,
                non_empty(tags.iter().cloned().collect::<Vec<_>>().join(", ")),
            )
            .await;
            FlashOutcome::Ok("protected tags updated.")
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

pub(super) fn render_protected_tags_form(
    repo_name: &str,
    config: &RepoConfig,
    manifest: &Manifest,
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let tags: Vec<&str> = manifest
        .refs
        .keys()
        .filter_map(|r| r.strip_prefix("refs/tags/"))
        .collect();
    if tags.is_empty() {
        return html! {};
    }
    let stored = config.protected_tags.iter().cloned().collect::<Vec<_>>().join(", ");
    let current = retry.map_or(stored.as_str(), |f| f.protected_tags.as_str());
    html! {
        (settings_section(&gitquarry_i18n::web::repo_settings::protected_tags_heading(locale), None, html! {
            p class="muted" {
                (gitquarry_i18n::web::repo_settings::protected_tags_prose(locale, &tags.join(", ")))
            }
            form method="post" action=(repo_link(repo_name, "settings", "").to_string()) data-async data-async-target="#repo-settings-body" data-confirm=(gitquarry_i18n::web::repo_settings::save_protected_tags_confirm(locale)) {
                (PreEscaped(csrf))
                input type="hidden" name="conf_action" value="set_protected_tags";
                input name="protected_tags" value=(current) size="60" placeholder=(gitquarry_i18n::web::repo_settings::protected_tags_placeholder(locale));
                button type="submit" { (gitquarry_i18n::web::repo_settings::save_button(locale)) }
            }
        }))
    }
}
