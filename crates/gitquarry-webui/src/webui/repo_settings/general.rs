//! Description/visibility/archived — `<repo>/.conf/config.json`'s own
//! fields, minus `owner` (always re-derived from the repo's namespace, see
//! [`save_conf`]'s doc comment).

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, Flash, non_empty, render_flash, repo_link};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::{RepoConfig, Visibility};
use maud::{Markup, PreEscaped, html};

/// `owner` is always re-derived from `name`'s own namespace segment, never
/// taken from the form — a repo living at `<namespace>/<local>` belongs to
/// whoever/whatever `<namespace>` names, full stop. This used to accept an
/// independent `owner` field (a same-cost "relabel without moving" — no
/// pack copying, unlike an actual move), but that let `RepoConfig.owner`
/// disagree with the repo's real namespace: `access::effective_permission`'s
/// namespace shortcut would still grant the namespace owner implicit
/// `Owner`-tier access regardless, while [`crate::auth::web::profile`]'s "this
/// user's repos" listing (which reads `owner` back) would silently show
/// the repo under someone else instead. Actually changing who a repo
/// belongs to means moving it to a new namespace — see
/// [`super::move_repo::move_repo`], which already keeps `owner` and the
/// namespace in lockstep. Recomputing it here on every save also self-heals
/// any repo a stale build of this form already mismatched.
pub(super) async fn save_conf(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
) -> Result<FlashOutcome, AppError> {
    let namespace = name.split_once('/').map(|(ns, _)| ns).unwrap_or(name);
    let new_owner = domain::repo::resolve_owner_ref(ctx, namespace).await?;
    let visibility = match form.visibility.as_str() {
        "limited" => Visibility::Limited,
        "private" => Visibility::Private,
        _ => Visibility::Public,
    };
    let archived = form.archived == "1";
    let is_template = form.is_template == "1";
    let patch = domain::repo::ConfPatch {
        description: non_empty(form.description.clone()),
        owner: new_owner,
        visibility,
        archived,
        is_template,
    };
    Ok(match domain::repo::set_conf(ctx, resolved, patch).await? {
        Cas::Ok => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "repo.set_conf",
                name,
                Some(format!("archived={archived} is_template={is_template}")),
            )
            .await;
            FlashOutcome::Ok("saved to the bucket.")
        }
        Cas::Conflict => FlashOutcome::Conflict,
    })
}

pub fn render_repo_conf_form(
    repo_name: &str,
    config: &RepoConfig,
    current_owner: &str,
    csrf: &str,
    message: Option<Flash>,
    retry: Option<&RepoConfForm>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    // `retry` only ever shows up on a CAS conflict here (there's no other
    // way this specific form gets rejected) — falls back to `config` on a
    // plain load, same as before.
    let description = retry.map_or(config.description.as_deref().unwrap_or(""), |f| f.description.as_str());
    let visibility = retry.map_or(config.visibility, |f| match f.visibility.as_str() {
        "limited" => Visibility::Limited,
        "private" => Visibility::Private,
        _ => Visibility::Public,
    });
    let archived = retry.map_or(config.archived, |f| f.archived == "1");
    let is_template = retry.map_or(config.is_template, |f| f.is_template == "1");
    html! {
        h3 { (t::repo_conf_heading(locale)) }
        p class="muted" {
            (t::repo_conf_prose_prefix(locale)) code { (repo_name) "/.conf/config.json" } (t::repo_conf_prose_suffix(locale))
        }
        (PreEscaped(render_flash(message, locale)))
        form method="post" action=(repo_link(repo_name, "settings", "").to_string()) data-async data-async-target="#repo-settings-body" data-confirm=(t::save_general_confirm(locale)) {
            (PreEscaped(csrf))
            div class="field" {
                label { (t::field_label_description(locale)) }
                input name="description" value=(description) size="60";
            }
            div class="field" {
                label { (t::field_label_owner(locale)) }
                @if current_owner.is_empty() { span class="muted" { (t::owner_option_none(locale)) } } @else { (current_owner) }
                " " span class="muted" { (t::owner_derived_from_namespace_hint(locale)) }
            }
            div class="field" {
                label { (t::field_label_visibility(locale)) }
                select name="visibility" {
                    option value="public" selected[visibility == Visibility::Public] { (t::visibility_public_option(locale)) }
                    option value="limited" selected[visibility == Visibility::Limited] { (t::visibility_limited_option(locale)) }
                    option value="private" selected[visibility == Visibility::Private] { (t::visibility_private_option(locale)) }
                }
            }
            div class="field" {
                label {
                    input type="checkbox" name="archived" value="1" checked[archived];
                    " " (t::archived_checkbox_hint(locale))
                }
            }
            div class="field" {
                label {
                    input type="checkbox" name="is_template" value="1" checked[is_template];
                    " " (t::is_template_checkbox_hint(locale))
                }
            }
            p { button type="submit" { (t::save_button(locale)) } }
        }
    }
}
