//! Topics — cosmetic discovery/categorization labels.

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link, settings_section};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::RepoConfig;
use maud::{Markup, PreEscaped, html};

/// Topics are cosmetic, so unlike `protected_branches`/`protected_tags`
/// there's nothing to validate against — instead each one gets normalized
/// to lowercase `[a-z0-9-]`, capped at 32 chars, so it's always safe to
/// drop straight into a `?q=` link on the index page without extra
/// escaping there. A token that normalizes to nothing (e.g. all
/// punctuation) is just dropped, not an error — this is a convenience
/// field, not a form worth rejecting typos on.
pub(super) async fn save_topics(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
) -> Result<FlashOutcome, AppError> {
    let (cas, topics) = match domain::repo::set_topics(ctx, resolved, &form.topics).await {
        Ok(v) => v,
        Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
    };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_topics",
                name,
                non_empty(topics.iter().cloned().collect::<Vec<_>>().join(", ")),
            )
            .await;
            FlashOutcome::Ok("topics updated.")
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

pub fn render_topics_form(
    repo_name: &str,
    config: &RepoConfig,
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let stored = config.topics.iter().cloned().collect::<Vec<_>>().join(", ");
    let current = retry.map_or(stored.as_str(), |f| f.topics.as_str());
    html! {
        (settings_section(&gitquarry_i18n::web::repo_settings::topics_heading(locale), Some(&gitquarry_i18n::web::repo_settings::topics_prose(locale)), html! {
            form method="post" action=(repo_link(repo_name, "settings", "").to_string()) {
                (PreEscaped(csrf))
                input type="hidden" name="conf_action" value="set_topics";
                input name="topics" value=(current) size="60" placeholder=(gitquarry_i18n::web::repo_settings::topics_placeholder(locale));
                button type="submit" { (gitquarry_i18n::web::repo_settings::save_button(locale)) }
            }
        }))
    }
}
