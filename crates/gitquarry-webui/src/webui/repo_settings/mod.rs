//! A single repo's "settings" tab:
//! - description/visibility (`<repo>/.conf/config.json` — `owner` lives in
//!   the same object but isn't editable here: it's always derived from the
//!   repo's own namespace, see [`general::save_conf`]. Actually changing who
//!   a repo belongs to means moving it, see [`move_repo::move_repo`]).
//! - default branch (`Manifest::head`, not `RepoConfig` — see
//!   [`default_branch::save_default_branch`]).
//! - per-user ssh:// permissions — a Pijul-Nest-style grid, adapted to what
//!   git actually has: no discussions/channels/tags, no "everybody" row
//!   (ssh:// always requires an authorized pubkey), just the five
//!   [`gitquarry_core::auth::access::Permission`] tiers per user, same enum
//!   [`gitquarry_core::auth::access`] already uses.
//! - deleting the repo outright (irreversible, gated by typing the repo's
//!   full name — see [`danger_zone::render_delete_repo_form`]).
//!
//! Split out of [`super::settings`] (server-wide settings) because it's a
//! per-repo concern with its own gate call ([`require_repo_authority`],
//! which — unlike [`super::settings::require_admin`] — also lets a
//! `Maintainer`/`Owner`-tier grant on this one repo through, not just
//! global admins) despite living behind the same `POST /{repo}` route as
//! the rest of [`super::repo`]'s views.
//!
//! One file per sub-form (mirrors the layout [`super::super::git::browse`]
//! uses for its own per-concern split), all wired together by
//! [`handler_repo_conf_post`] (the single `POST` dispatcher every sub-form
//! shares) and [`render_settings_page`] (assembles every sub-form's markup
//! into the one page).

#[cfg(any(test, feature = "test-util"))]
pub mod danger_zone;
#[cfg(not(any(test, feature = "test-util")))]
mod danger_zone;
#[cfg(any(test, feature = "test-util"))]
pub mod default_branch;
#[cfg(not(any(test, feature = "test-util")))]
mod default_branch;
#[cfg(any(test, feature = "test-util"))]
pub mod general;
#[cfg(not(any(test, feature = "test-util")))]
mod general;
#[cfg(any(test, feature = "test-util"))]
pub mod labels;
#[cfg(not(any(test, feature = "test-util")))]
mod labels;
#[cfg(any(test, feature = "test-util"))]
pub mod lfs_limit;
#[cfg(not(any(test, feature = "test-util")))]
mod lfs_limit;
#[cfg(any(test, feature = "test-util"))]
pub mod move_repo;
#[cfg(not(any(test, feature = "test-util")))]
mod move_repo;
#[cfg(any(test, feature = "test-util"))]
pub mod permissions;
#[cfg(not(any(test, feature = "test-util")))]
mod permissions;
#[cfg(any(test, feature = "test-util"))]
pub mod protected_branches;
#[cfg(not(any(test, feature = "test-util")))]
mod protected_branches;
mod protected_tags;
#[cfg(any(test, feature = "test-util"))]
pub mod require_signed_commits;
#[cfg(not(any(test, feature = "test-util")))]
mod require_signed_commits;
#[cfg(any(test, feature = "test-util"))]
pub mod required_reviewers;
#[cfg(not(any(test, feature = "test-util")))]
mod required_reviewers;
#[cfg(any(test, feature = "test-util"))]
pub mod topics;
#[cfg(not(any(test, feature = "test-util")))]
mod topics;

use super::{
    AppError, Flash, State_, csrf_field, effective_density, effective_locale, effective_theme, layout,
    render_repo_action_buttons, require_csrf, url_encode,
};
use crate::auth::web::login;
use crate::webui::repo_link;
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_core::storage::remote::Manifest;
use gitquarry_core::validate;
use maud::{Markup, html};

/// What each `save_*` helper below reports back, instead of a
/// `Flash<'static>` directly: the CAS-conflict case now needs
/// [`gitquarry_i18n::shared::cas_conflict`]'s locale-resolved *owned* `String`,
/// which can't fit `Flash`'s `&'static str` — so the actual `Flash` is only
/// built once, in [`handler_repo_conf_post`], where that owned string
/// already lives long enough to be borrowed from.
enum FlashOutcome {
    Ok(&'static str),
    Conflict,
    Invalid(String),
}

/// Whether `form`'s action needs `Owner`-tier authority (delete/transfer,
/// or granting `Maintainer`+ to someone else) or just `Maintainer` (every
/// other settings action, including granting a tier below `Maintainer`).
fn needs_owner_tier(form: &RepoConfForm) -> bool {
    match form.action.as_str() {
        "delete_repo" | "restore_repo" | "move_repo" => true,
        "set_perm" => matches!(form.perm.as_str(), "maintainer" | "owner"),
        _ => false,
    }
}

/// `None` if this request may proceed with the authority `form`'s action
/// needs on `repo` ([`access::can_manage_repo_members_for`] or
/// [`access::can_manage_repo_settings_for`], per [`needs_owner_tier`]);
/// `Some(response)` is what to return instead. Global admins always
/// qualify — folded into those two calls already, same as
/// `super::settings::require_admin`. This is what lets a `Maintainer`/
/// `Owner` grant on THIS repo manage it without needing `is_admin`. Same
/// "no login configured leaves admin pages open" and "session but not
/// entitled" split as that shared gate, just scoped to one repo's tier
/// instead of the global flag.
async fn require_repo_authority(
    ctx: &gitquarry_core::app::AppContext,
    headers: &HeaderMap,
    repo: &str,
    form: &RepoConfForm,
) -> Result<Option<Response>, AppError> {
    let login_configured = login::login_enforced(ctx).await;
    if !login_configured {
        return Ok(None);
    }
    let Some(user) = login::current_user(ctx, headers).await else {
        return Ok(Some(
            Redirect::to(&format!("/_login?return_to={}", url_encode(repo))).into_response(),
        ));
    };
    let allowed = if form.action == "purge_repo" {
        // Stricter than `needs_owner_tier`: skipping the grace window
        // entirely is an instance-admin call, not something a repo's own
        // Owner tier grants — see `domain::repo::purge`'s doc comment.
        user.is_admin
    } else if needs_owner_tier(form) {
        access::can_manage_repo_members_for(ctx.backend().as_ref(), &ctx.cipher, &user.username, repo).await?
    } else {
        access::can_manage_repo_settings_for(ctx.backend().as_ref(), &ctx.cipher, &user.username, repo).await?
    };
    if allowed {
        return Ok(None);
    }
    let locale = effective_locale(ctx, headers).await;
    Ok(Some(
        AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::insufficient_repo_permission(locale)
        ))
        .into_response(),
    ))
}

pub(crate) async fn handler_repo_conf_post(
    State(ctx): State_,
    Path(raw_repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<RepoConfForm>,
) -> Result<Response, AppError> {
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let name = validate::sanitize_path(&raw_repo).map_err(AppError::bad_request)?;
    if let Some(resp) = require_repo_authority(&ctx, &headers, &name, &form).await? {
        return Ok(resp);
    }
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo_not_found_in_bucket(locale, &name)
        )));
    };
    let users = access::list_all_users(&ctx.backend(), &ctx.cipher).await?;
    let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
    let actor = login::current_user(&ctx, &headers)
        .await
        .map(|u| u.username)
        .unwrap_or_default();

    let outcome = match form.action.as_str() {
        "delete_repo" if form.confirm_name.trim() != name => {
            FlashOutcome::Invalid(gitquarry_i18n::shared::type_name_to_confirm_deletion(locale, &name))
        }
        "delete_repo" => {
            gitquarry_core::domain::repo::delete(&ctx, &resolved, &name).await?;
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &actor,
                "repo.delete",
                &name,
                None,
            )
            .await;
            return Ok(Redirect::to("/").into_response());
        }
        "purge_repo" if form.confirm_name.trim() != name => {
            FlashOutcome::Invalid(gitquarry_i18n::shared::type_name_to_confirm_deletion(locale, &name))
        }
        "purge_repo" => {
            gitquarry_core::domain::repo::purge(&ctx, &resolved, &name).await?;
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &actor,
                "repo.purge",
                &name,
                None,
            )
            .await;
            return Ok(Redirect::to("/").into_response());
        }
        "restore_repo" => {
            gitquarry_core::domain::repo::restore(&ctx, &resolved).await?;
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &actor,
                "repo.restore",
                &name,
                None,
            )
            .await;
            return Ok(Redirect::to(&repo_link(&name, "settings", "").to_string()).into_response());
        }
        "move_repo" => match move_repo::move_repo(&ctx, &resolved, &name, &form, locale).await? {
            Err(msg) => FlashOutcome::Invalid(msg),
            Ok(new_name) => {
                gitquarry_core::storage::audit::record(
                    ctx.backend().as_ref(),
                    &ctx.cipher,
                    &actor,
                    "repo.move",
                    &name,
                    Some(new_name.clone()),
                )
                .await;
                return Ok(Redirect::to(&format!("/{}", url_encode(&new_name))).into_response());
            }
        },
        "set_perm" => permissions::save_perm(&ctx, &resolved, &name, &form, &actor, locale).await?,
        "set_default_branch" => {
            default_branch::save_default_branch(&ctx, &resolved, &name, &form, &actor, locale).await?
        }
        "set_protected_branches" => {
            protected_branches::save_protected_branches(&ctx, &resolved, &name, &form, &actor, locale).await?
        }
        "set_protected_tags" => {
            protected_tags::save_protected_tags(&ctx, &resolved, &name, &form, &actor, locale).await?
        }
        "set_require_signed_commits" => {
            require_signed_commits::save_require_signed_commits(&ctx, &resolved, &name, &form, &actor, locale).await?
        }
        "set_topics" => topics::save_topics(&ctx, &resolved, &name, &form, &actor).await?,
        "set_required_reviewers" => {
            required_reviewers::save_required_reviewers(&ctx, &resolved, &name, &form, &actor, locale).await?
        }
        "set_labels" => labels::save_labels(&ctx, &resolved, &name, &form, &actor).await?,
        "set_lfs_max_blob_size" => lfs_limit::save_lfs_max_blob_size(&ctx, &resolved, &name, &form, &actor).await?,
        _ => general::save_conf(&ctx, &resolved, &name, &form, &actor).await?,
    };
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let flash = match &outcome {
        FlashOutcome::Ok(msg) => Flash::Ok(msg),
        FlashOutcome::Conflict => Flash::Err(&cas_conflict_msg),
        FlashOutcome::Invalid(msg) => Flash::Err(msg),
    };
    // `Some(&form)` only on a rejected/conflicted save — every sub-form
    // below re-renders from freshly-refetched `config`/`manifest` regardless
    // of outcome, so without this a failed submit would silently discard
    // whatever the user just typed and show the old stored value instead.
    let retry_form: Option<&RepoConfForm> =
        matches!(outcome, FlashOutcome::Conflict | FlashOutcome::Invalid(_)).then_some(&form);

    let (config, _etag) = resolved.remote.get_repo_config().await?;
    let (manifest, _etag) = resolved.remote.get_manifest().await?;
    let actions = render_repo_action_buttons(&ctx, &name, &resolved.uid, &headers).await;
    let owner_display =
        access::resolve_owner_display(ctx.backend().as_ref(), &ctx.cipher, config.owner.as_ref()).await?;
    let granted = access::list_repo_permissions(&ctx.backend(), &ctx.cipher, &resolved.uid).await?;
    // Async (`data-async-target="#perm-grid"`, see `super::ASYNC_HEADER`'s
    // own doc comment): `set_perm` (row save or the add-access form) only
    // ever changes this one grid, and `granted` is already fetched above
    // regardless of which action fired.
    if headers.contains_key(super::ASYNC_HEADER) && form.action == "set_perm" {
        return Ok(axum::response::Html(
            permissions::render_perm_grid_fragment(&name, &granted, &csrf_field(&headers), locale).into_string(),
        )
        .into_response());
    }
    let density = effective_density(&ctx, &headers).await;
    let header = super::header_bar(
        &name,
        "",
        "settings",
        super::HeaderExtras {
            description: config.description.as_deref(),
            archived: config.archived,
            visibility: config.visibility,
            branches: &[],
            menu_extra: &actions,
            below_header: "",
            sub_tabs: "",
            locale,
        },
    );
    let topnav = login::topnav(&ctx, &headers).await;
    let body = render_settings_page(
        &name,
        &config,
        owner_display.as_deref().unwrap_or(""),
        Namespaces {
            users: &users,
            group_names: &group_names,
        },
        RepoState {
            manifest: &manifest,
            granted: &granted,
        },
        PageChrome {
            csrf: &csrf_field(&headers),
            message: Some(flash),
            retry: retry_form,
            locale,
        },
    )
    .into_string();
    Ok(axum::response::Html(layout(
        &name,
        &header,
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        density,
        locale,
    ))
    .into_response())
}

// `pub`, not `pub(crate)`: now referenced (as `Option<&RepoConfForm>`, the
// `retry` param) by every sub-form's render fn, several of which are `pub`
// under `test`/`test-util` for their own integration tests — a private type
// can't sit in a public fn's signature across that crate boundary (E0446),
// same reason `Flash` (`flash.rs`) is already dual-gated pub/pub(crate).
#[derive(serde::Deserialize)]
pub struct RepoConfForm {
    /// `"save_conf"` (default — the description/visibility form below
    /// doesn't send this field at all) or one of `set_perm`/`set_topics`/
    /// `delete_repo`/... (every other sub-form on this page). One
    /// `POST /{repo}` handles all of them so the settings tab stays a
    /// single route, same convention `/_users` uses for its own actions.
    ///
    /// Wire name `conf_action`, not `action`: `permissions.rs`'s `set_perm`
    /// forms are `data-async` -- `HTMLFormElement` is `[OverrideBuiltins]`, so a
    /// control literally named `action` shadows the form's own `.action`
    /// IDL property, and `style/app.js`'s `fetchFragment(form.action, ...)`
    /// got the `<input>` element instead of the URL, 404-ing. Every sub-form
    /// under `repo_settings/` shares this one struct/field, so the rename
    /// applies even to the plain (non-`data-async`) ones for consistency.
    #[serde(default, rename = "conf_action")]
    action: String,
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    visibility: String,
    /// Only read by `save_conf` — checkbox, present ("1") only if checked.
    #[serde(default)]
    archived: String,
    /// Only read by `save_conf` — checkbox, present ("1") only if checked.
    #[serde(default)]
    is_template: String,
    /// Only read by `set_perm`.
    #[serde(default)]
    username: String,
    /// Only read by `set_perm`: one of the five `Permission` tier names, or
    /// `"none"` (clears the explicit entry — falls back to that user's own
    /// `*` default, if any, same as any other repo they haven't listed).
    #[serde(default)]
    perm: String,
    /// A branch name (no `refs/heads/` prefix) — only read by
    /// `set_default_branch`.
    #[serde(default)]
    branch: String,
    /// Comma-separated branch names — only read by `set_protected_branches`.
    #[serde(default)]
    protected: String,
    /// Comma-separated tag names — only read by `set_protected_tags`.
    #[serde(default)]
    protected_tags: String,
    /// Comma-separated branch names — only read by `set_require_signed_commits`.
    #[serde(default)]
    signed_required: String,
    /// Comma-separated topic labels — only read by `set_topics`.
    #[serde(default)]
    topics: String,
    /// Comma-separated usernames — only read by `set_required_reviewers`.
    #[serde(default)]
    required_reviewers: String,
    /// `"name:#color, name:#color, ..."` — only read by `set_labels`.
    #[serde(default)]
    labels: String,
    /// `gitquarry_core::mirror::gc::parse_size` syntax (`"50M"`) — only read by
    /// `set_lfs_max_blob_size`. Empty clears it (feature off for this repo).
    #[serde(default)]
    lfs_max_blob_size: String,
    /// Only read by `delete_repo`/`purge_repo`/`move_repo`: has to equal the
    /// repo's full (pre-move) name exactly, or the action is refused —
    /// typing it out is the only friction between clicking a button and an
    /// irreversible `DeleteObjects`, or every existing clone URL breaking.
    #[serde(default)]
    confirm_name: String,
    /// Only read by `move_repo`: the destination's namespace (an existing
    /// username or group — same rule as `owner`) and the repo-local part
    /// of its new name.
    #[serde(default)]
    new_namespace: String,
    #[serde(default)]
    new_local_name: String,
}

/// `users`/`group_names` always travel together on this page — every
/// namespace-picking form (owner transfer, move) offers both as options —
/// bundled so [`render_settings_page`] doesn't carry two more positional
/// slice params on top of everything else it already does.
pub(crate) struct Namespaces<'a> {
    pub users: &'a [User],
    pub group_names: &'a [String],
}

/// `manifest`/`granted` always travel together into [`render_settings_page`]
/// too — this repo's own live branch list and its own permission grants —
/// bundled for the same reason [`Namespaces`] is: one struct slot instead of
/// two more positional params clippy's `too_many_arguments` would flag.
pub(crate) struct RepoState<'a> {
    pub manifest: &'a Manifest,
    pub granted: &'a [(User, Permission)],
}

/// `csrf`/`message`/`locale` bundled so [`render_settings_page`] doesn't
/// cross clippy's `too_many_arguments` now that `locale` (needed once its
/// sub-forms became locale-aware) would have been the 8th positional param —
/// same reasoning as [`Namespaces`]/[`RepoState`] and `groups::PageChrome`.
pub(crate) struct PageChrome<'a> {
    pub csrf: &'a str,
    pub message: Option<Flash<'a>>,
    /// `Some(&form)` only when the just-submitted action was rejected or
    /// hit a CAS conflict — lets whichever single sub-form actually failed
    /// re-render the user's own typed values instead of the stale stored
    /// ones. See [`handler_repo_conf_post`]'s own comment on why this can't
    /// just always be `Some`.
    pub retry: Option<&'a RepoConfForm>,
    pub locale: gitquarry_i18n::Locale,
}

pub(crate) fn render_settings_page(
    repo_name: &str,
    config: &RepoConfig,
    current_owner: &str,
    namespaces: Namespaces<'_>,
    state: RepoState<'_>,
    chrome: PageChrome<'_>,
) -> Markup {
    let Namespaces { users, group_names } = namespaces;
    let RepoState { manifest, granted } = state;
    let PageChrome {
        csrf,
        message,
        retry,
        locale,
    } = chrome;
    // Scopes `retry` (only ever the one action that just failed, if any)
    // down to the single sub-form it actually belongs to — every other
    // sub-form gets `None` and falls back to its own stored value, same as
    // a plain page load.
    let retry_for = |action: &str| retry.filter(|f| f.action == action);
    html! {
        (general::render_repo_conf_form(repo_name, config, current_owner, csrf, message, retry_for(""), locale))
        (default_branch::render_default_branch_form(repo_name, manifest, csrf, retry_for("set_default_branch"), locale))
        (protected_branches::render_protected_branches_form(repo_name, config, manifest, csrf, retry_for("set_protected_branches"), locale))
        (protected_tags::render_protected_tags_form(repo_name, config, manifest, csrf, retry_for("set_protected_tags"), locale))
        (require_signed_commits::render_require_signed_commits_form(repo_name, config, manifest, csrf, retry_for("set_require_signed_commits"), locale))
        (topics::render_topics_form(repo_name, config, csrf, retry_for("set_topics"), locale))
        (required_reviewers::render_required_reviewers_form(repo_name, config, csrf, retry_for("set_required_reviewers"), locale))
        (labels::render_labels_form(repo_name, config, csrf, retry_for("set_labels"), locale))
        (lfs_limit::render_lfs_limit_form(repo_name, config, csrf, retry_for("set_lfs_max_blob_size"), locale))
        (crate::webui::settings_section(
            &gitquarry_i18n::web::repo_settings::permissions_heading(locale),
            None,
            permissions::render_perm_grid(repo_name, granted, csrf, locale),
        ))
        (move_repo::render_move_repo_form(repo_name, users, group_names, csrf, retry_for("move_repo"), locale))
        (danger_zone::render_delete_repo_form(repo_name, config.deleted_at, csrf, retry_for("delete_repo"), retry_for("purge_repo"), locale))
    }
}
