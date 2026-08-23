//! Relocating a repo to a new namespace/name.

use super::RepoConfForm;
use crate::webui::{AppError, repo_link};
use gitquarry_core::auth::entities::User;
use gitquarry_core::domain;
use gitquarry_core::validate;
use maud::{Markup, PreEscaped, html};

/// Relocates the repo — since physical storage is keyed by `resolved.uid`,
/// not by its display name, this is a pure metadata update: claim
/// `new_name` in the repo-name index for the *same* uid, rewrite
/// `full_name`/`owner` on the *same* `RepoConfig` object, release the old
/// name claim. No git data moves, nothing gets re-encrypted — unlike a
/// fork's `copy_all_to` (a genuine duplication), a rename has no reason to
/// touch a single pack. Confirmed by typing the repo's current full name,
/// same friction as deleting it: still the same order of risk even though
/// it's now cheap, because getting the destination namespace wrong is just
/// as easy to fat-finger as before. `description`/`visibility` carry over
/// unchanged; the destination's `RepoConfig.owner` is set to
/// `new_namespace` regardless of what `owner` said before the move, so it
/// never disagrees with where the repo now lives.
/// `Ok(Err(msg))` for a rejected move (bad confirm text, unknown namespace,
/// same-repo destination, name already taken) — the caller folds that into
/// `FlashOutcome::Invalid` and re-renders the settings page instead of
/// losing it to the generic error page. `Err(AppError)` stays reserved for
/// truly exceptional failures (backend/storage errors from `?` below).
pub(super) async fn move_repo(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    locale: gitquarry_i18n::Locale,
) -> Result<Result<String, String>, AppError> {
    if form.confirm_name.trim() != name {
        return Ok(Err(gitquarry_i18n::shared::repo::type_name_to_confirm_move(
            locale, name,
        )));
    }
    let new_namespace = form.new_namespace.trim();
    let Some(new_owner) = domain::repo::resolve_owner_ref(ctx, new_namespace).await? else {
        return Ok(Err(gitquarry_i18n::shared::repo::namespace_not_found(
            locale,
            new_namespace,
        )));
    };
    let new_display_name = match validate::validate_repo_display_name(&form.new_local_name) {
        Ok(n) => n,
        Err(e) => return Ok(Err(e.to_string())),
    };
    let Some(slug) = domain::repo::slugify_repo_name(&new_display_name) else {
        return Ok(Err("name must contain at least one letter or digit.".to_string()));
    };
    let new_name = match validate::sanitize_path(&format!("{new_namespace}/{slug}")) {
        Ok(new_name) => new_name,
        Err(e) => return Ok(Err(e.to_string())),
    };
    if new_name == name {
        return Ok(Err(gitquarry_i18n::shared::repo::destination_is_same_repo(locale)));
    }

    let renamed = match domain::repo::rename(
        ctx,
        resolved,
        name,
        &new_name,
        new_owner,
        Some(new_display_name),
        locale,
    )
    .await
    {
        Ok(renamed) => renamed,
        Err(e) => return Ok(Err(e.to_string())),
    };
    Ok(renamed.ok_or_else(|| gitquarry_i18n::shared::repo::repo_already_exists_at(locale, &new_name)))
}

/// Same `<option>` list of existing usernames/groups the owner-transfer
/// field in `general::render_repo_conf_form` uses, minus the "(none)"
/// option — the move destination always has to be a real namespace, never
/// unset.
fn namespace_options(users: &[User], group_names: &[String], selected: Option<&str>) -> Markup {
    html! {
        @for username in users.iter().map(|u| u.username.as_str()) {
            option value=(username) selected[selected == Some(username)] { (username) }
        }
        @for group_name in group_names {
            option value=(group_name) selected[selected == Some(group_name.as_str())] { (group_name) }
        }
    }
}

/// Same order of risk as `danger_zone::render_delete_repo_form` (every
/// existing clone URL for the old path stops working), same confirmation-
/// by-typing-the-name friction.
pub fn render_move_repo_form(
    repo_name: &str,
    users: &[User],
    group_names: &[String],
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    let new_namespace = retry.map(|f| f.new_namespace.as_str());
    let new_local_name = retry.map_or("", |f| f.new_local_name.as_str());
    let confirm_name = retry.map_or("", |f| f.confirm_name.as_str());
    html! {
        h3 { (t::move_repo_heading(locale)) }
        p class="muted" {
            (t::move_repo_prose_intro(locale))
            (t::type_full_name_prefix(locale)) code { (repo_name) } (t::to_confirm_suffix(locale))
        }
        form method="post" action=(repo_link(repo_name, "settings", "").to_string()) {
            (PreEscaped(csrf))
            input type="hidden" name="conf_action" value="move_repo";
            div class="field" {
                label { (t::new_namespace_label(locale)) }
                select name="new_namespace" { (namespace_options(users, group_names, new_namespace)) }
            }
            div class="field" {
                label { (t::new_name_label(locale)) }
                input name="new_local_name" value=(new_local_name) required size="30" placeholder=(t::new_name_placeholder(locale));
                span class="hint" { (t::new_name_hint(locale)) }
            }
            div class="field" {
                label { (t::confirm_label(locale)) }
                input name="confirm_name" value=(confirm_name) required size="40" placeholder=(repo_name);
            }
            p { button type="submit" { (t::move_button(locale)) } }
        }
    }
}
