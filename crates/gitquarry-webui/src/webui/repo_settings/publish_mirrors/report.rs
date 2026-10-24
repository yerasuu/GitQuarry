//! What a publish mirror reports: the dry-run preview and the record of its
//! last runs, with every gate finding in words.

use gitquarry_i18n::Locale;
use gitquarry_i18n::web::publish_mirrors as t;
use gitquarry_mirror_external::publish::audit::{Finding, Gate, GateReport};
use gitquarry_mirror_external::publish::filter::ExcludedBy;
use gitquarry_mirror_external::publish::log::{GateRecord, PublishRun, RunOutcome};
use gitquarry_mirror_external::publish::preview::{Preview, RefPreview};
use gitquarry_mirror_external::publish::rewrite::IdentRole;
use gitquarry_mirror_external::publish::{self, PublishMode};
use maud::{Markup, html};

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
        } => match (published_path.is_empty(), excluded_path.is_empty()) {
            (true, _) => t::finding_blob_already_public(locale, sha, excluded_path),
            (false, true) => t::finding_blob_excluded_earlier(locale, sha, published_path),
            (false, false) => t::finding_blob_also_excluded(locale, sha, published_path, excluded_path),
        },
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
        Finding::UnmappedIdentity {
            email,
            role: IdentRole::Tagger,
            commits,
            example_commit,
        } => t::finding_unmapped_tagger(locale, email, *commits, example_commit),
        Finding::HostNotAllowed { error } => t::finding_host_not_allowed(locale, error),
        Finding::PublisherMissing => t::finding_publisher_missing(locale),
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

fn render_ref(r: &RefPreview, history: bool, locale: Locale) -> Markup {
    html! {
        h5 { code { (r.ref_name) } }
        @if let Some(error) = &r.error {
            p class="flash flash-err" { (t::ref_error(locale)) ": " (error) }
        } @else {
            @if r.history_truncated {
                p class="flash flash-err" { (t::history_truncated(locale, publish::preview::MAX_PREVIEW_COMMITS)) }
            }
            dl {
                dt { (t::private_head(locale)) } dd { code { (r.private_head) } }
                dt { (t::public_head(locale)) }
                dd {
                    @match &r.public_head {
                        Some(sha) => {
                            code { (sha) }
                            @if r.unchanged { " " em { "(" (t::public_head_unchanged(locale)) ")" } }
                        }
                        None => em { (t::public_head_none(locale)) },
                    }
                }
                dt { (t::published_commits(locale)) } dd { (r.published_commits) }
                dt { (t::collapsed_commits(locale)) } dd { (r.collapsed_commits) }
                @if history {
                    dt { (t::commits_to_rewrite(locale)) } dd { (r.commits_to_rewrite) }
                    dt { (t::commits_reused(locale)) } dd { (r.commits_reused) }
                }
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
                summary { (t::published_paths(locale, r.published_paths_total)) }
                @if r.published_paths.len() < r.published_paths_total {
                    p class="muted" { em { (t::paths_truncated(locale, r.published_paths.len(), r.published_paths_total)) } }
                }
                ul {
                    @for path in &r.published_paths { li { code { (path) } } }
                }
            }
        }
    }
}

/// Truncation is the only thing keeping the preview from passing.
fn only_truncated(preview: &Preview) -> bool {
    preview.host.passed()
        && !preview.epoch_bump_needed
        && !preview.refs.is_empty()
        && preview.refs.iter().any(|r| r.history_truncated)
        && preview
            .refs
            .iter()
            .all(|r| r.error.is_none() && r.gates.iter().all(GateReport::passed))
}

/// `history` is whether the mirror publishes its history, the only mode
/// with epochs, force pushes and rewrite counts to show.
pub(super) fn render_preview(label: &str, preview: &Preview, history: bool, locale: Locale) -> Markup {
    html! {
        div class="publish-preview" {
            h4 { (t::preview_heading(locale, label)) }
            @if history {
                p { (t::preview_epoch(locale, preview.epoch)) }
                @if preview.epoch_bump_needed {
                    p class="flash flash-err" { (t::preview_bump_needed(locale)) }
                }
                @if preview.force_push {
                    p class="flash flash-err" { strong { (t::preview_force_push(locale)) } }
                }
            }
            @if preview.passed() {
                p class="flash flash-ok" { (t::preview_verdict_pass(locale)) }
            } @else if only_truncated(preview) {
                p class="flash flash-err" { (t::preview_verdict_truncated(locale)) }
            } @else {
                p class="flash flash-err" { (t::preview_verdict_blocked(locale)) }
            }
            div class="scroll-x" {
                table class="list" { (render_gate(&preview.host, locale)) }
            }
            @if preview.refs.is_empty() {
                p { em { (t::preview_no_refs(locale)) } }
            }
            @for r in &preview.refs { (render_ref(r, history, locale)) }
        }
    }
}

fn outcome_badge(outcome: RunOutcome, locale: Locale) -> Markup {
    let (text, class) = match outcome {
        RunOutcome::Published => (t::outcome_published(locale), "badge ok"),
        RunOutcome::Unchanged => (t::outcome_unchanged(locale), "badge"),
        RunOutcome::Blocked => (t::outcome_blocked(locale), "badge danger"),
        RunOutcome::Diverged => (t::outcome_diverged(locale), "badge warn"),
        RunOutcome::Failed => (t::outcome_failed(locale), "badge danger"),
    };
    html! { span class=(class) { (text) } }
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn render_gate_record(record: &GateRecord, locale: Locale) -> Markup {
    html! {
        li {
            (gate_label(record.gate, locale)) ": "
            @if record.passed() {
                (t::gate_pass(locale))
            } @else {
                strong { (t::gate_blocked(locale)) }
                ul {
                    @for finding in &record.findings { li { (finding_text(finding, locale)) } }
                    @if record.omitted > 0 { li { em { (t::findings_omitted(locale, record.omitted)) } } }
                }
            }
        }
    }
}

pub(super) fn render_runs(runs: &[PublishRun], locale: Locale) -> Markup {
    html! {
        details {
            summary { (t::runs_heading(locale, runs.len())) }
            @if runs.is_empty() {
                p class="muted" { em { (t::runs_none(locale)) } }
            } @else {
                div class="scroll-x" {
                    table class="list" {
                        tr {
                            th { (t::run_started(locale)) }
                            th { (t::run_outcome(locale)) }
                            th { (t::run_refs(locale)) }
                            th { (t::run_gates(locale)) }
                            th { (t::run_duration(locale)) }
                        }
                        @for run in runs {
                            tr {
                                td { (run.started_at) }
                                td {
                                    (outcome_badge(run.outcome, locale))
                                    @if run.mode == PublishMode::History {
                                        br;
                                        small {
                                            @if let Some(epoch) = run.epoch { (t::run_epoch(locale, epoch)) " " }
                                            @if run.forced { strong { (t::run_forced(locale)) } " " }
                                            (t::run_history_counts(
                                                locale,
                                                run.commits_rewritten,
                                                run.commits_reused,
                                                run.commits_collapsed,
                                            ))
                                        }
                                    }
                                    @if let Some(error) = &run.error { br; small { (error) } }
                                }
                                td {
                                    @for r in &run.refs {
                                        div {
                                            code { (r.ref_name) } " "
                                            code { (short(&r.private_head)) } " → "
                                            @match &r.public_head {
                                                Some(head) => code { (short(head)) },
                                                None => em { "—" },
                                            }
                                            " " (t::run_counts(locale, r.published_files, r.excluded_entries))
                                        }
                                    }
                                }
                                td {
                                    @if run.gates_passed() {
                                        (t::gate_pass(locale))
                                    } @else {
                                        ul {
                                            @if let Some(host) = &run.host { (render_gate_record(host, locale)) }
                                            @for r in &run.refs {
                                                @for gate in r.gates.iter().filter(|g| !g.passed()) {
                                                    (render_gate_record(gate, locale))
                                                }
                                            }
                                        }
                                    }
                                }
                                td { (t::run_duration_ms(locale, run.duration_ms)) }
                            }
                        }
                    }
                }
            }
        }
    }
}
