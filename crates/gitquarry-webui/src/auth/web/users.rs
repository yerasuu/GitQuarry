//! `/_users`: the user/role admin page — SSH pubkeys, per-repo [`Permission`]
//! tiers, web password, and the `is_admin` flag, all backed by
//! `.conf/access.json` (see [`gitquarry_core::auth::access`]). Split out of
//! [`crate::settings::web`] because it's a distinct concern from server
//! settings and per-repo config even though all three are ".conf/*.json-backed
//! admin forms" — [`crate::settings::web::require_admin`] is the shared gate
//! all three still go through.

use super::login;
use crate::settings::web::require_admin;
use crate::webui::{
    AppError, Flash, State_, admin_header, csrf_field, effective_density, effective_locale, effective_theme, layout,
    non_empty, render_flash, require_csrf,
};
use anyhow::Result;
use axum::extract::{Form, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::uid::Uid;
use maud::{Markup, PreEscaped, html};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tokio::task::JoinSet;

pub(crate) async fn handler_users_get(
    State(ctx): State_,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_users").await {
        return Ok(resp);
    }
    let mut users = access::list_all_users(&ctx.backend(), &ctx.cipher).await?;
    let q = params.get("q").cloned().unwrap_or_default();
    let q_lower = q.to_ascii_lowercase();
    users.retain(|u| q.is_empty() || u.username.to_ascii_lowercase().contains(&q_lower));
    let repo_names = repo_uid_to_name_map(&ctx, &users).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let body = render_users_page(&users, &repo_names, &csrf_field(&headers), None, &q, locale).into_string();
    Ok(axum::response::Html(layout(
        "users",
        &admin_header("users", locale),
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Every distinct repo uid referenced across `users`' `repos` maps,
/// resolved to its current `full_name` — for rendering `User::repos`
/// (uid-keyed) as clickable/typeable names instead of raw hex. A repo uid
/// that no longer resolves (deleted) is just absent from the map;
/// `render_user` falls back to "(deleted repo)" for those.
async fn repo_uid_to_name_map(ctx: &gitquarry_core::app::AppContext, users: &[User]) -> HashMap<Uid, String> {
    let uids: BTreeSet<Uid> = users.iter().flat_map(|u| u.repos.keys().cloned()).collect();
    let mut set = JoinSet::new();
    for uid in uids {
        let backend = ctx.backend();
        let cipher = Arc::clone(&ctx.cipher);
        set.spawn(async move {
            let name = gitquarry_core::storage::settings::get_repo_config(backend.as_ref(), uid.as_str(), &cipher)
                .await
                .ok()
                .map(|(config, _etag)| config.full_name);
            (uid, name)
        });
    }
    let mut map = HashMap::new();
    while let Some(result) = set.join_next().await {
        if let Ok((uid, Some(name))) = result {
            map.insert(uid, name);
        }
    }
    map
}

#[derive(serde::Deserialize)]
pub struct UsersForm {
    /// Wire name `user_action`, not `action`: most forms on this page are
    /// `data-async` -- `HTMLFormElement` is `[OverrideBuiltins]`, so a
    /// control literally named `action` shadows the form's own `.action`
    /// IDL property, and `style/app.js`'s `fetchFragment(form.action, ...)`
    /// got the `<input>` element instead of the URL, 404-ing. Every form on
    /// this page shares this one struct/field, so the rename applies even
    /// to the few plain ones too. Rust field name unchanged -- this only
    /// affects the wire (`Form<UsersForm>`) deserialization, not the
    /// struct-literal construction the test suite already uses.
    #[serde(rename = "user_action")]
    pub action: String,
    #[serde(default)]
    pub csrf: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub pubkey: String,
    /// Bulk `"repo1:write,repo2:read"` field — only `add_user` reads this
    /// (a convenience for seeding perms at creation time). Managing an
    /// existing user's repos afterward goes one entry at a time via
    /// `add_repo_perm`/`remove_repo_perm` instead, same list+add+remove
    /// pattern the pubkeys table already uses.
    #[serde(default)]
    pub repos: String,
    #[serde(default)]
    pub fingerprint: String,
    /// Only read by `add_repo_perm`/`remove_repo_perm`.
    #[serde(default)]
    pub repo: String,
    /// One of the five `Permission` tier names — only read by `add_repo_perm`.
    #[serde(default)]
    pub perm: String,
    /// Plaintext, only ever used to compute a new hash. Empty on `add_user`
    /// means "no password" (SSH/OIDC-only user); empty on `set_password`
    /// means "clear the password" — unlike the ambiguous shared-form problem
    /// `/_settings` has, each user's password form is its own dedicated
    /// submit, so an empty field here is always an explicit choice, not a
    /// side effect of saving something unrelated.
    #[serde(default)]
    pub password: String,
    /// Checkbox (`"1"` when checked, absent/empty otherwise) — only read by
    /// `add_user`; toggling an existing user's admin flag is its own action
    /// (`toggle_admin`) so it can't be flipped by accident on an unrelated
    /// edit either.
    #[serde(default)]
    pub is_admin: String,
    /// `mirror::gc::parse_size` syntax (e.g. `"5G"`) — only read by
    /// `set_storage_quota`. Empty clears the limit (unlimited).
    #[serde(default)]
    pub storage_quota: String,
}

pub(crate) async fn handler_users_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<UsersForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_users").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let actor = login::current_user(&ctx, &headers)
        .await
        .map(|u| u.username)
        .unwrap_or_default();
    let locale = effective_locale(&ctx, &headers).await;
    let audit_detail = match form.action.as_str() {
        "add_repo_perm" | "remove_repo_perm" => Some(form.repo.clone()),
        _ => None,
    };

    let action_result = apply_users_action(&ctx, &form, locale).await?;
    let is_async = headers.contains_key(crate::webui::ASYNC_HEADER);

    // Async is different from the plain fallback below: neither target a
    // form on this page can hit (`#user-card-{username}`, `#users-list`)
    // contains a flash banner (`render_users_page`'s own shell has it,
    // above the list) — a page-level fragment swapped into either would
    // corrupt the DOM (`style/app.js`'s `target.replaceWith` taking the
    // whole `<html>` as the replacement). A rejected action goes through
    // `AppError` instead on this path: a real non-2xx status, picked up by
    // `extractFlashMessage`/`showErrorToast` the same way any other failed
    // `data-async` request already is — same reasoning
    // `keys::handler_keys_post`'s own doc comment gives for its `fail!`.
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let invalid_msg;
    let flash = match &action_result {
        Ok(Cas::Ok) => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &actor,
                &format!("user.{}", form.action),
                &form.username,
                audit_detail,
            )
            .await;
            Flash::Ok("saved to the bucket.")
        }
        Ok(Cas::Conflict) => {
            if is_async {
                return Err(AppError::bad_request(anyhow::anyhow!("{}", cas_conflict_msg)));
            }
            Flash::Err(&cas_conflict_msg)
        }
        Err(msg) => {
            if is_async {
                return Err(AppError::bad_request(anyhow::anyhow!("{msg}")));
            }
            invalid_msg = msg.clone();
            Flash::Err(&invalid_msg)
        }
    };
    // `action_result` is `Ok` past this point (both error arms above
    // already returned). Two targets, by action: everything whose whole
    // effect stays contained inside one user's own card (`is_admin`
    // badge, pubkey/repo-perm tables, email, password, quota, TOTP button)
    // swaps just that card; `add_user`/`delete_user` change *which* cards
    // exist at all, so those swap the whole `#users-list`.
    if is_async
        && matches!(
            form.action.as_str(),
            "toggle_admin"
                | "remove_pubkey"
                | "add_pubkey"
                | "remove_repo_perm"
                | "add_repo_perm"
                | "disable_totp"
                | "set_email"
                | "set_password"
                | "set_storage_quota"
        )
    {
        let (Some(user), _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &form.username).await? else {
            return Err(AppError::not_found(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::account_no_longer_exists(locale)
            )));
        };
        let repo_names = repo_uid_to_name_map(&ctx, std::slice::from_ref(&user)).await;
        return Ok(
            axum::response::Html(render_user(&user, &repo_names, &csrf_field(&headers), locale).into_string())
                .into_response(),
        );
    }
    let users = access::list_all_users(&ctx.backend(), &ctx.cipher).await?;
    let repo_names = repo_uid_to_name_map(&ctx, &users).await;
    if is_async && matches!(form.action.as_str(), "add_user" | "delete_user") {
        return Ok(axum::response::Html(
            render_users_list(&users, &repo_names, &csrf_field(&headers), locale).into_string(),
        )
        .into_response());
    }
    let topnav = login::topnav(&ctx, &headers).await;
    let body = render_users_page(&users, &repo_names, &csrf_field(&headers), Some(flash), "", locale).into_string();
    Ok(axum::response::Html(layout(
        "users",
        &admin_header("users", locale),
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// One user's worth of read-modify-write, per action — replaces the old
/// single whole-blob CAS: every branch here touches only the one user
/// object the action names (plus, for `add_user`/`delete_user`/pubkey
/// changes, the namespace/fingerprint index entries that go with it), never
/// the rest of the bucket's users.
/// `Ok(Err(msg))` for a rejected action (bad name, unknown action, bad quota
/// string, missing repo, invalid permission tier, ...) — the caller folds
/// that into `Flash::Err` and re-renders the users page instead of losing
/// it to the generic error page. `Err(AppError)` stays reserved for truly
/// exceptional errors (backend/storage via bare `?`) — this whole route is
/// already admin-gated by the caller, so there's no separate authorization
/// class to keep apart here the way `groups::apply_groups_action` does.
pub async fn apply_users_action(
    ctx: &gitquarry_core::app::AppContext,
    form: &UsersForm,
    locale: gitquarry_i18n::Locale,
) -> Result<Result<Cas, String>, AppError> {
    Ok(match form.action.as_str() {
        "add_user" => {
            let username = form.username.trim();
            let pubkey = non_empty(form.pubkey.trim().to_string());
            let password = non_empty(form.password.clone());
            let cas = match domain::user::create(
                ctx,
                domain::user::NewUser {
                    username,
                    email: non_empty(form.email.clone()),
                    pubkey: pubkey.as_deref(),
                    password_plain: password.as_deref(),
                    is_admin: form.is_admin == "1",
                    repos_field: &form.repos,
                    locale,
                },
            )
            .await
            {
                Ok(cas) => cas,
                Err(e) => return Ok(Err(e.to_string())),
            };
            cas.ok_or_else(|| gitquarry_i18n::shared::name_already_taken(locale, username))
        }
        "delete_user" => {
            domain::user::delete(ctx, &form.username).await?;
            Ok(Cas::Ok)
        }
        "set_password" => {
            let password = non_empty(form.password.clone());
            domain::user::set_password(ctx, &form.username, password.as_deref(), locale)
                .await
                .map_err(|e| e.to_string())
        }
        "set_email" => domain::user::set_email(ctx, &form.username, non_empty(form.email.clone()).as_deref(), locale)
            .await
            .map_err(|e| e.to_string()),
        "set_storage_quota" => {
            let raw = form.storage_quota.trim();
            let quota_bytes = if raw.is_empty() {
                None
            } else {
                match gitquarry_core::mirror::gc::parse_size(raw) {
                    Ok(bytes) => Some(bytes),
                    Err(e) => return Ok(Err(e.to_string())),
                }
            };
            domain::user::set_storage_quota(ctx, &form.username, quota_bytes, locale)
                .await
                .map_err(|e| e.to_string())
        }
        "toggle_admin" => domain::user::toggle_admin(ctx, &form.username, locale)
            .await
            .map_err(|e| e.to_string()),
        // No re-auth check here (unlike the self-service action in
        // `auth::web::security`) — this admin action IS the account-recovery path
        // for a user who lost their TOTP device: this app has no other one
        // (no email-based reset either).
        "disable_totp" => domain::user::disable_totp(ctx, &form.username, locale)
            .await
            .map_err(|e| e.to_string()),
        "add_pubkey" => domain::user::add_pubkey(ctx, &form.username, form.pubkey.trim(), locale)
            .await
            .map_err(|e| e.to_string()),
        "remove_pubkey" => domain::user::remove_pubkey(ctx, &form.username, &form.fingerprint, locale)
            .await
            .map_err(|e| e.to_string()),
        "add_repo_perm" => {
            let repo = form.repo.trim();
            if repo.is_empty() {
                return Ok(Err(gitquarry_i18n::shared::missing_repo(locale)));
            }
            let perm = match form.perm.as_str() {
                "guest" => Permission::Guest,
                "reporter" => Permission::Reporter,
                "developer" => Permission::Developer,
                "maintainer" => Permission::Maintainer,
                "owner" => Permission::Owner,
                other => return Ok(Err(gitquarry_i18n::shared::invalid_permission(locale, other))),
            };
            domain::user::set_repo_perm(ctx, &form.username, repo, Some(perm), locale)
                .await
                .map_err(|e| e.to_string())
        }
        "remove_repo_perm" => domain::user::set_repo_perm(ctx, &form.username, form.repo.trim(), None, locale)
            .await
            .map_err(|e| e.to_string()),
        other => Err(gitquarry_i18n::shared::unknown_action(locale, other)),
    })
}

pub fn render_users_page(
    users: &[User],
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    message: Option<Flash>,
    q: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use crate::auth::i18n::users as t;
    html! {
        p class="muted" {
            (t::intro_stored_prefix(locale)) code { ".conf/users/<username>.json" } (t::intro_bucket_and_keys(locale))
            @for (i, p) in PERMISSION_TIERS.iter().enumerate() {
                @if i > 0 { "/" }
                code { (permission_label(*p, locale)) }
            }
            ", " code { "*" } (t::intro_perms_default(locale))
            code { "/_login" } (t::intro_password_login(locale)) code { "admin" } (t::intro_admin_grants(locale))
            code { "/_settings" } (t::intro_settings_page(locale)) code { "private" } (t::intro_private_repo(locale))
            code { "ssh://" } (t::intro_ssh_no_password(locale))
            code { "git clone ssh://whoever@host:PORT/repo" } (t::intro_clone_resolved(locale))
        }
        (PreEscaped(render_flash(message, locale)))
        form method="get" action="/_users" {
            input name="q" value=(q) placeholder=(t::users_search_placeholder(locale));
            button type="submit" { (t::users_search_button(locale)) }
        }

        (render_users_list(users, repo_names, csrf, locale))

        h4 { (t::add_user_heading(locale)) }
        form method="post" action="/_users" data-async data-async-target="#users-list" {
            (PreEscaped(csrf))
            input type="hidden" name="user_action" value="add_user";
            div class="field" {
                label { (t::username_label(locale)) }
                input name="username" size="30";
            }
            div class="field" {
                label { (t::email_label(locale)) }
                input name="email" size="30" placeholder=(t::set_email_placeholder(locale));
            }
            div class="field" {
                label { (t::password_label(locale)) }
                input type="password" name="password" size="30" placeholder=(t::password_placeholder(locale));
            }
            div class="field" {
                label { (t::admin_label(locale)) }
                label {
                    input type="checkbox" name="is_admin" value="1";
                    (t::admin_full_access_checkbox(locale))
                }
            }
            div class="field" {
                label { (t::pubkey_label(locale)) }
                textarea name="pubkey" rows="2" cols="60" placeholder=(t::pubkey_optional_placeholder(locale)) {}
            }
            div class="field" {
                label { (t::repos_label(locale)) }
                input name="repos" size="60" placeholder="projectA:write,*:read";
            }
            p { button type="submit" { (t::create_button(locale)) } }
        }
    }
}

/// Every user card, wrapped in a stable `id` so `add_user`/`delete_user`
/// (which change *which* cards exist) can swap just this fragment
/// (`data-async-target="#users-list"`, see `crate::webui::ASYNC_HEADER`'s
/// own doc comment) instead of re-rendering the whole page -- unlike every
/// other action here, which targets the one card it stays contained in.
fn render_users_list(
    users: &[User],
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use crate::auth::i18n::users as t;
    html! {
        div id="users-list" {
            @if users.is_empty() {
                div class="empty-state" { div class="empty-state-title" { (t::users_empty_hint(locale)) } }
            }
            @for user in users {
                (render_user(user, repo_names, csrf, locale))
            }
        }
    }
}

pub fn render_user(
    user: &User,
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use crate::auth::i18n::users as t;
    html! {
        @let card_target = format!("#user-card-{}", user.username);
        div class="card card-pad user-card" id=(format!("user-card-{}", user.username)) {
            h4 {
                (crate::webui::identicon::account_avatar(&user.username, user.avatar_content_type.as_deref()))
                (user.username)
                @if let Some(email) = &user.email {
                    " " span class="muted" { (format!("<{email}>")) }
                }
                @if user.is_admin {
                    " " span class="badge accent" { (t::admin_label(locale)) }
                }
            }

            table class="list" {
                tr { th { (t::pubkey_label(locale)) } th {} }
                @if user.pubkeys.is_empty() {
                    tr { td colspan="2" { em { (t::no_keys_hint(locale)) } } }
                }
                @for pubkey in &user.pubkeys {
                    @let fp = access::key_fingerprint(pubkey).unwrap_or_else(|_| "?".to_string());
                    tr {
                        td { code { (fp) } }
                        td {
                            form method="post" action="/_users" style="display:inline" data-async data-async-target=(card_target) {
                                (PreEscaped(csrf))
                                input type="hidden" name="user_action" value="remove_pubkey";
                                input type="hidden" name="username" value=(user.username);
                                input type="hidden" name="fingerprint" value=(fp);
                                button type="submit" { (t::delete_button(locale)) }
                            }
                        }
                    }
                }
            }

            table class="list" {
                tr { th { (t::repo_label(locale)) } th { (t::permission_label(locale)) } th {} }
                @if user.repos.is_empty() && user.default_permission.is_none() {
                    tr { td colspan="3" { em { (t::no_repos_hint_before_star(locale)) code { "*" } (t::no_repos_hint_after_star(locale)) } } }
                }
                @if let Some(perm) = user.default_permission {
                    (repo_perm_row(user, "*", "*", perm, csrf, &card_target, locale))
                }
                @for (repo_uid, perm) in &user.repos {
                    @match repo_names.get(repo_uid) {
                        Some(name) => (repo_perm_row(user, name, name, *perm, csrf, &card_target, locale)),
                        None => tr { td colspan="3" { em { (gitquarry_i18n::shared::repo_deleted(locale)) } } },
                    }
                }
            }

            form method="post" action="/_users" data-async data-async-target=(card_target) {
                (PreEscaped(csrf))
                input type="hidden" name="user_action" value="add_pubkey";
                input type="hidden" name="username" value=(user.username);
                input name="pubkey" size="60" placeholder=(t::pubkey_key_placeholder(locale));
                button type="submit" { (t::add_key_button(locale)) }
            }
            form method="post" action="/_users" data-async data-async-target=(card_target) {
                (PreEscaped(csrf))
                input type="hidden" name="user_action" value="add_repo_perm";
                input type="hidden" name="username" value=(user.username);
                input name="repo" size="30" placeholder=(t::repo_field_placeholder(locale));
                select name="perm" {
                    option value="guest" { (t::permission_guest_label(locale)) }
                    option value="reporter" { (t::permission_reporter_label(locale)) }
                    option value="developer" selected { (t::permission_developer_label(locale)) }
                    option value="maintainer" { (t::permission_maintainer_label(locale)) }
                    option value="owner" { (t::permission_owner_label(locale)) }
                }
                button type="submit" { (t::add_update_repo_button(locale)) }
            }
            form method="post" action="/_users" data-async data-async-target=(card_target) {
                (PreEscaped(csrf))
                input type="hidden" name="user_action" value="set_email";
                input type="hidden" name="username" value=(user.username);
                input type="email" name="email" size="30" value=(user.email.as_deref().unwrap_or(""))
                    placeholder=(t::set_email_placeholder(locale));
                button type="submit" { (t::set_email_button(locale)) }
            }
            form method="post" action="/_users" data-async data-async-target=(card_target) {
                (PreEscaped(csrf))
                input type="hidden" name="user_action" value="set_password";
                input type="hidden" name="username" value=(user.username);
                input type="password" name="password" size="30"
                    placeholder=(if user.password_hash.is_some() { t::password_no_changes_placeholder(locale) } else { t::password_none_placeholder(locale) });
                button type="submit" {
                    (if user.password_hash.is_some() { t::change_password_button(locale) } else { t::set_password_button(locale) })
                }
            }
            form method="post" action="/_users" data-async data-async-target=(card_target) {
                (PreEscaped(csrf))
                input type="hidden" name="user_action" value="set_storage_quota";
                input type="hidden" name="username" value=(user.username);
                input name="storage_quota" size="12" placeholder=(t::storage_quota_placeholder(locale));
                @if let Some(bytes) = user.storage_quota_bytes {
                    span class="muted" { (t::storage_quota_current_prefix(locale)) (gitquarry_domain::commands::repack::human_bytes(bytes)) }
                }
                button type="submit" { (t::set_storage_quota_button(locale)) }
            }
            form method="post" action="/_users" style="display:inline" data-async data-async-target=(card_target) {
                (PreEscaped(csrf))
                input type="hidden" name="user_action" value="toggle_admin";
                input type="hidden" name="username" value=(user.username);
                button type="submit" { (if user.is_admin { t::remove_admin_button(locale) } else { t::make_admin_button(locale) }) }
            }
            @if user.totp_enabled {
                form method="post" action="/_users" style="display:inline" data-async data-async-target=(card_target) {
                    (PreEscaped(csrf))
                    input type="hidden" name="user_action" value="disable_totp";
                    input type="hidden" name="username" value=(user.username);
                    button type="submit" { (t::disable_totp_button(locale)) }
                }
            }
            form method="post" action="/_users" style="display:inline" data-async data-async-target="#users-list" {
                (PreEscaped(csrf))
                input type="hidden" name="user_action" value="delete_user";
                input type="hidden" name="username" value=(user.username);
                button type="submit" { (t::delete_user_button(locale)) }
            }
        }
    }
}

/// Every [`Permission`] tier, low to high — shared by [`render_users_page`]'s
/// intro prose (lists them all) and the add-perm `<select>` (offers them
/// all), same split as `auth::web::groups`'s `PERMISSION_TIERS`.
const PERMISSION_TIERS: [Permission; 5] = [
    Permission::Guest,
    Permission::Reporter,
    Permission::Developer,
    Permission::Maintainer,
    Permission::Owner,
];

fn permission_label(perm: Permission, locale: gitquarry_i18n::Locale) -> String {
    use crate::auth::i18n::users as t;
    match perm {
        Permission::Guest => t::permission_guest_label(locale),
        Permission::Reporter => t::permission_reporter_label(locale),
        Permission::Developer => t::permission_developer_label(locale),
        Permission::Maintainer => t::permission_maintainer_label(locale),
        Permission::Owner => t::permission_owner_label(locale),
    }
}

/// One `<tr>` in a user's repo-permission table — `display` is what's shown
/// (a resolved repo name, or literally `"*"`), `field_value` is what
/// `remove_repo_perm`'s hidden `repo` field submits (the same string for a
/// real repo — the backend re-resolves it to a uid — or `"*"`, handled
/// specially there instead of a repo lookup).
fn repo_perm_row(
    user: &User,
    display: &str,
    field_value: &str,
    perm: Permission,
    csrf: &str,
    card_target: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use crate::auth::i18n::users as t;
    let perm_label = permission_label(perm, locale);
    html! {
        tr {
            td { code { (display) } }
            td { span class=(crate::webui::permission_badge_class(perm)) { (perm_label) } }
            td {
                form method="post" action="/_users" style="display:inline" data-async data-async-target=(card_target) {
                    (PreEscaped(csrf))
                    input type="hidden" name="user_action" value="remove_repo_perm";
                    input type="hidden" name="username" value=(user.username);
                    input type="hidden" name="repo" value=(field_value);
                    button type="submit" { (t::remove_button(locale)) }
                }
            }
        }
    }
}
