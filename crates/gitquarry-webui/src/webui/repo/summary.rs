//! The default view (`?view=summary`): clone URLs, README, a handful of
//! branches/tags, and recent commits — the "front page" of a single repo.

use super::components::{commit_avatars, commit_table_with_stats, ref_avatars, ref_table, split_refs};
use super::{MAX_BLOB_BYTES, RECENT_LOG_LIMIT, SUMMARY_REFS_LIMIT};
use crate::auth::web::login;
use crate::webui::{
    effective_locale, effective_text_size, esc, github_blob_link, icons, markdown, raw_link, repo_link, topic_badges,
    url_encode,
};
use anyhow::{Context, Result};
use axum::http::HeaderMap;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::{Group, User};
use gitquarry_core::git;
use gitquarry_core::storage;
use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_i18n::web::repo_summary as t;
use maud::{Markup, PreEscaped, html};
use std::path::Path as FsPath;

/// The `<h3>clone</h3>` URL table — always both http (this same web server,
/// git smart-HTTP) and ssh://, regardless of whether `ssh_bind` was ever
/// saved to `.conf/global.json` (most deployments never do — running on the
/// CLI default is the common case, and `ctx.ssh_bind` already carries the
/// address actually bound at startup either way). Shared with
/// [`super::empty_repo_body`]: an empty repo needs the exact same URLs to
/// push to, not just an existing one to clone from.
#[cfg(any(test, feature = "test-util"))]
pub async fn render_clone_urls(ctx: &AppContext, repo_name: &str, headers: &HeaderMap) -> Result<Markup> {
    let locale = effective_locale(ctx, headers).await;
    // The `Host` header of this very request already carries the host:port
    // the browser used to reach the web server — the same port git
    // smart-HTTP is served on, since it's the same axum router.
    let host_with_port = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<host>");
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.split(':').next())
        .unwrap_or("<host>");
    let ssh_port = ctx.ssh_bind.rsplit_once(':').map(|(_, p)| p).unwrap_or(&ctx.ssh_bind);
    Ok(html! {
        h3 { (t::clone_heading(locale)) }
        table class="commit-meta" {
            tr { td { "http" } td { code { "http://" (host_with_port) "/" (repo_name) } } }
            tr { td { "ssh" } td { code { "ssh://" (host) ":" (ssh_port) "/" (repo_name) } } }
        }
    })
}
#[cfg(not(any(test, feature = "test-util")))]
pub(super) async fn render_clone_urls(ctx: &AppContext, repo_name: &str, headers: &HeaderMap) -> Result<Markup> {
    let locale = effective_locale(ctx, headers).await;
    // The `Host` header of this very request already carries the host:port
    // the browser used to reach the web server — the same port git
    // smart-HTTP is served on, since it's the same axum router.
    let host_with_port = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<host>");
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.split(':').next())
        .unwrap_or("<host>");
    let ssh_port = ctx.ssh_bind.rsplit_once(':').map(|(_, p)| p).unwrap_or(&ctx.ssh_bind);
    Ok(html! {
        h3 { (t::clone_heading(locale)) }
        table class="commit-meta" {
            tr { td { "http" } td { code { "http://" (host_with_port) "/" (repo_name) } } }
            tr { td { "ssh" } td { code { "ssh://" (host) ":" (ssh_port) "/" (repo_name) } } }
        }
    })
}

/// The repo's default landing tab (`?view=about`, or no `?view=` at all):
/// clone-adjacent metadata (snapshot links, fork lineage), the README, and
/// the GitHub-style sidebar (about/community/releases/language cards) —
/// everything cgit's own `summary` page *doesn't* show. [`render_summary`]
/// is the cgit-shaped counterpart: clone URLs plus the branches/tags/
/// recent-commits tables, nothing descriptive.
#[allow(clippy::too_many_arguments)]
pub(super) async fn render_about(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    bare_repo: &FsPath,
    repo_name: &str,
    rev: &str,
    config: &RepoConfig,
    headers: &HeaderMap,
    languages: &[(String, u64)],
) -> Result<String> {
    let size = effective_text_size(ctx, headers).await;
    // Re-derived here (cheap — a cookie read, occasionally one account
    // fetch) rather than taking it as one more parameter: keeps this
    // function's arg count under clippy's `too_many_arguments` threshold
    // without an arbitrary options-struct just to dodge the lint — same
    // reasoning `render_sidebar`'s own doc comment already gives for
    // re-checking README existence instead of taking it as a parameter.
    // `languages` breaks that pattern on purpose: unlike a cookie read,
    // `git::browse::language_stats` walks the whole tree — worth avoiding a
    // second walk when the caller (building the header's own `below_header`
    // language bar) already paid for one this same request.
    let locale = effective_locale(ctx, headers).await;
    // Resolving `forked_from` (a uid, not a name) *is* the existence check
    // the old raw-name hyperlink never had: a fetch that comes back with no
    // etag (source deleted) or errors just omits the line below, instead of
    // rendering a dead link. Recovers the source's current name too, so
    // this line survives the source being renamed any number of times.
    let forked_from_name = match &config.forked_from {
        Some(uid) => storage::settings::get_repo_config(ctx.backend().as_ref(), uid.as_str(), &ctx.cipher)
            .await
            .ok()
            .filter(|(_, etag)| etag.is_some())
            .map(|(source_config, _etag)| source_config.full_name),
        None => None,
    };
    // Fetched once and threaded through both calls below: `render_readme`
    // and `render_sidebar` used to each `ls_tree` the root independently
    // (and `render_sidebar` did it three more times internally, once per
    // README/CONTRIBUTING/LICENSE lookup) — four subprocess spawns of the
    // exact same listing on one page load.
    let root_entries = git::browse::ls_tree(bare_repo, rev, "").await?;
    let readme = render_readme_from_entries(bare_repo, rev, &root_entries).await?;
    let sidebar = render_sidebar(
        ctx,
        resolved,
        bare_repo,
        repo_name,
        rev,
        headers,
        config,
        languages,
        &root_entries,
    )
    .await?;

    Ok(html! {
        // `id="about-body"`: `text_size.rs`'s `render_text_size_buttons`
        // targets this exact id (`data-class-target`) to flip the class
        // instantly on click, before the round trip -- same idea as the
        // theme toggle's `data-html-attr`, just scoped to this div's own
        // class instead of `<html>`'s attribute (this preference only
        // affects the about tab's body, not the whole page).
        div id="about-body" class=(size.div_class()) {
            div class="repo-layout" {
                div class="repo-main" {
                    p {
                        a href=(raw_link("snapshot", repo_name, rev).fmt("tar.gz").to_string()) { (gitquarry_i18n::shared::snapshot_tar_gz(locale)) }
                        " · "
                        a href=(raw_link("snapshot", repo_name, rev).fmt("zip").to_string()) { (gitquarry_i18n::shared::snapshot_zip(locale)) }
                    }
                    @if let Some(source_name) = &forked_from_name {
                        p class="muted" { (t::forked_from_label(locale)) " " a href=(format!("/{}", url_encode(source_name))) { (source_name) } }
                    }

                    @if let Some(readme) = &readme {
                        (PreEscaped(readme))
                    }
                }
                div class="repo-sidebar" { (sidebar) }
            }
        }
    }
    .into_string())
}

/// cgit's own `summary` page, content-for-content: clone URLs, then the
/// branches/tags/recent-commits tables — no README, no sidebar, see
/// [`render_about`] for those. Single column (`.repo-main` alone, no
/// `.repo-layout`/`.repo-sidebar` flex pair): there's nothing here to put
/// beside.
pub(super) async fn render_summary(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    rev: &str,
    headers: &HeaderMap,
) -> Result<String> {
    let locale = effective_locale(ctx, headers).await;
    let clone_urls = render_clone_urls(ctx, repo_name, headers).await?;
    let refs = git::browse::list_refs_detailed(bare_repo, "refs/").await?;
    let (branches, tags) = split_refs(refs);
    let recent = git::browse::log_page_with_stats(bare_repo, rev, RECENT_LOG_LIMIT, 0, None).await?;
    let recent_commit_infos: Vec<_> = recent.iter().map(|(c, _)| c.clone()).collect();
    let recent_avatars = commit_avatars(ctx, bare_repo, &recent_commit_infos).await;
    let branch_avatars = ref_avatars(ctx, bare_repo, &branches[..branches.len().min(SUMMARY_REFS_LIMIT)]).await;
    let tag_avatars = ref_avatars(ctx, bare_repo, &tags[..tags.len().min(SUMMARY_REFS_LIMIT)]).await;

    Ok(html! {
        div class="repo-main" {
            (clone_urls)

            h3 { (icons::branch()) " " (t::branches_heading(locale)) }
            (ref_table(repo_name, &branches, SUMMARY_REFS_LIMIT, true, &branch_avatars, locale))
            @if branches.len() > SUMMARY_REFS_LIMIT {
                p { a href=(repo_link(repo_name, "refs", "").to_string()) { (t::all_branches_and_tags_link(locale)) } }
            }
            h3 { (icons::tag()) " " (t::tags_heading(locale)) }
            (ref_table(repo_name, &tags, SUMMARY_REFS_LIMIT, true, &tag_avatars, locale))

            h3 { (t::recent_commits_heading(locale)) }
            (commit_table_with_stats(repo_name, &recent, &recent_avatars, locale))
            p { a href=(repo_link(repo_name, "log", rev).to_string()) { (t::full_log_link(locale)) } }
        }
    }
    .into_string())
}

const README_NAMES: &[&str] = &["readme.md", "readme.markdown", "readme", "readme.txt"];
const CONTRIBUTING_NAMES: &[&str] = &[
    "contributing.md",
    "contributing.markdown",
    "contributing",
    "contributing.txt",
];
const LICENSE_NAMES: &[&str] = &[
    "license.md",
    "license",
    "license.txt",
    "copying",
    "copying.md",
    "copying.txt",
];

/// Name of the root-level blob matching one of `candidates` (case-
/// insensitive) among an already-fetched root listing, or `None` if there's
/// no such file. Pure lookup, no I/O — every caller shares one `ls_tree` of
/// the root ([`render_about`]'s `root_entries`) instead of each fetching its
/// own.
fn find_root_entry(entries: &[git::browse::TreeEntry], candidates: &[&str]) -> Option<String> {
    entries
        .iter()
        .find(|e| e.kind == "blob" && candidates.contains(&e.name.to_ascii_lowercase().as_str()))
        .map(|e| e.name.clone())
}

/// Best-effort SPDX-ish identifier for the root license file's content, from
/// a fixed set of distinctive phrases each license text is known to contain
/// — not a fuzzy/Dice-coefficient matcher (the approach tools like GitHub's
/// `licensee` use to handle edited/relicensed text): good enough for the
/// common unmodified case, `None` for anything else rather than a wrong
/// guess.
async fn detect_license_kind(bare_repo: &FsPath, rev: &str, name: &str) -> Result<Option<&'static str>> {
    let size = git::browse::blob_size(bare_repo, rev, name).await?;
    if size > MAX_BLOB_BYTES {
        return Ok(None);
    }
    let bytes = git::browse::read_blob(bare_repo, rev, name).await?;
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };
    Ok(license_kind_from_text(text))
}

#[cfg(any(test, feature = "test-util"))]
pub fn license_kind_from_text(text: &str) -> Option<&'static str> {
    let t = text.to_ascii_lowercase();
    let has = |s: &str| t.contains(s);

    if has("gnu affero general public license") {
        return Some(if has("version 3") { "AGPL-3.0" } else { "AGPL" });
    }
    if has("gnu lesser general public license") {
        return Some(if has("version 3") {
            "LGPL-3.0"
        } else if has("version 2.1") {
            "LGPL-2.1"
        } else {
            "LGPL"
        });
    }
    if has("gnu general public license") {
        return Some(if has("version 3") {
            "GPL-3.0"
        } else if has("version 2") {
            "GPL-2.0"
        } else {
            "GPL"
        });
    }
    if has("mozilla public license") && has("2.0") {
        return Some("MPL-2.0");
    }
    if has("apache license") && has("version 2.0") {
        return Some("Apache-2.0");
    }
    if has("this is free and unencumbered software released into the public domain") {
        return Some("Unlicense");
    }
    if has("cc0") && has("1.0 universal") {
        return Some("CC0-1.0");
    }
    if has("permission is hereby granted, free of charge") {
        return Some("MIT");
    }
    if has("permission to use, copy, modify, and/or distribute this software") {
        return Some("ISC");
    }
    if has("redistribution and use in source and binary forms") {
        return Some(if has("neither the name") && has("endorse or promote") {
            "BSD-3-Clause"
        } else {
            "BSD-2-Clause"
        });
    }
    None
}
#[cfg(not(any(test, feature = "test-util")))]
fn license_kind_from_text(text: &str) -> Option<&'static str> {
    let t = text.to_ascii_lowercase();
    let has = |s: &str| t.contains(s);

    if has("gnu affero general public license") {
        return Some(if has("version 3") { "AGPL-3.0" } else { "AGPL" });
    }
    if has("gnu lesser general public license") {
        return Some(if has("version 3") {
            "LGPL-3.0"
        } else if has("version 2.1") {
            "LGPL-2.1"
        } else {
            "LGPL"
        });
    }
    if has("gnu general public license") {
        return Some(if has("version 3") {
            "GPL-3.0"
        } else if has("version 2") {
            "GPL-2.0"
        } else {
            "GPL"
        });
    }
    if has("mozilla public license") && has("2.0") {
        return Some("MPL-2.0");
    }
    if has("apache license") && has("version 2.0") {
        return Some("Apache-2.0");
    }
    if has("this is free and unencumbered software released into the public domain") {
        return Some("Unlicense");
    }
    if has("cc0") && has("1.0 universal") {
        return Some("CC0-1.0");
    }
    if has("permission is hereby granted, free of charge") {
        return Some("MIT");
    }
    if has("permission to use, copy, modify, and/or distribute this software") {
        return Some("ISC");
    }
    if has("redistribution and use in source and binary forms") {
        return Some(if has("neither the name") && has("endorse or promote") {
            "BSD-3-Clause"
        } else {
            "BSD-2-Clause"
        });
    }
    None
}

/// Renders the root README as markdown (`.md`/`.markdown`) or as preformatted
/// text otherwise. `None` if the repo has no README at `rev`'s root, it's too
/// big, or it isn't valid UTF-8.
///
/// Only called from cross-crate `tests/*.rs` (`render_about` calls
/// [`render_readme_from_entries`] directly instead, sharing one `ls_tree`)
/// — dead outside `test-util`, which is what `cargo check` flags.
#[cfg(any(test, feature = "test-util"))]
pub async fn render_readme(bare_repo: &FsPath, rev: &str) -> Result<Option<String>> {
    let root_entries = git::browse::ls_tree(bare_repo, rev, "").await?;
    render_readme_from_entries(bare_repo, rev, &root_entries).await
}

/// [`render_readme`]'s body, taking an already-fetched root listing so
/// [`render_about`] can share one `ls_tree` across this and
/// [`render_sidebar`] instead of each doing their own.
async fn render_readme_from_entries(
    bare_repo: &FsPath,
    rev: &str,
    root_entries: &[git::browse::TreeEntry],
) -> Result<Option<String>> {
    let Some(name) = find_root_entry(root_entries, README_NAMES) else {
        return Ok(None);
    };

    let size = git::browse::blob_size(bare_repo, rev, &name).await?;
    if size > MAX_BLOB_BYTES {
        return Ok(None);
    }
    let bytes = git::browse::read_blob(bare_repo, rev, &name).await?;
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };

    let lower = name.to_ascii_lowercase();
    let rendered = if lower.ends_with(".md") || lower.ends_with(".markdown") {
        // Parsing + syntax-highlighting fenced code blocks is synchronous
        // CPU work (same reasoning as `blob::render_blob`'s own
        // `spawn_blocking` for a source file) — a large README with a lot
        // of highlighted code shouldn't stall an async worker thread.
        let owned_text = text.to_string();
        tokio::task::spawn_blocking(move || markdown::render(&owned_text))
            .await
            .context("readme markdown rendering task panicked")?
    } else {
        format!("<pre>{}</pre>", esc(text))
    };
    Ok(Some(format!("<div id=\"readme\" class=\"readme\">{rendered}</div>")))
}

/// The star/watch/fork count row alone, `id="community-stats"` so the
/// star/watch buttons' async toggle response (see `actions.rs`'s
/// `data-async-extra-target`) can replace just this row in place — without
/// an id to swap by, a click on either button would flip the button's own
/// label but leave this row showing the pre-toggle count until a full
/// reload, since the button (in the header) and this row (in the sidebar)
/// are two different subtrees the single-target `data-async` swap can't
/// reach at once.
pub(crate) fn community_stats_html(config: &RepoConfig, locale: gitquarry_i18n::Locale) -> Markup {
    html! {
        div id="community-stats" class="community-stats" {
            // `id="star-count-value"`/`id="watcher-count-value"` around the
            // number alone (not the icon/label): `actions.rs`'s
            // `data-optimistic-target` needs to bump just the digits, in
            // place, the instant star/watch is clicked.
            span class="community-stat" { (icons::star()) " " span id="star-count-value" { (config.star_count) } " " (t::stars_label(locale)) }
            span class="community-stat" { (icons::eye()) " " span id="watcher-count-value" { (config.watcher_count) } " " (t::watchers_label(locale)) }
            span class="community-stat" { (icons::fork()) " " (config.fork_count) " " (t::forks_label(locale)) }
        }
    }
}

/// GitHub-style right sidebar: an "about" box (description + topics, only
/// shown when there's something to show — a repo with neither is exactly as
/// bare as the page was before this box existed), a "community" box
/// (README/CONTRIBUTING existence, activity link, star/fork counts with a
/// fork form for a logged-in viewer — same content the old inline
/// "community" strip had, just moved into the sidebar), and a "releases" box
/// (count only, linking to the releases tab — that tab already renders the
/// full list, no need to duplicate release details here).
///
/// Takes the caller's already-fetched root listing (`root_entries`) rather
/// than re-`ls_tree`-ing the root itself for each of README/CONTRIBUTING/
/// LICENSE — a lookup against a slice already in memory adds no argument-
/// count pressure the way three more scalar fields would, so this still
/// avoids the arbitrary options-struct [`render_about`]'s own doc comment
/// reasons about.
/// `config.fork_count`/`config.star_count` are the caller's already-loaded
/// `RepoConfig` — see those fields' own doc comments for why neither is a
/// bucket-wide scan anymore; unlike the old `access.json`-scan version, this
/// means an anonymous viewer sees an accurate count too, not just a
/// logged-in one.
#[allow(clippy::too_many_arguments)]
async fn render_sidebar(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    bare_repo: &FsPath,
    repo_name: &str,
    rev: &str,
    headers: &HeaderMap,
    config: &RepoConfig,
    languages: &[(String, u64)],
    root_entries: &[git::browse::TreeEntry],
) -> Result<Markup> {
    let locale = effective_locale(ctx, headers).await;
    let readme_exists = find_root_entry(root_entries, README_NAMES).is_some();
    let contributing_name = find_root_entry(root_entries, CONTRIBUTING_NAMES);
    let license_name = find_root_entry(root_entries, LICENSE_NAMES);
    let license_kind = match &license_name {
        Some(name) => detect_license_kind(bare_repo, rev, name).await?,
        None => None,
    };
    let viewer = login::current_user(ctx, headers).await;
    // Only needed now for the fork form's namespace list ("every group I
    // can manage") — the star count no longer depends on this fetch at all,
    // and enumerating groups is unavoidable here: there's no reverse index
    // from user to "groups I own", so the fork form has to look at all of
    // them.
    let groups = match &viewer {
        Some(_) => Some(access::list_all_groups(&ctx.backend(), &ctx.cipher).await?),
        None => None,
    };
    let release_count = resolved.remote.list_release_tags().await?.len();

    Ok(html! {
        @if config.description.is_some() || !config.topics.is_empty() {
            div class="card card-pad sidebar-box" {
                h4 { (t::about_heading(locale)) }
                @if let Some(description) = &config.description {
                    p { (description) }
                }
                @if !config.topics.is_empty() {
                    p { (PreEscaped(topic_badges(&config.topics))) }
                }
            }
        }
        div class="card card-pad sidebar-box sidebar-lang" {
            h4 { (t::languages_heading(locale)) }
            (render_language_bar(languages, locale))
        }
        div class="card card-pad sidebar-box" {
            h4 { (t::community_heading(locale)) }
            // GitHub-style: a compact stat row (icon + count, no "not
            // found" placeholders — the community-list below already
            // follows that same "only show what exists" convention for
            // readme/contributing/license), then a plain vertical list of
            // icon+link rows instead of a two-column label/value table.
            (community_stats_html(config, locale))
            ul class="community-list" {
                @if readme_exists {
                    li { (icons::book()) " " a href="#readme" { (t::readme_label(locale)) } }
                }
                @if let Some(name) = &contributing_name {
                    li { (icons::file()) " " a href=(github_blob_link(repo_name, rev, name)) { (t::contributing_label(locale)) } }
                }
                @if let Some(name) = &license_name {
                    li {
                        (icons::license()) " "
                        a href=(github_blob_link(repo_name, rev, name)) {
                            @if let Some(kind) = &license_kind { (kind) } @else { (t::license_label(locale)) }
                        }
                    }
                }
                li { (icons::activity()) " " a href=(repo_link(repo_name, "activity", rev).to_string()) { (t::recent_activity_link(locale)) } }
            }
            @if let Some(me) = &viewer {
                p class="text-sm" { a href="/_account/favorites" { (t::your_favorites_link(locale)) } }
                (render_fork_form(repo_name, me, headers, groups.as_deref().expect("viewer implies groups were fetched"), "", None, locale))
            }
        }
        div class="card card-pad sidebar-box" {
            h4 { (t::releases_heading(locale)) }
            p {
                a href=(repo_link(repo_name, "releases", "").to_string()) {
                    @if release_count == 0 {
                        (t::no_releases(locale))
                    } @else if release_count == 1 {
                        (t::one_release(locale))
                    } @else {
                        (t::n_releases(locale, release_count))
                    }
                }
            }
        }
    })
}

/// The color GitHub's own Linguist assigns each language (`languages.yml`)
/// — matches every name [`git::browse::language_for_extension`] can
/// produce, so the language bar reads the same "Rust is orange, Python is
/// blue" way GitHub's own repo page does, instead of an arbitrary hash-hue.
/// `JSX` has no color of its own in Linguist (`.jsx` is just JavaScript
/// there) — reuses JavaScript's, the closest honest answer.
fn linguist_color(name: &str) -> Option<&'static str> {
    Some(match name {
        "Rust" => "#dea584",
        "Python" => "#3572A5",
        "JavaScript" | "JSX" => "#f1e05a",
        "TypeScript" => "#3178c6",
        "TSX" => "#3178c6",
        "Go" => "#00ADD8",
        "Java" => "#b07219",
        "Kotlin" => "#A97BFF",
        "C" => "#555555",
        "C++" => "#f34b7d",
        "C#" => "#178600",
        "Ruby" => "#701516",
        "PHP" => "#4F5D95",
        "Swift" => "#F05138",
        "Scala" => "#c22d40",
        "Haskell" => "#5e5086",
        "Elixir" => "#6e4a7e",
        "Erlang" => "#B83998",
        "Lua" => "#000080",
        "Perl" => "#0298c3",
        "Shell" => "#89e051",
        "PowerShell" => "#012456",
        "SQL" => "#e38c00",
        "HTML" => "#e34c26",
        "CSS" => "#563d7c",
        "SCSS" => "#c6538c",
        "Vue" => "#41b883",
        "Svelte" => "#ff3e00",
        "JSON" => "#292929",
        "YAML" => "#cb171e",
        "TOML" => "#9c4221",
        "Markdown" => "#083fa1",
        "Dockerfile" => "#384d54",
        "Nix" => "#7e7eff",
        "Zig" => "#ec915c",
        "Dart" => "#00B4AB",
        "R" => "#198CE7",
        "Julia" => "#a270ba",
        _ => return None,
    })
}

/// Color for a language name — [`linguist_color`] when it's one of the
/// languages `gitquarry_core::git::browse::language_for_extension` can produce, else a
/// fixed hash-to-hue fallback (djb2-style) so an unrecognized name still
/// gets a stable, distinct color instead of a missing one.
#[cfg(any(test, feature = "test-util"))]
pub fn language_color(name: &str) -> String {
    if let Some(hex) = linguist_color(name) {
        return hex.to_string();
    }
    let mut hash: u32 = 5381;
    for b in name.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(b));
    }
    format!("hsl({} 55% 45%)", hash % 360)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(super) fn language_color(name: &str) -> String {
    if let Some(hex) = linguist_color(name) {
        return hex.to_string();
    }
    let mut hash: u32 = 5381;
    for b in name.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(b));
    }
    format!("hsl({} 55% 45%)", hash % 360)
}

/// One stacked bar (segment width = byte share) plus a legend underneath —
/// same "dynamic value inline, shape in CSS" split as `stats.rs`'s own
/// commit-count bars (`.lang-bar`/`.lang-swatch`, `base.css`). Each legend
/// entry's dot+name+percentage sits inside its own `.lang-key-item` (also
/// `base.css`) so wrapping at a narrow width breaks between languages, not
/// between a dot and the label it belongs to.
#[cfg(any(test, feature = "test-util"))]
pub fn render_language_bar(languages: &[(String, u64)], locale: gitquarry_i18n::Locale) -> Markup {
    let total: u64 = languages.iter().map(|(_, n)| n).sum();
    html! {
        @if total == 0 {
            p { em { (t::languages_empty_hint(locale)) } }
        } @else {
            div class="lang-bar" {
                @for (lang, bytes) in languages {
                    span
                        style=(format!("width:{:.2}%;background:{}", 100.0 * *bytes as f64 / total as f64, language_color(lang)))
                        title=(lang) {}
                }
            }
            p class="muted" {
                @for (i, (lang, bytes)) in languages.iter().enumerate() {
                    @if i > 0 { " " }
                    span class="lang-key-item" {
                        span class="lang-swatch" style=(format!("background:{}", language_color(lang))) {}
                        (lang) " " (format!("{:.1}%", 100.0 * *bytes as f64 / total as f64))
                    }
                }
            }
        }
    }
}
#[cfg(not(any(test, feature = "test-util")))]
pub(super) fn render_language_bar(languages: &[(String, u64)], locale: gitquarry_i18n::Locale) -> Markup {
    let total: u64 = languages.iter().map(|(_, n)| n).sum();
    html! {
        @if total == 0 {
            p { em { (t::languages_empty_hint(locale)) } }
        } @else {
            div class="lang-bar" {
                @for (lang, bytes) in languages {
                    span
                        style=(format!("width:{:.2}%;background:{}", 100.0 * *bytes as f64 / total as f64, language_color(lang)))
                        title=(lang) {}
                }
            }
            p class="muted" {
                @for (i, (lang, bytes)) in languages.iter().enumerate() {
                    @if i > 0 { " " }
                    span class="lang-key-item" {
                        span class="lang-swatch" style=(format!("background:{}", language_color(lang))) {}
                        (lang) " " (format!("{:.1}%", 100.0 * *bytes as f64 / total as f64))
                    }
                }
            }
        }
    }
}

/// A small "fork this repo" form under the forks count, only for a
/// logged-in viewer — forking has to land somewhere the caller already
/// controls, either their own username or a group they can manage (an
/// admin, or listed as an owner — same rule `access`'s
/// `can_manage_group_for` applies, just evaluated directly against
/// `viewer` since both are already in hand here). `groups` comes from the
/// caller — see [`render_sidebar`], which already fetched every group for
/// this. Stacked vertically (`class="fork-form"`, see `base.css`) rather
/// than the old single inline row: that fit fine in the full-width main
/// column this used to live in, but overflows the sidebar's ~220px content
/// width once a `<select>` + a 20-char `<input>` + a button all have to
/// share it.
/// `name_value`/`error`: a rejected fork (bad name, destination already
/// exists, ...) re-renders this exact form with whatever the caller typed
/// still filled in and the reason shown above it, instead of losing it to
/// the generic error page — `""`/`None` for the plain sidebar render.
fn render_fork_form(
    repo_name: &str,
    viewer: &User,
    headers: &HeaderMap,
    groups: &[Group],
    name_value: &str,
    error: Option<&str>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let default_name = repo_name.rsplit('/').next().unwrap_or(repo_name);
    let name_value = if name_value.is_empty() {
        default_name
    } else {
        name_value
    };
    html! {
        @if let Some(msg) = error {
            p class="alert alert-err" { (msg) }
        }
        form method="post" action="/_fork" class="fork-form" {
            (PreEscaped(crate::webui::csrf_field(headers)))
            input type="hidden" name="source" value=(repo_name);
            select name="namespace" {
                option value=(viewer.username) { (viewer.username) " " (t::you_suffix(locale)) }
                @for group in groups.iter().filter(|g| viewer.is_admin || g.owners.contains(&viewer.uid)) {
                    option value=(group.name) { (group.name) }
                }
            }
            input name="name" size="20" value=(name_value) placeholder=(t::name_placeholder(locale));
            button type="submit" class="btn" { (icons::fork()) " " (t::fork_button(locale)) }
        }
    }
}
