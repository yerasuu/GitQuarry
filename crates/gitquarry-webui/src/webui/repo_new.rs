//! `/_new`: creates an empty repo — an empty `refs.json` manifest plus an
//! initial `.conf/config.json` — so `git push` afterward fills in refs the
//! normal way, same as pushing to a repo that doesn't exist yet already
//! does implicitly. The repo's folder (and `owner`) is either the creator's
//! own username or an existing [`gitquarry_core::auth::entities::Group`] — never a
//! free-text namespace: [`crate::auth::web::profile`] reads `owner` back to list "this
//! user/group's repos", so it has to name something real. Admin-gated like
//! `/_settings`/`/_users`: creating a repo is an operator action, not
//! something every ssh:// user with write on some other repo gets to do —
//! any admin may target any group, not just ones they're a member of, same
//! as `is_admin` already bypasses per-repo grants everywhere else.

use super::{
    AppError, Flash, State_, csrf_field, effective_density, effective_locale, effective_theme, layout, non_empty,
    render_flash, require_csrf, url_encode,
};
use crate::auth::web::login;
use crate::settings::web::require_admin;
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::auth::access;
use gitquarry_core::domain;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::validate;
use maud::{Markup, PreEscaped, html};

pub(crate) async fn handler_new_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_new").await {
        return Ok(resp);
    }
    let Some(me) = login::current_user(&ctx, &headers).await else {
        return Ok(no_session_page(&ctx, &headers).await);
    };
    let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let body = render_new_form(&me.username, &group_names, &csrf_field(&headers), None, locale).into_string();
    Ok(axum::response::Html(layout(
        "new repo",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct NewRepoForm {
    /// The creator's own username, or an existing group name — validated
    /// against both lists in [`handler_new_post`], never trusted as-is:
    /// it's a `<select>` in the rendered form, but nothing stops a crafted
    /// POST from naming an arbitrary namespace otherwise.
    namespace: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    visibility: String,
    /// Full name of an existing [`gitquarry_core::storage::entities::RepoConfig::is_template`]
    /// repo to copy from instead of creating empty — blank means the plain
    /// empty-repo path. When set, `description`/`visibility` above are
    /// ignored: the new repo inherits both from the template, same as
    /// [`domain::repo::fork`] doesn't let the forker override them either.
    #[serde(default)]
    template: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_new_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<NewRepoForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_new").await {
        return Ok(resp);
    }
    // The repo's folder is always the creator's own username or an
    // existing group, so there has to be a logged-in identity — the one
    // case `require_admin` still lets through with no session at all (no
    // login configured whatsoever) can't name either.
    let Some(me) = login::current_user(&ctx, &headers).await else {
        return Ok(no_session_page(&ctx, &headers).await);
    };
    require_csrf(&headers, &form.csrf)?;

    let namespace = form.namespace.trim();
    let Some(owner) = domain::repo::resolve_owner_ref(&ctx, namespace).await? else {
        let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
        return Ok(error_page(&ctx, &headers, &me.username, &group_names, "invalid namespace.").await);
    };

    let display_name = match validate::validate_repo_display_name(&form.name) {
        Ok(n) => n,
        Err(e) => {
            let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
            return Ok(error_page(&ctx, &headers, &me.username, &group_names, &e.to_string()).await);
        }
    };
    let Some(slug) = domain::repo::slugify_repo_name(&display_name) else {
        let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
        return Ok(error_page(
            &ctx,
            &headers,
            &me.username,
            &group_names,
            "name must contain at least one letter or digit.",
        )
        .await);
    };
    let name = match validate::sanitize_path(&format!("{namespace}/{slug}")) {
        Ok(name) => name,
        Err(e) => {
            let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
            return Ok(error_page(&ctx, &headers, &me.username, &group_names, &e.to_string()).await);
        }
    };
    let template = form.template.trim();
    let created = if template.is_empty() {
        let visibility = match form.visibility.as_str() {
            "limited" => Visibility::Limited,
            "private" => Visibility::Private,
            _ => Visibility::Public,
        };
        domain::repo::create(
            &ctx,
            &name,
            Some(display_name),
            Some(owner),
            non_empty(form.description),
            visibility,
        )
        .await?
    } else {
        let locale = effective_locale(&ctx, &headers).await;
        let Some(source) = ctx.resolve_repo(template).await? else {
            let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
            return Ok(error_page(&ctx, &headers, &me.username, &group_names, "template repo not found.").await);
        };
        match domain::repo::create_from_template(&ctx, &source, &name, Some(display_name), owner, locale).await {
            Ok(created) => created,
            Err(e) => {
                let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
                return Ok(error_page(&ctx, &headers, &me.username, &group_names, &e.to_string()).await);
            }
        }
    };
    let Some(_resolved) = created else {
        let group_names = access::list_group_names(ctx.backend().as_ref()).await?;
        return Ok(error_page(
            &ctx,
            &headers,
            &me.username,
            &group_names,
            "a repo with that name already exists.",
        )
        .await);
    };

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "repo.create",
        &name,
        None,
    )
    .await;

    Ok(Redirect::to(&format!("/{}", url_encode(&name))).into_response())
}

async fn no_session_page(ctx: &gitquarry_core::app::AppContext, headers: &HeaderMap) -> Response {
    let topnav = login::topnav(ctx, headers).await;
    let locale = effective_locale(ctx, headers).await;
    let message = gitquarry_i18n::web::repo_new::repo_new_no_session_error(locale);
    axum::response::Html(layout(
        "new repo",
        "",
        &render_flash(Some(Flash::Err(&message)), locale),
        &topnav,
        effective_theme(ctx, headers).await,
        effective_density(ctx, headers).await,
        locale,
    ))
    .into_response()
}

async fn error_page(
    ctx: &gitquarry_core::app::AppContext,
    headers: &HeaderMap,
    username: &str,
    group_names: &[String],
    message: &str,
) -> Response {
    let locale = effective_locale(ctx, headers).await;
    let content = render_new_form(
        username,
        group_names,
        &csrf_field(headers),
        Some(Flash::Err(message)),
        locale,
    );
    // Async (`data-async-target="#new-repo-body"`, see `crate::webui::ASYNC_HEADER`'s
    // own doc comment): a rejected name/namespace re-renders in place, no
    // real page navigation for an F5 to later warn about resubmitting.
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        return axum::response::Html(content.into_string()).into_response();
    }
    let topnav = login::topnav(ctx, headers).await;
    let body = content.into_string();
    axum::response::Html(layout(
        "new repo",
        "",
        &body,
        &topnav,
        effective_theme(ctx, headers).await,
        effective_density(ctx, headers).await,
        locale,
    ))
    .into_response()
}

#[cfg(any(test, feature = "test-util"))]
pub fn render_new_form(
    username: &str,
    group_names: &[String],
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_new::*;
    html! {
      div id="new-repo-body" {
        h3 { (repo_new_heading(locale)) }
        p class="muted" {
            (repo_new_prose_creates_empty(locale)) code { "refs.json" } (repo_new_prose_in_bucket_under(locale))
            code { "namespace/name" } (repo_new_prose_your_own_folder(locale))
            a href=(format!("/_profile/{username}")) { (repo_new_your_profile_link(locale)) }
            (repo_new_prose_or_existing_group(locale)) a href="/_groups" { "/_groups" } (repo_new_prose_close_paren(locale))
            code { "git push" } (repo_new_prose_push_fills_refs(locale))
            (repo_new_prose_avoids_blind_push(locale))
            code { "owner" } (repo_new_prose_owner_note(locale))
        }
        (PreEscaped(render_flash(message, locale)))
        form method="post" action="/_new" data-async data-async-target="#new-repo-body" {
            (PreEscaped(csrf))
            div class="field" {
                label { (repo_new_namespace_label(locale)) }
                select name="namespace" {
                    option value=(username) { (username) (repo_new_me_suffix(locale)) }
                    @for group_name in group_names {
                        option value=(group_name) { (group_name) }
                    }
                }
            }
            div class="field" {
                label { (repo_new_name_label(locale)) }
                input name="name" size="40" placeholder=(repo_new_name_placeholder(locale)) required autofocus;
                span class="hint" { (repo_new_name_hint(locale)) }
            }
            div class="field" {
                label { (repo_new_description_label(locale)) }
                input name="description" size="60";
            }
            div class="field" {
                label { (repo_new_visibility_label(locale)) }
                select name="visibility" {
                    option value="public" { (repo_new_visibility_public(locale)) }
                    option value="limited" { (repo_new_visibility_limited(locale)) }
                    option value="private" { (repo_new_visibility_private(locale)) }
                }
            }
            div class="field" {
                label { (repo_new_template_label(locale)) }
                input name="template" size="40" placeholder=(repo_new_template_placeholder(locale));
            }
            p { button type="submit" { (repo_new_create_button(locale)) } }
        }
      }
    }
}
#[cfg(not(any(test, feature = "test-util")))]
fn render_new_form(
    username: &str,
    group_names: &[String],
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_new::*;
    html! {
      div id="new-repo-body" {
        h3 { (repo_new_heading(locale)) }
        p class="muted" {
            (repo_new_prose_creates_empty(locale)) code { "refs.json" } (repo_new_prose_in_bucket_under(locale))
            code { "namespace/name" } (repo_new_prose_your_own_folder(locale))
            a href=(format!("/_profile/{username}")) { (repo_new_your_profile_link(locale)) }
            (repo_new_prose_or_existing_group(locale)) a href="/_groups" { "/_groups" } (repo_new_prose_close_paren(locale))
            code { "git push" } (repo_new_prose_push_fills_refs(locale))
            (repo_new_prose_avoids_blind_push(locale))
            code { "owner" } (repo_new_prose_owner_note(locale))
        }
        (PreEscaped(render_flash(message, locale)))
        form method="post" action="/_new" data-async data-async-target="#new-repo-body" {
            (PreEscaped(csrf))
            div class="field" {
                label { (repo_new_namespace_label(locale)) }
                select name="namespace" {
                    option value=(username) { (username) (repo_new_me_suffix(locale)) }
                    @for group_name in group_names {
                        option value=(group_name) { (group_name) }
                    }
                }
            }
            div class="field" {
                label { (repo_new_name_label(locale)) }
                input name="name" size="40" placeholder=(repo_new_name_placeholder(locale)) required autofocus;
                span class="hint" { (repo_new_name_hint(locale)) }
            }
            div class="field" {
                label { (repo_new_description_label(locale)) }
                input name="description" size="60";
            }
            div class="field" {
                label { (repo_new_visibility_label(locale)) }
                select name="visibility" {
                    option value="public" { (repo_new_visibility_public(locale)) }
                    option value="limited" { (repo_new_visibility_limited(locale)) }
                    option value="private" { (repo_new_visibility_private(locale)) }
                }
            }
            div class="field" {
                label { (repo_new_template_label(locale)) }
                input name="template" size="40" placeholder=(repo_new_template_placeholder(locale));
            }
            p { button type="submit" { (repo_new_create_button(locale)) } }
        }
      }
    }
}
