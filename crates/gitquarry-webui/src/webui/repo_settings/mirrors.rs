//! External mirrors — push/pull sync against a non-gitquarry forge
//! (GitHub/GitLab/Codeberg), see `docs/external-mirror-concept.md`. Last
//! phase of Track A: everything below is a thin transport layer over
//! `mirror::external::domain`/`access` — no sync logic lives here. Track
//! B's own last phase (PR-sync status) lives here too: a `sync_pull_requests`
//! checkbox on the mirror form, and — for a mirror with it on — a small
//! table of its `ExternalPrLink`s.

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link};
use anyhow::Result;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::uid::Uid;
use gitquarry_mirror_external::access::{
    self, ExternalMirror, ExternalMirrorProvider, ExternalPrLink, RefSyncStatus, SyncDirection,
};
use gitquarry_mirror_external::domain;
use maud::{Markup, PreEscaped, html};
use std::collections::HashMap;

/// Every linked PR for every mirror in `mirrors` that has `sync_pull_requests`
/// on, keyed by mirror id — fetched once per page render/action and passed
/// down to [`render_mirrors_section`], rather than each mirror row fetching
/// its own (this can run inside a hot request path, same reasoning as
/// `access::list_external_mirrors`' own "small, curated, fine to just list"
/// tradeoff, extended one level to its PR links).
pub(crate) async fn fetch_pr_links_by_mirror(
    ctx: &AppContext,
    uid: &Uid,
    mirrors: &[(ExternalMirror, String)],
) -> Result<HashMap<String, Vec<ExternalPrLink>>> {
    let mut by_mirror = HashMap::new();
    for (mirror, _etag) in mirrors {
        if !mirror.sync_pull_requests {
            continue;
        }
        let links = access::list_pr_links(ctx.backend().as_ref(), &ctx.cipher, uid, &mirror.id).await?;
        by_mirror.insert(mirror.id.clone(), links);
    }
    Ok(by_mirror)
}

fn parse_provider(s: &str) -> ExternalMirrorProvider {
    match s {
        "gitlab" => ExternalMirrorProvider::GitLab,
        "codeberg" => ExternalMirrorProvider::Codeberg,
        "other" => ExternalMirrorProvider::Other,
        _ => ExternalMirrorProvider::GitHub,
    }
}

fn provider_label(p: ExternalMirrorProvider) -> &'static str {
    match p {
        ExternalMirrorProvider::GitHub => "GitHub",
        ExternalMirrorProvider::GitLab => "GitLab",
        ExternalMirrorProvider::Codeberg => "Codeberg",
        ExternalMirrorProvider::Other => "other",
    }
}

fn parse_direction(s: &str) -> SyncDirection {
    match s {
        "push_only" => SyncDirection::PushOnly,
        "pull_only" => SyncDirection::PullOnly,
        _ => SyncDirection::Bidirectional,
    }
}

fn direction_label(d: SyncDirection, locale: gitquarry_i18n::Locale) -> String {
    use gitquarry_i18n::web::repo_settings as t;
    match d {
        SyncDirection::PushOnly => t::direction_push_only_label(locale),
        SyncDirection::PullOnly => t::direction_pull_only_label(locale),
        SyncDirection::Bidirectional => t::direction_bidirectional_label(locale),
    }
}

/// Dispatches `add_mirror`/`edit_mirror`/`delete_mirror`/`sync_mirror_now` —
/// [`super::handler_repo_conf_post`]'s single entry point for this tab.
/// Returns the just-set `(mirror_id, token)` alongside the flash outcome
/// only for `add_mirror`/an `edit_mirror` that actually rotated the token —
/// see [`super::PageChrome::pending_mirror_token`] for why that's never
/// re-derived from storage on a later render.
pub(super) async fn save_mirror(
    ctx: &gitquarry_core::app::AppContext,
    resolved: &gitquarry_core::app::ResolvedRepo,
    name: &str,
    form: &RepoConfForm,
    actor: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(FlashOutcome, Option<(String, String)>), AppError> {
    match form.action.as_str() {
        "add_mirror" => {
            let mirror = ExternalMirror {
                remote_url: form.remote_url.trim().to_string(),
                token: form.token.trim().to_string(),
                provider: parse_provider(&form.provider),
                direction: parse_direction(&form.direction),
                enabled: form.mirror_enabled == "1",
                sync_pull_requests: form.sync_pull_requests == "1",
                label: form.label.trim().to_string(),
                ..Default::default()
            };
            let created = match domain::create(ctx, &resolved.uid, mirror).await {
                Ok(created) => created,
                Err(e) => return Ok((FlashOutcome::Invalid(e.to_string()), None)),
            };
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "mirror.add",
                name,
                non_empty(created.label.clone()),
            )
            .await;
            let pending = Some((created.id.clone(), created.token.clone()));
            Ok((FlashOutcome::Ok("mirror added."), pending))
        }
        "edit_mirror" => {
            // The CAS guard is `form.mirror_etag` — what the browser last
            // saw when it loaded this row — never the etag `get` just
            // fetched fresh (that would always match "current" and make
            // the CAS check a no-op, defeating the whole point: catching a
            // concurrent edit that landed between this page loading and
            // this submit).
            let Some((mut mirror, _current_etag)) = domain::get(ctx, &resolved.uid, &form.mirror_id)
                .await
                .map_err(AppError::from)?
            else {
                return Err(AppError::not_found(anyhow::anyhow!("no such mirror")));
            };
            mirror.remote_url = form.remote_url.trim().to_string();
            let rotated_token = !form.token.trim().is_empty();
            if rotated_token {
                mirror.token = form.token.trim().to_string();
            }
            mirror.provider = parse_provider(&form.provider);
            mirror.direction = parse_direction(&form.direction);
            mirror.enabled = form.mirror_enabled == "1";
            mirror.sync_pull_requests = form.sync_pull_requests == "1";
            mirror.label = form.label.trim().to_string();
            let cas = match domain::update(ctx, &resolved.uid, &mirror, &form.mirror_etag).await {
                Ok(cas) => cas,
                Err(e) => return Ok((FlashOutcome::Invalid(e.to_string()), None)),
            };
            Ok(match cas {
                Cas::Ok => {
                    gitquarry_core::storage::audit::record(
                        ctx.backend().as_ref(),
                        &ctx.cipher,
                        actor,
                        "mirror.edit",
                        name,
                        non_empty(mirror.label.clone()),
                    )
                    .await;
                    let pending = rotated_token.then(|| (mirror.id.clone(), mirror.token.clone()));
                    (FlashOutcome::Ok("mirror updated."), pending)
                }
                Cas::Conflict => (FlashOutcome::Conflict, None),
            })
        }
        "delete_mirror" => {
            domain::delete(ctx, &resolved.uid, &form.mirror_id)
                .await
                .map_err(AppError::from)?;
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                actor,
                "mirror.delete",
                name,
                non_empty(form.mirror_id.clone()),
            )
            .await;
            Ok((FlashOutcome::Ok("mirror removed."), None))
        }
        _ => {
            // "sync_mirror_now" — the only other action this dispatcher is
            // ever called with, see `handler_repo_conf_post`'s match arm.
            // Runs inline: a mirror count per repo is small and
            // operator-curated (see `access::list_external_mirrors`'s own
            // doc comment), so one request blocking for the round trip is
            // an accepted tradeoff for an explicit "do it now" click, same
            // as `domain::repo::fork` blocking its own request.
            //
            // Targets exactly the clicked mirror via `sync_one_mirror_now`,
            // never `sync_repo_mirrors` (every mirror on the repo, and —
            // the actual bug this replaced — always reports back
            // `Ok(())` regardless of what happened, since that function's
            // whole job is the periodic tick's "never abort, just log to
            // stderr" resilience; a click needs the opposite: whatever
            // really went wrong, surfaced in the flash, not silently
            // swallowed into a log line nobody watching the browser can see).
            let Some((mirror, _etag)) = domain::get(ctx, &resolved.uid, &form.mirror_id)
                .await
                .map_err(AppError::from)?
            else {
                return Err(AppError::not_found(anyhow::anyhow!("no such mirror")));
            };
            if let Err(e) = gitquarry_mirror_external::engine::sync_one_mirror_now(ctx, resolved, &mirror).await {
                return Ok((FlashOutcome::Invalid(e.to_string()), None));
            }
            let _ = locale;
            Ok((FlashOutcome::Ok("sync triggered."), None))
        }
    }
}

fn ref_status_pill(ref_name: &str, status: &RefSyncStatus, locale: gitquarry_i18n::Locale) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    let (text, title, class) = match status {
        RefSyncStatus::InSync => (t::ref_status_in_sync(locale), None, "badge ok"),
        RefSyncStatus::Diverged => (t::ref_status_diverged(locale), None, "badge warn"),
        RefSyncStatus::Error(msg) => (t::ref_status_error(locale), Some(msg.clone()), "badge danger"),
    };
    html! {
        span class=(class) title=[title] { code { (ref_name) } " " (text) }
        " "
    }
}

/// One mirror's fields, prefilled for an edit (`existing = Some(...)`) or
/// blank for the add form (`existing = None`) — same field set either way,
/// just a different `action`/hidden-id and the token placeholder switching
/// between "type one" and "leave blank to keep the current one" (`existing`
/// alone decides which, no separate `is_edit` flag needed).
fn render_mirror_form(
    repo_name: &str,
    existing: Option<(&ExternalMirror, &str)>,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    let action_url = repo_link(repo_name, "settings", "").to_string();
    let (action, mirror_id, etag, remote_url, provider, direction, label, enabled, sync_pull_requests) = match existing
    {
        Some((m, etag)) => (
            "edit_mirror",
            m.id.as_str(),
            etag,
            m.remote_url.as_str(),
            m.provider,
            m.direction,
            m.label.as_str(),
            m.enabled,
            m.sync_pull_requests,
        ),
        None => (
            "add_mirror",
            "",
            "",
            "",
            ExternalMirrorProvider::GitHub,
            SyncDirection::Bidirectional,
            "",
            true,
            false,
        ),
    };
    html! {
        form method="post" action=(&action_url) {
            (PreEscaped(csrf))
            input type="hidden" name="conf_action" value=(action);
            @if !mirror_id.is_empty() {
                input type="hidden" name="mirror_id" value=(mirror_id);
                input type="hidden" name="mirror_etag" value=(etag);
            }
            label {
                (t::mirror_field_label_label(locale)) " "
                input name="label" value=(label) placeholder=(t::mirror_label_placeholder(locale));
            }
            label {
                (t::mirror_field_label_url(locale)) " "
                input type="url" name="remote_url" value=(remote_url) required size="40" placeholder=(t::mirror_url_placeholder(locale));
            }
            label {
                (t::mirror_field_label_token(locale)) " "
                input type="password" name="token" size="24"
                    placeholder=(if mirror_id.is_empty() { t::mirror_token_placeholder_new(locale) } else { t::mirror_token_placeholder_unchanged(locale) });
            }
            select name="provider" {
                option value="github" selected[provider == ExternalMirrorProvider::GitHub] { "GitHub" }
                option value="gitlab" selected[provider == ExternalMirrorProvider::GitLab] { "GitLab" }
                option value="codeberg" selected[provider == ExternalMirrorProvider::Codeberg] { "Codeberg" }
                option value="other" selected[provider == ExternalMirrorProvider::Other] { "other" }
            }
            select name="direction" {
                option value="bidirectional" selected[direction == SyncDirection::Bidirectional] { (t::direction_bidirectional_option(locale)) }
                option value="push_only" selected[direction == SyncDirection::PushOnly] { (t::direction_push_only_option(locale)) }
                option value="pull_only" selected[direction == SyncDirection::PullOnly] { (t::direction_pull_only_option(locale)) }
            }
            label {
                input type="checkbox" name="mirror_enabled" value="1" checked[enabled];
                " " (t::mirror_field_label_enabled(locale))
            }
            label {
                input type="checkbox" name="sync_pull_requests" value="1" checked[sync_pull_requests];
                " " (t::mirror_field_label_sync_pull_requests(locale))
            }
            button type="submit" {
                (if mirror_id.is_empty() { t::add_mirror_button(locale) } else { t::save_mirror_button(locale) })
            }
        }
    }
}

/// One mirror's linked PRs — local number, external number, and the
/// last-synced cursor (`ExternalPrLink::remote_updated_cursor`), all a
/// settings-tab operator needs to sanity-check Track B's sync without
/// digging into storage directly. Nothing here is actionable (no
/// merge/close/comment buttons) — that's the pull-request pages themselves.
fn render_pr_links_table(links: &[ExternalPrLink], locale: gitquarry_i18n::Locale) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    html! {
        @if links.is_empty() {
            p class="muted" { em { (t::no_linked_prs_hint(locale)) } }
        } @else {
            div class="scroll-x" {
            table class="list" {
                tr {
                    th { (t::pr_link_table_header_local(locale)) }
                    th { (t::pr_link_table_header_external(locale)) }
                    th { (t::pr_link_table_header_cursor(locale)) }
                }
                @for link in links {
                    tr {
                        td { "#" (link.local_number) }
                        td { "#" (link.external_number) }
                        td { code { (link.remote_updated_cursor) } }
                    }
                }
            }
            }
        }
    }
}

pub fn render_mirrors_section(
    repo_name: &str,
    mirrors: &[(ExternalMirror, String)],
    pr_links: &HashMap<String, Vec<ExternalPrLink>>,
    csrf: &str,
    pending_token: Option<&(String, String)>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::repo_settings as t;
    let action_url = repo_link(repo_name, "settings", "").to_string();
    html! {
        div id="mirrors-section" {
        h3 { (t::mirrors_heading(locale)) }
        p class="muted" { (t::mirrors_prose(locale)) }
        @if let Some((mirror_id, token)) = pending_token {
            @let label = mirrors.iter().find(|(m, _)| &m.id == mirror_id).map_or("", |(m, _)| m.label.as_str());
            p class="flash flash-ok" {
                (t::mirror_token_shown_once_prefix(locale)) (label) (t::mirror_token_shown_once_suffix(locale))
            }
            p { code { (token) } }
        }
        @if mirrors.is_empty() {
            p { em { (t::no_mirrors_hint(locale)) } }
        } @else {
            div class="scroll-x" {
            table class="list" {
                tr {
                    th { (t::mirror_table_header_label(locale)) }
                    th { (t::mirror_table_header_provider(locale)) }
                    th { (t::mirror_table_header_direction(locale)) }
                    th { (t::mirror_table_header_status(locale)) }
                    th { (t::mirror_table_header_actions(locale)) }
                }
                @for (mirror, etag) in mirrors {
                    tr {
                        td { (mirror.label) }
                        td { (provider_label(mirror.provider)) }
                        td { (direction_label(mirror.direction, locale)) }
                        td {
                            // Before the per-ref pills: a whole-mirror
                            // failure means the per-ref state below is
                            // whatever the last successful sync left, not
                            // what this mirror looks like now.
                            @if let Some(error) = &mirror.last_error {
                                span class="badge danger" title=(error) { (t::mirror_last_error(locale)) }
                                " "
                            }
                            @if mirror.ref_status.is_empty() && mirror.last_error.is_none() {
                                em { "—" }
                            }
                            @for (ref_name, status) in &mirror.ref_status {
                                (ref_status_pill(ref_name, status, locale))
                            }
                        }
                        td {
                            form method="post" action=(&action_url) style="display:inline" data-async data-async-target="#mirrors-section" {
                                (PreEscaped(csrf))
                                input type="hidden" name="conf_action" value="sync_mirror_now";
                                input type="hidden" name="mirror_id" value=(mirror.id);
                                button type="submit" { (t::sync_now_button(locale)) }
                            }
                            " "
                            form method="post" action=(&action_url) style="display:inline" data-async data-async-target="#mirrors-section" data-confirm=(t::delete_mirror_confirm(locale)) {
                                (PreEscaped(csrf))
                                input type="hidden" name="conf_action" value="delete_mirror";
                                input type="hidden" name="mirror_id" value=(mirror.id);
                                button type="submit" { (t::delete_mirror_button(locale)) }
                            }
                        }
                    }
                    @if mirror.sync_pull_requests {
                        tr {
                            td colspan="5" {
                                strong { (t::linked_prs_heading(locale)) }
                                (render_pr_links_table(pr_links.get(&mirror.id).map_or(&[][..], Vec::as_slice), locale))
                            }
                        }
                    }
                    tr {
                        td colspan="5" { (render_mirror_form(repo_name, Some((mirror, etag.as_str())), csrf, locale)) }
                    }
                }
            }
            }
        }
        h4 { (t::add_mirror_heading(locale)) }
        (render_mirror_form(repo_name, None, csrf, locale))
        }
    }
}
