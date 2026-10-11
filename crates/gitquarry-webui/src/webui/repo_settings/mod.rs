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
//!   [`gitquarry_core::auth::entities::Permission`] tiers per user, same enum
//!   [`gitquarry_core::auth::access`] already uses.
//! - deleting the repo outright (irreversible, gated by typing the repo's
//!   full name — see [`danger_zone::render_delete_repo_form`]).
//!
//! Split out of [`crate::settings`] (server-wide settings) because it's a
//! per-repo concern with its own gate call ([`require_repo_authority`],
//! which — unlike [`crate::settings::web::require_admin`] — also lets a
//! `Maintainer`/`Owner`-tier grant on this one repo through, not just
//! global admins) despite living behind the same `POST /{repo}` route as
//! the rest of [`super::repo`]'s views.
//!
//! One file per sub-form (mirrors the layout `gitquarry_core::git::browse`
//! uses for its own per-concern split), each its own `?view=settings_<id>`
//! page (`settings_sections`, the sidebar in [`render_settings_section`]) —
//! the general/description
//! section alone keeps the bare `?view=settings` id, both because it's the
//! first tab and because it's also `header_bar`'s own top-level "Settings"
//! tab target. All wired together by [`handler_repo_conf_post`] (the single
//! `POST` dispatcher every sub-form still shares — one route, same as
//! before, only the *response* narrowed from all 13 sections to whichever
//! one owns the action that just posted) and [`render_settings_section`]
//! (assembles one section's sub-nav + markup, called by both that POST
//! handler and `repo::handler_repo`'s own GET dispatch).

mod danger_zone;
mod default_branch;
mod general;
mod labels;
mod lfs_limit;
mod mirrors;
mod move_repo;
mod permissions;
mod protected_branches;
mod protected_tags;
mod require_signed_commits;
mod required_reviewers;
mod topics;

use super::{
    AppError, Flash, State_, csrf_field, effective_density, effective_locale, effective_theme, layout, render_flash,
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
use maud::{Markup, PreEscaped, html};

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
        // A mirror ships this repo's contents to a third party and stores a
        // credential for it — the same ownership-level decision as handing
        // the repo to someone else, not an everyday setting like a
        // description or a protected branch. `sync_mirror_now` is in here
        // too: it's the one action that reaches the external side
        // immediately, without waiting for a sweep.
        "add_mirror" | "edit_mirror" | "delete_mirror" | "sync_mirror_now" => true,
        "set_perm" => matches!(form.perm.as_str(), "maintainer" | "owner"),
        _ => false,
    }
}

/// Whether a `set_perm` targets someone who already holds `Maintainer`+ on
/// `repo`. [`needs_owner_tier`] only sees the *new* tier, so without this a
/// `Maintainer` could demote or strip an `Owner` by submitting a lower one.
async fn touches_privileged_member(
    ctx: &gitquarry_core::app::AppContext,
    repo: &str,
    form: &RepoConfForm,
) -> Result<bool, AppError> {
    if form.action != "set_perm" {
        return Ok(false);
    }
    let current = access::effective_permission_for(ctx.backend().as_ref(), &ctx.cipher, &form.username, repo).await?;
    Ok(matches!(current, Some(Permission::Maintainer | Permission::Owner)))
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
        // Same first-boot handling as `settings::web::require_admin`: the
        // startup token, exchanged at `/_bootstrap`, is what stands in for
        // the session that can't exist yet.
        if crate::webui::bootstrap::holds_grant(ctx, headers) {
            return Ok(None);
        }
        let locale = effective_locale(ctx, headers).await;
        return Ok(Some(
            AppError::forbidden(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::settings::bootstrap_grant_required(locale)
            ))
            .into_response(),
        ));
    }
    let Some(user) = login::current_user(ctx, headers).await else {
        return Ok(Some(
            Redirect::to(&format!("/_login?return_to={}", url_encode(&format!("/{repo}")))).into_response(),
        ));
    };
    let allowed = if form.action == "purge_repo" {
        // Stricter than `needs_owner_tier`: skipping the grace window
        // entirely is an instance-admin call, not something a repo's own
        // Owner tier grants — see `domain::repo::purge`'s doc comment.
        user.is_admin
    } else if needs_owner_tier(form) || touches_privileged_member(ctx, repo, form).await? {
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
            gitquarry_i18n::shared::repo::insufficient_repo_permission(locale)
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
            gitquarry_i18n::shared::repo::repo_not_found_in_bucket(locale, &name)
        )));
    };
    let users = access::list_all_users(&ctx.backend(), &ctx.cipher).await?;
    let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
    let actor = login::current_user(&ctx, &headers)
        .await
        .map(|u| u.username)
        .unwrap_or_default();

    let (outcome, pending_mirror_token) = match form.action.as_str() {
        "delete_repo" if form.confirm_name.trim() != name => (
            FlashOutcome::Invalid(gitquarry_i18n::shared::repo::type_name_to_confirm_deletion(
                locale, &name,
            )),
            None,
        ),
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
        "purge_repo" if form.confirm_name.trim() != name => (
            FlashOutcome::Invalid(gitquarry_i18n::shared::repo::type_name_to_confirm_deletion(
                locale, &name,
            )),
            None,
        ),
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
            Err(msg) => (FlashOutcome::Invalid(msg), None),
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
        "set_perm" => (
            permissions::save_perm(&ctx, &resolved, &name, &form, &actor, locale).await?,
            None,
        ),
        "set_default_branch" => (
            default_branch::save_default_branch(&ctx, &resolved, &name, &form, &actor, locale).await?,
            None,
        ),
        "set_protected_branches" => (
            protected_branches::save_protected_branches(&ctx, &resolved, &name, &form, &actor, locale).await?,
            None,
        ),
        "set_protected_tags" => (
            protected_tags::save_protected_tags(&ctx, &resolved, &name, &form, &actor, locale).await?,
            None,
        ),
        "set_require_signed_commits" => (
            require_signed_commits::save_require_signed_commits(&ctx, &resolved, &name, &form, &actor, locale).await?,
            None,
        ),
        "set_topics" => (topics::save_topics(&ctx, &resolved, &name, &form, &actor).await?, None),
        "set_required_reviewers" => (
            required_reviewers::save_required_reviewers(&ctx, &resolved, &name, &form, &actor, locale).await?,
            None,
        ),
        "set_labels" => (labels::save_labels(&ctx, &resolved, &name, &form, &actor).await?, None),
        "set_lfs_max_blob_size" => (
            lfs_limit::save_lfs_max_blob_size(&ctx, &resolved, &name, &form, &actor).await?,
            None,
        ),
        "add_mirror" | "edit_mirror" | "delete_mirror" | "sync_mirror_now" => {
            mirrors::save_mirror(&ctx, &resolved, &name, &form, &actor, locale).await?
        }
        _ => (general::save_conf(&ctx, &resolved, &name, &form, &actor).await?, None),
    };
    let is_async = headers.contains_key(super::ASYNC_HEADER);
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    // Async is different from the plain fallback below: none of this page's
    // three async targets (`#mirrors-section`, `#perm-grid`, and every
    // other action's `#repo-settings-section-body` — see
    // `render_settings_section`'s own doc comment) is guaranteed to
    // contain a flash banner (the first two don't), so a rejected/
    // conflicted save goes through `AppError` here instead: a real non-2xx
    // status, picked up by `extractFlashMessage`/`showErrorToast` the same
    // way any other failed `data-async` request already is — same
    // reasoning `keys::handler_keys_post`'s own doc comment gives for its
    // `fail!`. Checked before any of `save_conf`/`save_topics`/etc.'s
    // success paths below, so it also covers the two narrow fragments that
    // were already async (`sync_mirror_now`/`delete_mirror`/`set_perm`)
    // and previously had this same gap.
    if is_async {
        match &outcome {
            FlashOutcome::Conflict => return Err(AppError::bad_request(anyhow::anyhow!("{}", cas_conflict_msg))),
            FlashOutcome::Invalid(msg) => return Err(AppError::bad_request(anyhow::anyhow!("{msg}"))),
            FlashOutcome::Ok(_) => {}
        }
    }
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
    let mirrors_list = gitquarry_mirror_external::domain::list_with_etags(&ctx, &resolved.uid).await?;
    let mr_links_by_mirror = mirrors::fetch_mr_links_by_mirror(&ctx, &resolved.uid, &mirrors_list).await?;
    // Async (`data-async-target="#mirrors-section"`, see `super::ASYNC_HEADER`'s
    // own doc comment): `sync_mirror_now`/`delete_mirror` only ever change
    // this one section, and everything `render_mirrors_section` needs is
    // already fetched above regardless of which action fired -- no extra
    // query for this path.
    if is_async && matches!(form.action.as_str(), "sync_mirror_now" | "delete_mirror") {
        return Ok(axum::response::Html(
            mirrors::render_mirrors_section(
                &name,
                &mirrors_list,
                &mr_links_by_mirror,
                &csrf_field(&headers),
                pending_mirror_token.as_ref(),
                locale,
            )
            .into_string(),
        )
        .into_response());
    }
    // Async (`data-async-target="#perm-grid"`, see `super::ASYNC_HEADER`'s
    // own doc comment): `set_perm` (row save or the add-access form) only
    // ever changes this one grid, and `granted` is already fetched above
    // regardless of which action fired.
    if is_async && form.action == "set_perm" {
        return Ok(axum::response::Html(
            permissions::render_perm_grid_fragment(&name, &granted, &csrf_field(&headers), locale).into_string(),
        )
        .into_response());
    }
    // Async, every other action (general/topics/labels/protected-branches/
    // protected-tags/signed-commits/reviewers/lfs-limit/move/danger-zone's
    // own name-mismatch retry — everything not already handled by the two
    // narrower targets above): `render_settings_section` re-renders the
    // one active section, wrapped in `#repo-settings-body` (see that fn's
    // own doc comment) so `data-async-target="#repo-settings-body"` has
    // something to swap.
    if is_async {
        return Ok(axum::response::Html(
            render_settings_section(
                section_for_action(&form.action),
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
                    mirrors: &mirrors_list,
                    mr_links: &mr_links_by_mirror,
                },
                PageChrome {
                    csrf: &csrf_field(&headers),
                    message: Some(flash),
                    pending_mirror_token: pending_mirror_token.as_ref(),
                    retry: retry_form,
                    locale,
                },
            )
            .into_string(),
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
            display_name: config.display_name.as_deref(),
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
    let body = render_settings_section(
        section_for_action(&form.action),
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
            mirrors: &mirrors_list,
            mr_links: &mr_links_by_mirror,
        },
        PageChrome {
            csrf: &csrf_field(&headers),
            message: Some(flash),
            pending_mirror_token: pending_mirror_token.as_ref(),
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

#[derive(serde::Deserialize)]
pub(crate) struct RepoConfForm {
    /// `"save_conf"` (default — the description/visibility form below
    /// doesn't send this field at all) or one of `set_perm`/`set_topics`/
    /// `delete_repo`/... (every other sub-form on this page). One
    /// `POST /{repo}` handles all of them so the settings tab stays a
    /// single route, same convention `/_users` uses for its own actions.
    ///
    /// Wire name `conf_action`, not `action`: `permissions.rs`'s `set_perm`
    /// forms and `mirrors.rs`'s `sync_mirror_now`/`delete_mirror` forms are
    /// `data-async` -- `HTMLFormElement` is `[OverrideBuiltins]`, so a
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
    /// Only read by `edit_mirror`/`delete_mirror`/`sync_mirror_now`.
    #[serde(default)]
    mirror_id: String,
    /// Only read by `edit_mirror` — CAS guard, same role `confirm_name`
    /// plays for delete/move but for concurrency instead of confirmation.
    #[serde(default)]
    mirror_etag: String,
    /// Only read by `add_mirror`/`edit_mirror`.
    #[serde(default)]
    remote_url: String,
    /// Only read by `add_mirror`/`edit_mirror` — empty on an edit means
    /// "keep the currently stored token", never "clear it" (there's no way
    /// to have an enabled mirror with no token at all).
    #[serde(default)]
    token: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    direction: String,
    #[serde(default)]
    label: String,
    /// Only read by `add_mirror`/`edit_mirror` — checkbox, present ("1")
    /// only if checked.
    #[serde(default)]
    mirror_enabled: String,
    /// Only read by `add_mirror`/`edit_mirror` — checkbox, present ("1")
    /// only if checked. Track B opt-in, see `ExternalMirror::sync_merge_requests`.
    #[serde(default)]
    sync_merge_requests: String,
    /// Only read by `add_mirror`/`edit_mirror` — comma-separated, same
    /// convention as the repo-settings protected-branches field. See
    /// `ExternalMirror::branch_patterns`.
    #[serde(default)]
    branch_patterns: String,
}

/// `users`/`group_names` always travel together on this page — every
/// namespace-picking form (owner transfer, move) offers both as options —
/// bundled so [`render_settings_section`] doesn't carry two more positional
/// slice params on top of everything else it already does.
pub(crate) struct Namespaces<'a> {
    pub users: &'a [User],
    pub group_names: &'a [String],
}

/// Re-exported so `webui::repo`'s own GET handler (a sibling module, not a
/// descendant of this one) can fetch the same data
/// [`handler_repo_conf_post`] does without `mirrors` itself needing to be
/// anything but private — same "cross into a submodule only through a
/// `pub(crate)` item right here" convention every other name reaching
/// outside this module already follows.
pub(crate) use mirrors::fetch_mr_links_by_mirror;

/// `manifest`/`granted` always travel together into [`render_settings_section`]
/// too — this repo's own live branch list and its own permission grants —
/// bundled for the same reason [`Namespaces`] is: one struct slot instead of
/// two more positional params clippy's `too_many_arguments` would flag.
pub(crate) struct RepoState<'a> {
    pub manifest: &'a Manifest,
    pub granted: &'a [(User, Permission)],
    /// Each mirror with its current etag — the edit form's CAS guard, see
    /// `mirrors::render_mirror_form`.
    pub mirrors: &'a [(gitquarry_mirror_external::access::ExternalMirror, String)],
    /// Track B: every mirror's linked MRs, keyed by mirror id — see
    /// `mirrors::fetch_mr_links_by_mirror`.
    pub mr_links: &'a std::collections::HashMap<String, Vec<gitquarry_mirror_external::access::ExternalPrLink>>,
}

/// `csrf`/`message`/`locale` bundled so [`render_settings_section`] doesn't
/// cross clippy's `too_many_arguments` now that `locale` (needed once its
/// sub-forms became locale-aware) would have been the 8th positional param —
/// same reasoning as [`Namespaces`]/[`RepoState`] and `groups::PageChrome`.
pub(crate) struct PageChrome<'a> {
    pub csrf: &'a str,
    pub(crate) message: Option<Flash<'a>>,
    /// `Some((mirror_id, token))` for exactly one response: right after
    /// `add_mirror`/`edit_mirror` sets a token, never on a later page load
    /// (that would mean fetching a real, live, reversible secret back out
    /// of storage just to redisplay it) — same "shown once" discipline
    /// `auth::web::security`'s `pending_totp_secret` already established.
    pub pending_mirror_token: Option<&'a (String, String)>,
    /// `Some(&form)` only when the just-submitted action was rejected or
    /// hit a CAS conflict — lets whichever single sub-form actually failed
    /// re-render the user's own typed values instead of the stale stored
    /// ones. See [`handler_repo_conf_post`]'s own comment on why this can't
    /// just always be `Some`.
    pub retry: Option<&'a RepoConfForm>,
    pub locale: gitquarry_i18n::Locale,
}

/// Which of [`settings_sections`]' ids owns a given `RepoConfForm::action` —
/// [`handler_repo_conf_post`] uses this to know which single section to
/// re-render after a save, same job `RepoConfForm`'s own doc comment
/// describes for `action` itself, just inverted (action → owning page
/// instead of page → accepted actions). Falls back to `"settings"` (general)
/// for `""`/anything unrecognized, matching [`render_settings_section`]'s own
/// fallback arm.
fn section_for_action(action: &str) -> &'static str {
    match action {
        "set_default_branch" => "settings_default_branch",
        "set_protected_branches" => "settings_protected_branches",
        "set_protected_tags" => "settings_protected_tags",
        "set_require_signed_commits" => "settings_signed_commits",
        "set_topics" => "settings_topics",
        "set_required_reviewers" => "settings_reviewers",
        "set_labels" => "settings_labels",
        "set_lfs_max_blob_size" => "settings_lfs",
        "set_perm" => "settings_permissions",
        "add_mirror" | "edit_mirror" | "delete_mirror" | "sync_mirror_now" => "settings_mirrors",
        "move_repo" => "settings_move",
        "delete_repo" | "purge_repo" => "settings_danger_zone",
        _ => "settings",
    }
}

/// Section ids/hrefs/labels for [`render_settings_section`]'s own sidebar —
/// same "id, href, label" shape [`super::account_sections`] uses for the
/// account settings sidebar (this one's `.settings-sidebar`/
/// `.settings-layout` too — same CSS, a second instance). Used to render as
/// `project_subnav`'s horizontal tab row instead, which read fine for
/// account settings' 6 entries but wrapped badly (or forced `table.tabs`'s
/// own horizontal scroll) at this page's 13.
/// `"settings"`, not `"settings_general"`: it's also `header_bar`'s
/// top-level "Settings" tab target, so a bare `?view=settings` link (the
/// tab itself, [`handler_repo_conf_post`]'s `restore_repo` redirect) lands
/// on a real section instead of a dead end that'd need its own
/// default-redirect.
fn settings_sections(repo_name: &str, locale: gitquarry_i18n::Locale) -> [(&'static str, String, String); 13] {
    use gitquarry_i18n::web::repo_settings as t;
    let link = |view: &str| repo_link(repo_name, view, "").to_string();
    [
        ("settings", link("settings"), t::repo_conf_heading(locale)),
        (
            "settings_default_branch",
            link("settings_default_branch"),
            t::default_branch_heading(locale),
        ),
        (
            "settings_protected_branches",
            link("settings_protected_branches"),
            t::protected_branches_heading(locale),
        ),
        (
            "settings_protected_tags",
            link("settings_protected_tags"),
            t::protected_tags_heading(locale),
        ),
        (
            "settings_signed_commits",
            link("settings_signed_commits"),
            t::require_signed_commits_heading(locale),
        ),
        ("settings_topics", link("settings_topics"), t::topics_heading(locale)),
        (
            "settings_reviewers",
            link("settings_reviewers"),
            t::required_reviewers_heading(locale),
        ),
        ("settings_labels", link("settings_labels"), t::labels_heading(locale)),
        ("settings_lfs", link("settings_lfs"), t::lfs_limit_heading(locale)),
        (
            "settings_permissions",
            link("settings_permissions"),
            t::permissions_heading(locale),
        ),
        ("settings_mirrors", link("settings_mirrors"), t::mirrors_heading(locale)),
        ("settings_move", link("settings_move"), t::move_repo_heading(locale)),
        (
            "settings_danger_zone",
            link("settings_danger_zone"),
            t::delete_repo_label(locale),
        ),
    ]
}

/// `section` is one of [`settings_sections`]'s ids — everything else this
/// page's data (`?view=`-dispatched on the GET side, [`section_for_action`]
/// -dispatched after a POST) is still fetched unconditionally by both
/// callers, same as before this split, when one call here rendered all 13
/// sections at once: splitting *what's shown* from *what's fetched* would
/// trade a bigger diff across both call sites for a per-tab fetch saving
/// neither caller asked for. Only the assembly changed — a sidebar row per
/// [`settings_sections`] entry plus exactly one sub-form's markup instead
/// of all 13 stacked.
///
/// The whole thing (sidebar, flash, and the one active section) is wrapped
/// in `#repo-settings-body` — every sub-form under `repo_settings/` other
/// than the mirrors/permissions sections (which have their own narrower
/// `#mirrors-section`/`#perm-grid` targets) is `data-async
/// data-async-target="#repo-settings-body"`, so a save re-renders in
/// place instead of leaving the browser's "confirm resubmission" dialog
/// for a later F5.
pub(crate) fn render_settings_section(
    section: &str,
    repo_name: &str,
    config: &RepoConfig,
    current_owner: &str,
    namespaces: Namespaces<'_>,
    state: RepoState<'_>,
    chrome: PageChrome<'_>,
) -> Markup {
    let Namespaces { users, group_names } = namespaces;
    let RepoState {
        manifest,
        granted,
        mirrors,
        mr_links,
    } = state;
    let PageChrome {
        csrf,
        message,
        pending_mirror_token,
        retry,
        locale,
    } = chrome;
    // Scopes `retry` (only ever the one action that just failed, if any)
    // down to the single sub-form it actually belongs to — every other
    // sub-form gets `None` and falls back to its own stored value, same as
    // a plain page load.
    let retry_for = |action: &str| retry.filter(|f| f.action == action);
    let sections = settings_sections(repo_name, locale);
    let tabs: Vec<(&str, String, String)> = sections
        .iter()
        .map(|(id, href, label)| (*id, href.clone(), label.clone()))
        .collect();
    // Normalized once, up front, rather than matched separately by the
    // sub-nav below and the `content` dispatch further down: an
    // unrecognized `section` (a stale bookmark, a typo'd `?view=`) falls
    // back to general content either way (`content`'s own `_` arm), so the
    // sub-nav needs the same fallback id to highlight the tab that actually
    // matches what's showing — highlighting nothing while general's content
    // renders anyway would be its own small "which tab am I on" bug.
    let active = sections
        .iter()
        .find(|(id, _, _)| *id == section)
        .map_or("settings", |(id, _, _)| id);
    let content = match active {
        "settings_default_branch" => default_branch::render_default_branch_form(
            repo_name,
            manifest,
            csrf,
            retry_for("set_default_branch"),
            locale,
        ),
        "settings_protected_branches" => protected_branches::render_protected_branches_form(
            repo_name,
            config,
            manifest,
            csrf,
            retry_for("set_protected_branches"),
            locale,
        ),
        "settings_protected_tags" => protected_tags::render_protected_tags_form(
            repo_name,
            config,
            manifest,
            csrf,
            retry_for("set_protected_tags"),
            locale,
        ),
        "settings_signed_commits" => require_signed_commits::render_require_signed_commits_form(
            repo_name,
            config,
            manifest,
            csrf,
            retry_for("set_require_signed_commits"),
            locale,
        ),
        "settings_topics" => topics::render_topics_form(repo_name, config, csrf, retry_for("set_topics"), locale),
        "settings_reviewers" => required_reviewers::render_required_reviewers_form(
            repo_name,
            config,
            csrf,
            retry_for("set_required_reviewers"),
            locale,
        ),
        "settings_labels" => labels::render_labels_form(repo_name, config, csrf, retry_for("set_labels"), locale),
        "settings_lfs" => {
            lfs_limit::render_lfs_limit_form(repo_name, config, csrf, retry_for("set_lfs_max_blob_size"), locale)
        }
        "settings_permissions" => crate::webui::settings_section(
            &gitquarry_i18n::web::repo_settings::permissions_heading(locale),
            None,
            permissions::render_perm_grid(repo_name, granted, csrf, locale),
        ),
        "settings_mirrors" => {
            mirrors::render_mirrors_section(repo_name, mirrors, mr_links, csrf, pending_mirror_token, locale)
        }
        "settings_move" => {
            move_repo::render_move_repo_form(repo_name, users, group_names, csrf, retry_for("move_repo"), locale)
        }
        "settings_danger_zone" => danger_zone::render_delete_repo_form(
            repo_name,
            config.deleted_at,
            csrf,
            retry_for("delete_repo"),
            retry_for("purge_repo"),
            locale,
        ),
        // "settings" (general) falls in here too, alongside any unrecognized
        // `?view=` value — same "unknown view, show the first tab" fallback
        // every other repo view already has via `handler_repo`'s own
        // `view.get("view").map_or("about", ...)`.
        _ => general::render_repo_conf_form(repo_name, config, current_owner, csrf, None, retry_for(""), locale),
    };
    html! {
      div id="repo-settings-body" {
        // Vertical sidebar instead of `project_subnav`'s horizontal tab
        // row — same `.settings-layout`/`.settings-sidebar`/
        // `.settings-content` `account_settings_page` already uses for its
        // own 6-section nav, reused as-is rather than a new look: 13
        // sections is too many for one horizontal row on anything but a
        // wide viewport without either wrapping badly or forcing
        // `table.tabs`'s own horizontal scroll just to reach "delete repo".
        div class="settings-layout" {
            nav class="settings-sidebar" aria-label=(gitquarry_i18n::web::chrome::repo_settings_sidebar_nav_label(locale)) {
                ul {
                    @for (id, href, label) in &tabs {
                        li {
                            a class=[(active == *id).then_some("active")] aria-current=[(active == *id).then_some("page")] href=(href) { (label) }
                        }
                    }
                }
            }
            div class="settings-content" {
                (PreEscaped(render_flash(message, locale)))
                (content)
            }
        }
      }
    }
}
