//! Publish mirrors — a private repo published, filtered, to a public remote
//! (`docs/publish-mirror-concept.md`). Phase 0: the entity's form and the
//! dry-run preview. There is no action here that pushes, and none that
//! could be added without a publish engine that doesn't exist yet: the only
//! thing the preview action calls is `publish::preview::preview`, which
//! reads the local mirror and computes object ids in memory.

use super::{FlashOutcome, RepoConfForm};
use crate::webui::{AppError, non_empty, repo_link, settings_section};
use gitquarry_core::storage::Cas;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::publish_mirrors as t;
use gitquarry_mirror_external::publish::audit::{Finding, Gate, GateReport};
use gitquarry_mirror_external::publish::filter::ExcludedBy;
use gitquarry_mirror_external::publish::preview::{Preview, RefPreview};
use gitquarry_mirror_external::publish::rewrite::IdentRole;
use gitquarry_mirror_external::publish::{
    self, FilterConfig, FilterMode, IdentityMapping, PublishMirror, SecretAllow, UnmappedPolicy,
};
use maud::{Markup, PreEscaped, html};

/// A preview just computed for one mirror, rendered once in the response to
/// the click that asked for it and never stored.
pub(crate) struct ShownPreview {
    pub mirror_id: String,
    pub preview: Preview,
}

/// Dispatches `add_publish_mirror`/`edit_publish_mirror`/
/// `delete_publish_mirror`/`preview_publish_mirror`.
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

/// Applies the form's fields over `mirror` (a stored one on edit, so its id
/// survives). `Err` carries a message for the flash; everything that reaches
/// storage is validated again by `publish::create`/`update`.
fn mirror_from_form(mut mirror: PublishMirror, form: &RepoConfForm, locale: Locale) -> Result<PublishMirror, String> {
    mirror.label = form.label.trim().to_string();
    mirror.remote_url = form.remote_url.trim().to_string();
    mirror.filter = FilterConfig {
        mode: match form.publish_mode.as_str() {
            "exclude" => FilterMode::Exclude,
            _ => FilterMode::Allowlist,
        },
        include: lines(&form.publish_include).map(str::to_string).collect(),
        exclude: lines(&form.publish_exclude).map(str::to_string).collect(),
        exclude_submodules: form.publish_exclude_submodules == "1",
        strip_empty_dirs: form.publish_strip_empty_dirs == "1",
    };
    mirror.publish_branches = gitquarry_core::domain::repo::parse_csv_field(&form.publish_branches);
    mirror.publish_tags = gitquarry_core::domain::repo::parse_csv_field(&form.publish_tags);
    mirror.identities = lines(&form.publish_identities)
        .map(|line| {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(private), Some(public)) => {
                    let name = parts.collect::<Vec<_>>().join(" ");
                    Ok(IdentityMapping {
                        private: private.to_string(),
                        public: public.to_string(),
                        name: (!name.is_empty()).then_some(name),
                    })
                }
                _ => Err(t::bad_identity_line(locale, line)),
            }
        })
        .collect::<Result<_, _>>()?;
    mirror.unmapped = match form.publish_unmapped.as_str() {
        "passthrough" => UnmappedPolicy::Passthrough,
        _ => UnmappedPolicy::Block,
    };
    let spdx = form.publish_spdx.trim();
    mirror.spdx = (!spdx.is_empty()).then(|| spdx.to_string());
    mirror.secret_allowlist = lines(&form.publish_secret_allowlist)
        .map(|line| match line.split_once(char::is_whitespace) {
            Some((sha, path)) => SecretAllow {
                path: path.trim().to_string(),
                sha: sha.to_string(),
            },
            None => SecretAllow {
                path: String::new(),
                sha: line.to_string(),
            },
        })
        .collect();
    Ok(mirror)
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty())
}

fn render_form(repo_name: &str, existing: Option<(&PublishMirror, &str)>, csrf: &str, locale: Locale) -> Markup {
    let action_url = repo_link(repo_name, "settings", "").to_string();
    let default = PublishMirror::default();
    let (mirror, etag) = existing.unwrap_or((&default, ""));
    let join_lines = |items: &[String]| items.join("\n");
    let identities = mirror
        .identities
        .iter()
        .map(|i| match &i.name {
            Some(name) => format!("{} {} {name}", i.private, i.public),
            None => format!("{} {}", i.private, i.public),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let allowlist = mirror
        .secret_allowlist
        .iter()
        .map(|a| format!("{} {}", a.sha, a.path).trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let csv = |set: &std::collections::BTreeSet<String>| set.iter().cloned().collect::<Vec<_>>().join(", ");
    html! {
        form method="post" action=(&action_url) {
            (PreEscaped(csrf))
            @if existing.is_some() {
                input type="hidden" name="conf_action" value="edit_publish_mirror";
                input type="hidden" name="publish_mirror_id" value=(mirror.id);
                input type="hidden" name="publish_mirror_etag" value=(etag);
            } @else {
                input type="hidden" name="conf_action" value="add_publish_mirror";
            }
            label { (t::field_label(locale)) " " input name="label" value=(mirror.label); }
            label {
                (t::field_remote_url(locale)) " "
                input type="url" name="remote_url" value=(mirror.remote_url) required size="40";
            }
            label {
                (t::field_mode(locale)) " "
                select name="publish_mode" {
                    option value="allowlist" selected[mirror.filter.mode == FilterMode::Allowlist] { (t::mode_allowlist(locale)) }
                    option value="exclude" selected[mirror.filter.mode == FilterMode::Exclude] { (t::mode_exclude(locale)) }
                }
            }
            label { (t::field_include(locale)) br; textarea name="publish_include" rows="4" cols="40" { (join_lines(&mirror.filter.include)) } }
            label { (t::field_exclude(locale)) br; textarea name="publish_exclude" rows="4" cols="40" { (join_lines(&mirror.filter.exclude)) } }
            label {
                input type="checkbox" name="publish_exclude_submodules" value="1" checked[mirror.filter.exclude_submodules];
                " " (t::field_exclude_submodules(locale))
            }
            label {
                input type="checkbox" name="publish_strip_empty_dirs" value="1" checked[mirror.filter.strip_empty_dirs];
                " " (t::field_strip_empty_dirs(locale))
            }
            label { (t::field_branches(locale)) " " input name="publish_branches" value=(csv(&mirror.publish_branches)) size="30"; }
            label { (t::field_tags(locale)) " " input name="publish_tags" value=(csv(&mirror.publish_tags)) size="30"; }
            label { (t::field_identities(locale)) br; textarea name="publish_identities" rows="3" cols="60" { (identities) } }
            label {
                (t::field_unmapped(locale)) " "
                select name="publish_unmapped" {
                    option value="block" selected[mirror.unmapped == UnmappedPolicy::Block] { (t::unmapped_block(locale)) }
                    option value="passthrough" selected[mirror.unmapped == UnmappedPolicy::Passthrough] { (t::unmapped_passthrough(locale)) }
                }
            }
            label { (t::field_spdx(locale)) " " input name="publish_spdx" value=(mirror.spdx.as_deref().unwrap_or("")) size="20"; }
            label { (t::field_secret_allowlist(locale)) br; textarea name="publish_secret_allowlist" rows="3" cols="60" { (allowlist) } }
            button type="submit" { (if existing.is_some() { t::save_button(locale) } else { t::add_button(locale) }) }
        }
    }
}

fn rule_label(rule: &ExcludedBy, locale: Locale) -> String {
    match rule {
        ExcludedBy::Config(pattern) => t::rule_config(locale, pattern),
        ExcludedBy::RepoFile(pattern) => t::rule_repo_file(locale, pattern),
        ExcludedBy::FilterFile => t::rule_filter_file(locale),
        ExcludedBy::Submodule => t::rule_submodule(locale),
        ExcludedBy::NotInAllowlist => t::rule_not_in_allowlist(locale),
    }
}

fn gate_label(gate: Gate, locale: Locale) -> String {
    match gate {
        Gate::Filter => t::gate_filter(locale),
        Gate::BlobSet => t::gate_blob_set(locale),
        Gate::License => t::gate_license(locale),
        Gate::Secrets => t::gate_secrets(locale),
        Gate::Identity => t::gate_identity(locale),
        Gate::Host => t::gate_host(locale),
    }
}

fn finding_text(finding: &Finding, locale: Locale) -> String {
    match finding {
        Finding::RepoFilterInvalid { error } => t::finding_repo_filter_invalid(locale, error),
        Finding::ExcludedPathPublished { path, rule } => {
            t::finding_excluded_path_published(locale, path, &rule_label(rule, locale))
        }
        Finding::BlobAlsoExcluded {
            sha,
            published_path,
            excluded_path,
        } => t::finding_blob_also_excluded(locale, sha, published_path, excluded_path),
        Finding::LicenseMissing => t::finding_license_missing(locale),
        Finding::SpdxNotDeclared { expected } => t::finding_spdx_not_declared(locale, expected),
        Finding::SpdxMismatch { expected, found } => t::finding_spdx_mismatch(locale, expected, found),
        Finding::Secret { path, sha, rule, line } if path.is_empty() => {
            t::finding_secret_in_message(locale, sha, rule, *line)
        }
        Finding::Secret { path, sha, rule, line } => t::finding_secret(locale, path, sha, rule, *line),
        Finding::SecretScanSkipped { path, sha } => t::finding_secret_scan_skipped(locale, path, sha),
        Finding::UnmappedIdentity {
            email,
            role: IdentRole::Author,
            commits,
            example_commit,
        } => t::finding_unmapped_author(locale, email, *commits, example_commit),
        Finding::UnmappedIdentity {
            email,
            role: IdentRole::Committer,
            commits,
            example_commit,
        } => t::finding_unmapped_committer(locale, email, *commits, example_commit),
        Finding::HostNotAllowed { error } => t::finding_host_not_allowed(locale, error),
    }
}

fn render_gate(report: &GateReport, locale: Locale) -> Markup {
    html! {
        tr {
            td { (gate_label(report.gate, locale)) }
            td {
                @if report.passed() {
                    span class="badge ok" { (t::gate_pass(locale)) }
                } @else {
                    span class="badge danger" { (t::gate_blocked(locale)) }
                    ul {
                        @for finding in &report.findings {
                            li { (finding_text(finding, locale)) }
                        }
                    }
                }
            }
        }
    }
}

fn render_ref(r: &RefPreview, locale: Locale) -> Markup {
    html! {
        h5 { code { (r.ref_name) } }
        @if let Some(error) = &r.error {
            p class="flash flash-err" { (t::ref_error(locale)) ": " (error) }
        } @else {
            dl {
                dt { (t::private_head(locale)) } dd { code { (r.private_head) } }
                dt { (t::public_head(locale)) }
                dd {
                    @match &r.public_head {
                        Some(sha) => code { (sha) },
                        None => em { (t::public_head_none(locale)) },
                    }
                }
                dt { (t::published_commits(locale)) } dd { (r.published_commits) }
                dt { (t::collapsed_commits(locale)) } dd { (r.collapsed_commits) }
            }
            div class="scroll-x" {
                table class="list" {
                    @for gate in &r.gates { (render_gate(gate, locale)) }
                }
            }
            strong { (t::excluded_heading(locale)) }
            @if r.excluded.is_empty() {
                p class="muted" { em { (t::excluded_none(locale)) } }
            } @else {
                ul {
                    @for (rule, count) in &r.excluded {
                        li { (rule_label(rule, locale)) ": " (count) }
                    }
                }
            }
            details {
                summary { (t::published_paths(locale, r.published_paths.len())) }
                ul {
                    @for path in &r.published_paths { li { code { (path) } } }
                }
            }
        }
    }
}

fn render_preview(label: &str, preview: &Preview, locale: Locale) -> Markup {
    html! {
        div class="publish-preview" {
            h4 { (t::preview_heading(locale, label)) }
            @if preview.passed() {
                p class="flash flash-ok" { (t::preview_verdict_pass(locale)) }
            } @else {
                p class="flash flash-err" { (t::preview_verdict_blocked(locale)) }
            }
            div class="scroll-x" {
                table class="list" { (render_gate(&preview.host, locale)) }
            }
            @if preview.refs.is_empty() {
                p { em { (t::preview_no_refs(locale)) } }
            }
            @for r in &preview.refs { (render_ref(r, locale)) }
        }
    }
}

pub(super) fn render_publish_mirrors_section(
    repo_name: &str,
    mirrors: &[(PublishMirror, String)],
    shown: Option<&ShownPreview>,
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
            @for (mirror, etag) in mirrors {
                div class="scroll-x" {
                    table class="list" {
                        tr {
                            th { (t::table_label(locale)) }
                            th { (t::table_remote(locale)) }
                            th { (t::table_refs(locale)) }
                            th { (t::table_actions(locale)) }
                        }
                        tr {
                            td { (mirror.label) }
                            td { code { (mirror.remote_url) } }
                            td {
                                @for b in &mirror.publish_branches { code { "heads/" (b) } " " }
                                @for tag in &mirror.publish_tags { code { "tags/" (tag) } " " }
                            }
                            td {
                                form method="post" action=(&action_url) class="inline-form" {
                                    (PreEscaped(csrf))
                                    input type="hidden" name="conf_action" value="preview_publish_mirror";
                                    input type="hidden" name="publish_mirror_id" value=(mirror.id);
                                    button type="submit" { (t::preview_button(locale)) }
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
                @if let Some(shown) = shown.filter(|s| s.mirror_id == mirror.id) {
                    (render_preview(&mirror.label, &shown.preview, locale))
                }
                (render_form(repo_name, Some((mirror, etag.as_str())), csrf, locale))
                hr;
            }
            h4 { (t::add_heading(locale)) }
            (render_form(repo_name, None, csrf, locale))
        },
    )
}
