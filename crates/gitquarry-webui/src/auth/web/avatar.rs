//! `/_account/profile`: self-service avatar upload/remove — the "profile"
//! section of the account settings sidebar (`crate::webui::account_settings_page`).
//! Also owns the public image serve (`GET /_avatar/<username>`), unrelated
//! to the sidebar but keyed by username since it's a public URL embedded in
//! every `<img>` `crate::webui::identicon::account_avatar` renders anywhere
//! on the site, not just here.

use crate::webui::{
    AppError, State_, account_settings_page, csrf_field, effective_density, effective_locale, effective_theme,
    federation_ui_active, layout, require_csrf, require_login,
};
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::auth::entities::User;
use gitquarry_core::auth::{access, domain};
use gitquarry_core::validate;
use maud::{Markup, PreEscaped, html};

/// Disk-fill guard on the scratch buffer the multipart body streams into
/// before it's uploaded — an avatar is a small, fixed-purpose image, not an
/// arbitrary release binary, so this is far tighter than
/// `repo::releases::MAX_ASSET_UPLOAD_BYTES`.
const MAX_AVATAR_UPLOAD_BYTES: usize = 2 * 1024 * 1024;

pub(crate) async fn handler_account_profile_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let user = match require_login(&ctx, &headers, "/_account/profile").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let topnav = super::login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_avatar_section(&user, &csrf_field(&headers), locale);
    let body = account_settings_page("profile", content, federation_ui_active(&ctx), locale).into_string();
    Ok(Html(layout(
        "profile",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// The `/_account/profile` page's content — just the avatar, for now (the
/// account-settings equivalent of what used to be a section embedded on the
/// old combined preferences page).
fn render_avatar_section(user: &User, csrf: &str, locale: gitquarry_i18n::Locale) -> Markup {
    html! {
        h4 { (crate::auth::i18n::avatar::heading(locale)) }
        p class="muted" { (crate::auth::i18n::avatar::description(locale)) }
        p { (crate::webui::identicon::account_avatar(&user.username, user.avatar_content_type.as_deref(), 64)) }
        form method="post" action="/_account/profile" enctype="multipart/form-data" {
            (PreEscaped(csrf))
            input type="file" name="avatar" accept="image/png,image/jpeg,image/webp" required;
            button type="submit" { (crate::auth::i18n::avatar::upload_button(locale)) }
        }
        @if user.avatar_content_type.is_some() {
            form method="post" action="/_account/profile/delete" {
                (PreEscaped(csrf))
                button type="submit" { (crate::auth::i18n::avatar::remove_button(locale)) }
            }
        }
    }
}

pub(crate) async fn handler_avatar_upload_post(
    State(ctx): State_,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    // Before a byte of the body is read: an anonymous caller must not be
    // able to make this process buffer `MAX_AVATAR_UPLOAD_BYTES` (or stream
    // an unbounded number of ignored fields past it) just by POSTing here.
    // The session lives in the request headers, so this guard needs nothing
    // from the body — unlike `require_csrf` below, whose token is itself a
    // multipart field and so can only be checked once the body is parsed.
    let me = match require_login(&ctx, &headers, "/_account/profile").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let mut csrf = String::new();
    let mut uploaded: Option<Vec<u8>> = None;

    while let Some(mut field) = multipart.next_field().await.map_err(|e| {
        AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::invalid_multipart(locale, e)
        ))
    })? {
        match field.name() {
            Some("csrf") => {
                csrf = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad_request(anyhow::anyhow!("{e}")))?;
            }
            Some("avatar") => {
                let mut buf = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(|e| {
                    AppError::bad_request(anyhow::anyhow!("{}", gitquarry_i18n::shared::reading_file(locale, e)))
                })? {
                    if buf.len() + chunk.len() > MAX_AVATAR_UPLOAD_BYTES {
                        return Err(AppError::bad_request(anyhow::anyhow!(
                            "{}",
                            gitquarry_i18n::shared::asset_too_large(locale, MAX_AVATAR_UPLOAD_BYTES as u64)
                        )));
                    }
                    buf.extend_from_slice(&chunk);
                }
                uploaded = Some(buf);
            }
            _ => {}
        }
    }

    require_csrf(&headers, &csrf)?;

    let buf = uploaded.ok_or_else(|| {
        AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::missing_file_to_upload(locale)
        ))
    })?;
    let content_type = validate::sniff_avatar_content_type(&buf).map_err(AppError::bad_request)?;

    let tmp = tempfile::Builder::new()
        .prefix(".gitquarry-avatar-")
        .tempfile()
        .map_err(|e| {
            AppError::bad_request(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::creating_temp_file(locale, e)
            ))
        })?
        .into_temp_path();
    tokio::fs::write(&tmp, &buf).await?;

    domain::user::set_avatar(&ctx, &me.username, content_type, &tmp, locale)
        .await
        .map_err(AppError::bad_request)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "avatar.set",
        &me.username,
        None,
    )
    .await;
    Ok(Redirect::to("/_account/profile").into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct AvatarDeleteForm {
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_avatar_delete_post(
    State(ctx): State_,
    headers: HeaderMap,
    axum::extract::Form(form): axum::extract::Form<AvatarDeleteForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/profile").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;

    domain::user::remove_avatar(&ctx, &me.username, locale).await?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "avatar.remove",
        &me.username,
        None,
    )
    .await;
    Ok(Redirect::to("/_account/profile").into_response())
}

/// `GET /_avatar/<username>` — public, no login gate, same as the identicon
/// it replaces: this is what every `<img>` [`crate::webui::identicon::account_avatar`]
/// renders actually fetches.
pub(crate) async fn handler_avatar_get(State(ctx): State_, Path(username): Path<String>) -> Result<Response, AppError> {
    let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &username)
        .await?
        .0
    else {
        return Err(AppError::not_found(anyhow::anyhow!("no such user")));
    };
    let Some(content_type) = user.avatar_content_type else {
        return Err(AppError::not_found(anyhow::anyhow!("{username} has no avatar")));
    };

    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().join("avatar");
    ctx.avatar_remote().download_avatar(&username, &tmp_path).await?;
    let bytes = tokio::fs::read(&tmp_path).await?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "private, max-age=300".to_string()),
        ],
        bytes,
    )
        .into_response())
}
