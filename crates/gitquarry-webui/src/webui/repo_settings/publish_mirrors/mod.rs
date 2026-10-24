//! Publish mirrors — a private repo published, filtered, to a public remote
//! (`docs/publish-mirror-concept.md`): the entity's form, the dry-run
//! preview, "publish now" and the record of the last runs. No publish logic
//! lives here; the one action that pushes calls
//! `publish::run::publish_mirror_now`, which runs every gate first.

mod form;
mod report;

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link, settings_section};
use form::{mirror_from_form, render_form};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::uid::Uid;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::publish_mirrors as t;
use gitquarry_mirror_external::publish::log::{PublishLog, RunOutcome};
use gitquarry_mirror_external::publish::preview::Preview;
use gitquarry_mirror_external::publish::{self, PublishMirror};
use maud::{Markup, PreEscaped, html};
use report::{render_preview, render_runs};

/// One mirror as the section shows it: the entity, its etag (the edit
/// form's CAS guard) and its run log.
pub(crate) struct ListedPublishMirror {
    pub mirror: PublishMirror,
    pub etag: String,
    pub log: PublishLog,
}

/// Every publish mirror of the repo with its log: one small GET per mirror
/// and one per log, the same tradeoff as the external mirrors' listing.
pub(crate) async fn list_publish_mirrors(ctx: &AppContext, uid: &Uid) -> anyhow::Result<Vec<ListedPublishMirror>> {
    let mirrors = publish::list_with_etags(ctx, uid).await?;
    let mut listed = Vec::with_capacity(mirrors.len());
    for (mirror, etag) in mirrors {
        let log = publish::log::get(ctx, uid, &mirror.id).await?;
        listed.push(ListedPublishMirror { mirror, etag, log });
    }
    Ok(listed)
}

/// A preview just computed for one mirror, rendered once in the response to
/// the click that asked for it and never stored.
pub(crate) struct ShownPreview {
    pub mirror_id: String,
    pub preview: Preview,
}

/// Dispatches `add_publish_mirror`/`edit_publish_mirror`/
/// `delete_publish_mirror`/`preview_publish_mirror`/`publish_mirror_now`.
pub(super) async fn save_publish_mirror(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
    locale: Locale,
) -> Result<(FlashOutcome, Option<ShownPreview>), AppError> {
    let audit = |action: &'static str, detail: String| async move {
        gitquarry_core::storage::audit::record(
            ctx.backend().as_ref(),
            &ctx.cipher,
            actor,
            action,
            name,
            non_empty(detail),
        )
        .await;
    };
    match form.action.as_str() {
        "add_publish_mirror" => {
            let mirror = match mirror_from_form(PublishMirror::default(), form, locale) {
                Ok(mirror) => mirror,
                Err(msg) => return Ok((FlashOutcome::Invalid(msg), None)),
            };
            match publish::create(ctx, &resolved.uid, mirror).await {
                Ok(created) => {
                    audit("publish_mirror.add", created.label).await;
                    Ok((FlashOutcome::Ok(t::added(locale)), None))
                }
                Err(e) => Ok((FlashOutcome::Invalid(e.to_string()), None)),
            }
        }
        "edit_publish_mirror" => {
            let Some((existing, _current_etag)) = publish::get(ctx, &resolved.uid, &form.publish_mirror_id)
                .await
                .map_err(AppError::from)?
            else {
                return Err(AppError::not_found(anyhow::anyhow!("no such publish mirror")));
            };
            let mirror = match mirror_from_form(existing, form, locale) {
                Ok(mirror) => mirror,
                Err(msg) => return Ok((FlashOutcome::Invalid(msg), None)),
            };
            // The etag the browser loaded the form with, so an edit that
            // landed in between is a conflict rather than silently lost.
            match publish::update(ctx, &resolved.uid, &mirror, &form.publish_mirror_etag).await {
                Ok(Cas::Ok) => {
                    audit("publish_mirror.edit", mirror.label).await;
                    Ok((FlashOutcome::Ok(t::updated(locale)), None))
                }
                Ok(Cas::Conflict) => Ok((FlashOutcome::Conflict, None)),
                Err(e) => Ok((FlashOutcome::Invalid(e.to_string()), None)),
            }
        }
        "delete_publish_mirror" => {
            if publish::get(ctx, &resolved.uid, &form.publish_mirror_id)
                .await
                .map_err(AppError::from)?
                .is_none()
            {
                return Err(AppError::not_found(anyhow::anyhow!("no such publish mirror")));
            }
            publish::delete(ctx, &resolved.uid, &form.publish_mirror_id)
                .await
                .map_err(AppError::from)?;
            audit("publish_mirror.delete", form.publish_mirror_id.clone()).await;
            Ok((FlashOutcome::Ok(t::removed(locale)), None))
        }
        "publish_mirror_now" => {
            let Some((mirror, _etag)) = publish::get(ctx, &resolved.uid, &form.publish_mirror_id)
                .await
                .map_err(AppError::from)?
            else {
                return Err(AppError::not_found(anyhow::anyhow!("no such publish mirror")));
            };
            // Inline, like the external mirrors' "sync now": the click asked
            // for this run, so it waits for it and hears how it ended. The
            // error is already redacted and capped by the run.
            let run = match publish::run::publish_mirror_now(ctx, resolved, &mirror).await {
                Ok(run) => run,
                Err(e) => return Ok((FlashOutcome::Invalid(e.to_string()), None)),
            };
            audit("publish_mirror.publish", mirror.label).await;
            let message = match run.outcome {
                RunOutcome::Unchanged => t::published_unchanged(locale),
                _ => t::published(locale),
            };
            Ok((FlashOutcome::Ok(message), None))
        }
        _ => {
            // "preview_publish_mirror", the only other action routed here.
            let Some((mirror, _etag)) = publish::get(ctx, &resolved.uid, &form.publish_mirror_id)
                .await
                .map_err(AppError::from)?
            else {
                return Err(AppError::not_found(anyhow::anyhow!("no such publish mirror")));
            };
            let preview = publish::preview::preview(ctx, resolved, &mirror)
                .await
                .map_err(AppError::from)?;
            Ok((
                FlashOutcome::Ok(t::previewed(locale)),
                Some(ShownPreview {
                    mirror_id: mirror.id,
                    preview,
                }),
            ))
        }
    }
}

pub(super) fn render_publish_mirrors_section(
    repo_name: &str,
    mirrors: &[ListedPublishMirror],
    shown: Option<&ShownPreview>,
    retry: Option<&RepoConfForm>,
    csrf: &str,
    locale: Locale,
) -> Markup {
    let action_url = repo_link(repo_name, "settings", "").to_string();
    settings_section(
        &t::heading(locale),
        Some(&t::prose(locale)),
        html! {
            @if mirrors.is_empty() {
                p { em { (t::none_hint(locale)) } }
            }
            @for ListedPublishMirror { mirror, etag, log } in mirrors {
                div class="scroll-x" {
                    table class="list" {
                        tr {
                            th { (t::table_label(locale)) }
                            th { (t::table_remote(locale)) }
                            th { (t::table_refs(locale)) }
                            th { (t::table_status(locale)) }
                            th { (t::table_actions(locale)) }
                        }
                        tr {
                            td {
                                (mirror.label)
                                @if !mirror.enabled { " " span class="badge" { (t::disabled(locale)) } }
                            }
                            td { code { (mirror.remote_url) } }
                            td {
                                @for b in &mirror.publish_branches { code { "heads/" (b) } " " }
                                @for tag in &mirror.publish_tags { code { "tags/" (tag) } " " }
                            }
                            td {
                                @if let Some(error) = &log.last_error {
                                    span class="badge danger" title=(error) { (t::last_error(locale)) }
                                    " "
                                }
                                @if log.ref_status.is_empty() && log.last_error.is_none() {
                                    em { "—" }
                                }
                                @for (ref_name, status) in &log.ref_status {
                                    (super::mirrors::ref_status_pill(ref_name, status, locale))
                                }
                            }
                            td {
                                form method="post" action=(&action_url) class="inline-form" {
                                    (PreEscaped(csrf))
                                    input type="hidden" name="conf_action" value="preview_publish_mirror";
                                    input type="hidden" name="publish_mirror_id" value=(mirror.id);
                                    button type="submit" { (t::preview_button(locale)) }
                                }
                                " "
                                form method="post" action=(&action_url) class="inline-form" data-confirm=(t::publish_confirm(locale, &mirror.label)) {
                                    (PreEscaped(csrf))
                                    input type="hidden" name="conf_action" value="publish_mirror_now";
                                    input type="hidden" name="publish_mirror_id" value=(mirror.id);
                                    button type="submit" { (t::publish_button(locale)) }
                                }
                                " "
                                form method="post" action=(&action_url) class="inline-form" data-confirm=(t::delete_confirm(locale, &mirror.label)) {
                                    (PreEscaped(csrf))
                                    input type="hidden" name="conf_action" value="delete_publish_mirror";
                                    input type="hidden" name="publish_mirror_id" value=(mirror.id);
                                    button type="submit" { (t::delete_button(locale)) }
                                }
                            }
                        }
                    }
                }
                (render_runs(&log.runs, locale))
                @if let Some(shown) = shown.filter(|s| s.mirror_id == mirror.id) {
                    (render_preview(&mirror.label, &shown.preview, locale))
                }
                (render_form(
                    repo_name,
                    Some((mirror, etag.as_str())),
                    retry.filter(|f| f.action == "edit_publish_mirror" && f.publish_mirror_id == mirror.id),
                    csrf,
                    locale,
                ))
                hr;
            }
            h4 { (t::add_heading(locale)) }
            (render_form(repo_name, None, retry.filter(|f| f.action == "add_publish_mirror"), csrf, locale))
        },
    )
}
