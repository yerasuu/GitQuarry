//! Per-user ssh:// access grid for one repo — a Pijul-Nest-style grid,
//! adapted to what git actually has: no discussions/channels/tags, no
//! "everybody" row (ssh:// always requires an authorized pubkey), just the
//! five [`Permission`] tiers per user.

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, repo_link};
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use maud::{Markup, PreEscaped, html};

/// Sets (or clears) one user's explicit `Permission` for `name`, leaving
/// every other repo in their `repos` map and every other user untouched —
/// unlike `/_users`' `set_repos`, which replaces a user's whole map from a
/// comma-separated field, this only ever touches the one (user, repo) cell
/// the submitted row represents.
pub(super) async fn save_perm(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<FlashOutcome, AppError> {
    let perm = match form.perm.as_str() {
        "guest" => Some(Permission::Guest),
        "reporter" => Some(Permission::Reporter),
        "developer" => Some(Permission::Developer),
        "maintainer" => Some(Permission::Maintainer),
        "owner" => Some(Permission::Owner),
        "none" => None,
        other => {
            return Ok(FlashOutcome::Invalid(gitquarry_i18n::shared::auth::invalid_permission(
                locale, other,
            )));
        }
    };
    let cas = match domain::repo::set_permission(ctx, &resolved.uid, &form.username, perm, locale).await {
        Ok(cas) => cas,
        Err(e) => return Ok(FlashOutcome::Invalid(e.to_string())),
    };
    Ok(match cas {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_perm",
                name,
                Some(format!("{}: {}", form.username, form.perm)),
            )
            .await;
            FlashOutcome::Ok(gitquarry_i18n::web::repo_settings::permission_saved(locale))
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

/// Same tier names as the radio labels in [`render_perm_grid`], just as
/// plain text for the pill badge next to each granted user's name.
fn permission_tier_label(perm: Permission, locale: gitquarry_i18n::Locale) -> String {
    use gitquarry_i18n::web::repo_settings as t;
    match perm {
        Permission::Guest => t::radio_guest_label(locale),
        Permission::Reporter => t::radio_reporter_label(locale),
        Permission::Developer => t::radio_developer_label(locale),
        Permission::Maintainer => t::radio_maintainer_label(locale),
        Permission::Owner => t::radio_owner_label(locale),
    }
}

/// Per-user ssh:// access for this one repo — independent of `visibility`
/// (which only gates git:// and the web UI, both identity-less transports).
/// Each row is its own tiny form (same pattern Pijul Nest's permission grid
/// uses): picking a radio and hitting "save" submits just that (user, repo)
/// cell, nothing else. `granted`: every user with an explicit `Permission`
/// on this repo — from `access::list_repo_permissions` (an index scoped to
/// this one repo), not every user in the bucket like the owner-transfer/
/// move-repo dropdowns on the same page still need. A user not in `granted`
/// can still get access via the "add" form below by typing their username
/// directly: this grid only ever needs to *display* the (typically small)
/// set of people who already have something, not offer a row per bucket
/// user to toggle.
pub fn render_perm_grid(
    repo_name: &str,
    granted: &[(User, Permission)],
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    html! {
        h3 { (t::perm_grid_heading(locale)) }
        p class="muted" {
            (t::perm_grid_prose_1(locale)) code { "*" } (t::perm_grid_prose_2(locale))
            a href="/_users" { "/_users" } (t::perm_grid_prose_3(locale))
            code { "visibility" } (t::perm_grid_prose_4(locale))
        }
        (render_perm_grid_fragment(repo_name, granted, csrf, locale))
    }
}

/// The grid + add-access form alone, wrapped in a stable `id` so `set_perm`
/// can swap just this fragment (`data-async-target="#perm-grid"`, see
/// `crate::webui::ASYNC_HEADER`'s own doc comment) instead of re-rendering
/// the whole settings page.
pub(super) fn render_perm_grid_fragment(
    repo_name: &str,
    granted: &[(User, Permission)],
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    html! {
        div id="perm-grid" {
        @if granted.is_empty() {
            p { em { (t::no_explicit_permissions_hint(locale)) } }
        } @else {
            div class="scroll-x" {
            table class="list" {
                tr { th { (t::table_header_user(locale)) } th { (t::table_header_access(locale)) } }
                @for (user, current) in granted {
                    tr {
                        td {
                            (crate::webui::identicon::account_avatar(&user.username, user.avatar_content_type.as_deref()))
                            (user.username)
                            " " span class=(crate::webui::permission_badge_class(*current)) { (permission_tier_label(*current, locale)) }
                            @if user.is_admin {
                                // Same "admin" badge text already cataloged for
                                // `auth::web::users`'s own permission table — reused
                                // here rather than duplicated verbatim.
                                " " span class="badge accent" { (gitquarry_i18n::web::auth::users::admin_label(locale)) }
                            }
                        }
                        td {
                            form method="post" action=(repo_link(repo_name, "settings", "").to_string()) class="inline-form" data-async data-async-target="#perm-grid" data-confirm=(t::change_perm_confirm(locale, &user.username)) {
                                (PreEscaped(csrf))
                                input type="hidden" name="conf_action" value="set_perm";
                                input type="hidden" name="username" value=(user.username);
                                label {
                                    // Never pre-checked: `granted` only ever
                                    // lists rows with a real current
                                    // permission, so "none" is always the
                                    // change, never the starting state.
                                    input type="radio" name="perm" value="none";
                                    " " (t::radio_none_label(locale))
                                }
                                label {
                                    input type="radio" name="perm" value="guest" checked[*current == Permission::Guest];
                                    " " (t::radio_guest_label(locale))
                                }
                                label {
                                    input type="radio" name="perm" value="reporter" checked[*current == Permission::Reporter];
                                    " " (t::radio_reporter_label(locale))
                                }
                                label {
                                    input type="radio" name="perm" value="developer" checked[*current == Permission::Developer];
                                    " " (t::radio_developer_label(locale))
                                }
                                label {
                                    input type="radio" name="perm" value="maintainer" checked[*current == Permission::Maintainer];
                                    " " (t::radio_maintainer_label(locale))
                                }
                                label {
                                    input type="radio" name="perm" value="owner" checked[*current == Permission::Owner];
                                    " " (t::radio_owner_label(locale))
                                }
                                button type="submit" { (t::save_button(locale)) }
                            }
                        }
                    }
                }
            }
            }
        }
        h4 { (t::add_access_heading(locale)) }
        form method="post" action=(repo_link(repo_name, "settings", "").to_string()) data-async data-async-target="#perm-grid" {
            (PreEscaped(csrf))
            input type="hidden" name="conf_action" value="set_perm";
            input type="text" name="username" required placeholder=(t::username_placeholder(locale));
            label { input type="radio" name="perm" value="guest"; " " (t::radio_guest_label(locale)) }
            label { input type="radio" name="perm" value="reporter" checked[true]; " " (t::radio_reporter_label(locale)) }
            label { input type="radio" name="perm" value="developer"; " " (t::radio_developer_label(locale)) }
            label { input type="radio" name="perm" value="maintainer"; " " (t::radio_maintainer_label(locale)) }
            label { input type="radio" name="perm" value="owner"; " " (t::radio_owner_label(locale)) }
            button type="submit" { (t::add_button(locale)) }
        }
        }
    }
}
