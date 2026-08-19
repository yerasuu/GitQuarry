//! Auto-LFS enforcement — rejects a push at the pre-receive hook if it
//! introduces a blob over this size that isn't already an LFS pointer
//! (enforcement itself lives in `mirror::hook`, this only edits the limit).

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link, settings_section};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::RepoConfig;
use maud::{Markup, PreEscaped, html};

pub(super) async fn save_lfs_max_blob_size(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
) -> Result<FlashOutcome, AppError> {
    let value = non_empty(form.lfs_max_blob_size.clone());
    let (cas, value) = match domain::repo::set_lfs_max_blob_size(ctx, resolved, value).await {
        Ok(v) => v,
        Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
    };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_lfs_max_blob_size",
                name,
                value,
            )
            .await;
            FlashOutcome::Ok("LFS size limit updated.")
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

pub fn render_lfs_limit_form(
    repo_name: &str,
    config: &RepoConfig,
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let current = retry.map_or(config.lfs_max_blob_size.as_deref().unwrap_or(""), |f| {
        f.lfs_max_blob_size.as_str()
    });
    html! {
        (settings_section(&gitquarry_i18n::web::repo_settings::lfs_limit_heading(locale), Some(&gitquarry_i18n::web::repo_settings::lfs_limit_prose(locale)), html! {
            form method="post" action=(repo_link(repo_name, "settings", "").to_string()) {
                (PreEscaped(csrf))
                input type="hidden" name="conf_action" value="set_lfs_max_blob_size";
                input name="lfs_max_blob_size" value=(current) pattern="[0-9]+[BbKkMmGg]?" title="a number optionally followed by B/K/M/G, e.g. 50M"
                    placeholder=(gitquarry_i18n::web::repo_settings::lfs_limit_placeholder(locale));
                button type="submit" { (gitquarry_i18n::web::repo_settings::save_button(locale)) }
            }
        }))
    }
}
