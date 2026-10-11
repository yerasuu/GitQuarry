//! Labels — a repo's name+color issue-label vocabulary.

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, repo_link, settings_section};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::RepoConfig;
use maud::{Markup, PreEscaped, html};

/// Unlike [`super::topics::save_topics`], a malformed entry here is a hard
/// error, not a silent drop — see `domain::repo::set_labels`'s own doc
/// comment on why a broken color can't just be normalized away.
pub(super) async fn save_labels(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
) -> Result<FlashOutcome, AppError> {
    let (cas, labels) = match domain::repo::set_labels(ctx, resolved, &form.labels).await {
        Ok(v) => v,
        Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
    };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_labels",
                name,
                Some(labels.iter().map(|l| l.name.clone()).collect::<Vec<_>>().join(", ")),
            )
            .await;
            FlashOutcome::Ok("labels updated.")
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

pub(super) fn render_labels_form(
    repo_name: &str,
    config: &RepoConfig,
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let stored = config
        .labels
        .iter()
        .map(|l| format!("{}:{}", l.name, l.color))
        .collect::<Vec<_>>()
        .join(", ");
    let current = retry.map_or(stored.as_str(), |f| f.labels.as_str());
    html! {
        (settings_section(&gitquarry_i18n::web::repo_settings::labels_heading(locale), Some(&gitquarry_i18n::web::repo_settings::labels_prose(locale)), html! {
            @if !config.labels.is_empty() {
                p {
                    @for label in &config.labels {
                        (crate::webui::repo::components::label_pill(&label.name, &label.color))
                        " "
                    }
                }
            }
            form method="post" action=(repo_link(repo_name, "settings", "").to_string()) data-async data-async-target="#repo-settings-body" data-confirm=(gitquarry_i18n::web::repo_settings::save_labels_confirm(locale)) {
                (PreEscaped(csrf))
                input type="hidden" name="conf_action" value="set_labels";
                input name="labels" value=(current) size="60" placeholder=(gitquarry_i18n::web::repo_settings::labels_placeholder(locale));
                button type="submit" { (gitquarry_i18n::web::repo_settings::save_button(locale)) }
            }
        }))
    }
}
