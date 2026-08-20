//! Required reviewers — usernames who must approve before
//! `domain::merge_request::merge` lets any MR land, on top of whatever a
//! `CODEOWNERS` file in the repo additionally requires for the specific
//! paths an MR touches (see that function's own doc comment).

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link, settings_section};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::RepoConfig;
use maud::{Markup, PreEscaped, html};

/// `form.required_reviewers` is comma-separated, each name checked against
/// real users (`domain::repo::set_required_reviewers`'s own doc comment on
/// why a typo here can't just be silently dropped the way a topic would).
pub(super) async fn save_required_reviewers(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<FlashOutcome, AppError> {
    let (cas, reviewers) =
        match domain::repo::set_required_reviewers(ctx, resolved, &form.required_reviewers, locale).await {
            Ok(v) => v,
            Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
        };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_required_reviewers",
                name,
                non_empty(reviewers.iter().cloned().collect::<Vec<_>>().join(", ")),
            )
            .await;
            FlashOutcome::Ok("required reviewers updated.")
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

pub fn render_required_reviewers_form(
    repo_name: &str,
    config: &RepoConfig,
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    let stored = config.required_reviewers.iter().cloned().collect::<Vec<_>>().join(", ");
    let current = retry.map_or(stored.as_str(), |f| f.required_reviewers.as_str());
    html! {
        (settings_section(&t::required_reviewers_heading(locale), Some(&t::required_reviewers_prose(locale)), html! {
            form method="post" action=(repo_link(repo_name, "settings", "").to_string()) {
                (PreEscaped(csrf))
                input type="hidden" name="conf_action" value="set_required_reviewers";
                input name="required_reviewers" value=(current) size="60" placeholder=(t::required_reviewers_placeholder(locale));
                button type="submit" { (t::save_button(locale)) }
            }
        }))
    }
}
