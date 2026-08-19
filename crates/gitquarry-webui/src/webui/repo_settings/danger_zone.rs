//! Deleting the repo outright — and, while it's still within its grace
//! window (`domain::repo::DELETE_GRACE_SECS`), restoring it.

use super::RepoConfForm;
use crate::webui::repo_link;
use maud::{Markup, PreEscaped, html};

/// `deleted_at` set means `domain::repo::delete` already ran and the repo
/// is sitting in its grace window — show the restore action instead of the
/// delete form (deleting an already-deleted repo isn't a request that
/// makes sense, `domain::repo::delete` rejects it anyway).
///
/// `retry_delete`/`retry_purge`: `Some(&form)` only when that specific
/// action just got rejected for a mismatched `confirm_name` — without this,
/// the failed submit re-renders the page with the modal back to its default
/// `hidden` state and the mistyped name gone, so the user has to reopen the
/// modal AND retype the name from scratch just to see what they got wrong.
pub fn render_delete_repo_form(
    repo_name: &str,
    deleted_at: Option<u64>,
    csrf: &str,
    retry_delete: Option<&RepoConfForm>,
    retry_purge: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    match deleted_at {
        Some(deleted_at) => render_restore_repo_form(repo_name, deleted_at, csrf, retry_purge, locale),
        None => render_delete_form(repo_name, csrf, retry_delete, locale),
    }
}

/// Irreversible: `handler_repo_conf_post` refuses `delete_repo` unless
/// `confirm_name` matches `repo_name` exactly, so the only way this fires
/// is typing the full name out. Not actually irreversible anymore in
/// itself (see [`render_restore_repo_form`]) — the wording still calls it
/// that because from here it looks that way, and the grace window is a
/// safety net, not a feature to advertise as an escape hatch.
///
/// Sits inside a `styler` modal (`#delete-repo-modal`) now — the form
/// itself, the CSRF field, and the name-confirmation input are all
/// unchanged from before; the modal is a second, independent layer on top
/// (can't even see the confirm field without a deliberate click first),
/// not a replacement for the first one. `data-modal-open`/`data-modal-close`
/// need no glue JS — `styler`'s `modal.js` is markup/attribute-driven and
/// never touches `form`/`submit` at all, confirmed by reading it in full.
fn render_delete_form(
    repo_name: &str,
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    let confirm_name = retry.map_or("", |f| f.confirm_name.as_str());
    html! {
        h3 { (t::delete_repo_label(locale)) }
        p class="muted" {
            (t::delete_repo_prose_intro(locale)) code { "refs.json" } ", " code { ".conf/config.json" }
            (t::delete_repo_prose_pack_note(locale))
        }
        button type="button" class="btn danger" data-modal-open="#delete-repo-modal" { (t::delete_repo_label(locale)) }
        div class="modal-backdrop" id="delete-repo-modal" hidden[retry.is_none()] {
            div class="modal" {
                div class="modal-header" {
                    div class="modal-title" { (t::delete_repo_label(locale)) }
                    span class="close" data-modal-close aria-label=(gitquarry_i18n::shared::cancel_button(locale)) { "×" }
                }
                p class="muted" {
                    (t::type_full_name_prefix(locale)) code { (repo_name) } (t::to_confirm_suffix(locale))
                }
                form method="post" action=(repo_link(repo_name, "settings", "").to_string()) {
                    (PreEscaped(csrf))
                    input type="hidden" name="conf_action" value="delete_repo";
                    input name="confirm_name" value=(confirm_name) required size="40" placeholder=(repo_name);
                    div class="modal-footer" {
                        button type="button" class="secondary" data-modal-close { (gitquarry_i18n::shared::cancel_button(locale)) }
                        button type="submit" class="danger" { (t::delete_repo_label(locale)) }
                    }
                }
            }
        }
    }
}

/// `deleted_at` is unix seconds — `domain::repo::grace_seconds_remaining`
/// turns that into how long is actually left, rounded up to whole days so
/// "less than a day left" still reads as `1`, not `0`.
///
/// Also offers "force purge now": this settings tab's GET route is already
/// admin-gated (`webui::repo`'s `require_admin`, unless no login method is
/// configured at all, in which case every action here is unrestricted
/// anyway) — same reasoning `render_delete_repo_form` relies on for showing
/// its own form unconditionally. The POST side re-checks `User::is_admin`
/// itself regardless (`require_repo_authority`), so this button existing is
/// never the only thing standing between a non-admin and an immediate purge.
fn render_restore_repo_form(
    repo_name: &str,
    deleted_at: u64,
    csrf: &str,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    let days_left = gitquarry_core::domain::repo::grace_seconds_remaining(deleted_at).div_ceil(86_400);
    let confirm_name = retry.map_or("", |f| f.confirm_name.as_str());
    html! {
        h3 { (t::restore_repo_label(locale)) }
        p class="muted" { (t::deleted_repo_notice(locale, days_left)) }
        form method="post" action=(repo_link(repo_name, "settings", "").to_string()) {
            (PreEscaped(csrf))
            input type="hidden" name="conf_action" value="restore_repo";
            button type="submit" { (t::restore_repo_label(locale)) }
        }
        h3 { (t::force_purge_label(locale)) }
        p class="muted" { (t::force_purge_prose(locale)) }
        button type="button" class="btn danger" data-modal-open="#purge-repo-modal" { (t::force_purge_label(locale)) }
        div class="modal-backdrop" id="purge-repo-modal" hidden[retry.is_none()] {
            div class="modal" {
                div class="modal-header" {
                    div class="modal-title" { (t::force_purge_label(locale)) }
                    span class="close" data-modal-close aria-label=(gitquarry_i18n::shared::cancel_button(locale)) { "×" }
                }
                p class="muted" {
                    (t::type_full_name_prefix(locale)) code { (repo_name) } (t::to_confirm_suffix(locale))
                }
                form method="post" action=(repo_link(repo_name, "settings", "").to_string()) {
                    (PreEscaped(csrf))
                    input type="hidden" name="conf_action" value="purge_repo";
                    input name="confirm_name" value=(confirm_name) required size="40" placeholder=(repo_name);
                    div class="modal-footer" {
                        button type="button" class="secondary" data-modal-close { (gitquarry_i18n::shared::cancel_button(locale)) }
                        button type="submit" class="danger" { (t::force_purge_label(locale)) }
                    }
                }
            }
        }
    }
}
