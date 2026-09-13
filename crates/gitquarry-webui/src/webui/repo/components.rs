//! Render helpers shared by 2+ views — [`commit_table`] by summary/tree/log,
//! [`ref_table`]/[`split_refs`] by summary/refs, [`color_diff`] by
//! commit/compare. Anything used by exactly one view lives in that view's
//! own file instead.

use crate::webui::{csrf_field, esc, repo_link, url_encode};
use axum::http::HeaderMap;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::{access, entities::User};
use gitquarry_core::domain;
use gitquarry_core::domain::commit_signature::CommitSigner;
use gitquarry_core::git::browse::{CommitInfo, RefInfo};
use gitquarry_core::validate;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::components as t;
use maud::{Markup, PreEscaped, html};

/// The `.w-*` class for a percentage, clamped to the whole-percent rules
/// `style/utilities.css` defines.
///
/// A width can't be a `style` attribute — the CSP allows no inline style,
/// and a nonce covers `<style>` elements, never attributes — so a bar
/// segment picks the nearest class instead. Any non-zero share rounds up to
/// at least `.w-1`: a language at 0.2% of a repo should read as a sliver,
/// not vanish, and `.lang-bar`'s flex row absorbs the odd percent that
/// rounding adds.
pub(crate) fn width_class(percent: f64) -> &'static str {
    if percent <= 0.0 {
        return WIDTH_CLASSES[0];
    }
    WIDTH_CLASSES[percent.round().clamp(1.0, 100.0) as usize]
}

/// Written out rather than formatted per segment: these are the exact names
/// `style/utilities.css` carries, and a `&'static str` costs no allocation
/// on a page that renders a few hundred of them.
const WIDTH_CLASSES: [&str; 101] = [
    "w-0", "w-1", "w-2", "w-3", "w-4", "w-5", "w-6", "w-7", "w-8", "w-9", "w-10", "w-11", "w-12", "w-13", "w-14",
    "w-15", "w-16", "w-17", "w-18", "w-19", "w-20", "w-21", "w-22", "w-23", "w-24", "w-25", "w-26", "w-27", "w-28",
    "w-29", "w-30", "w-31", "w-32", "w-33", "w-34", "w-35", "w-36", "w-37", "w-38", "w-39", "w-40", "w-41", "w-42",
    "w-43", "w-44", "w-45", "w-46", "w-47", "w-48", "w-49", "w-50", "w-51", "w-52", "w-53", "w-54", "w-55", "w-56",
    "w-57", "w-58", "w-59", "w-60", "w-61", "w-62", "w-63", "w-64", "w-65", "w-66", "w-67", "w-68", "w-69", "w-70",
    "w-71", "w-72", "w-73", "w-74", "w-75", "w-76", "w-77", "w-78", "w-79", "w-80", "w-81", "w-82", "w-83", "w-84",
    "w-85", "w-86", "w-87", "w-88", "w-89", "w-90", "w-91", "w-92", "w-93", "w-94", "w-95", "w-96", "w-97", "w-98",
    "w-99", "w-100",
];

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinSet;

/// Batched account lookup for [`commit_table`]/[`commit_table_with_stats`],
/// keyed by commit sha (not `author_email`: a GPG-verified match resolves
/// to a real account regardless of what free-text email the commit
/// carries). Two tiers, GPG first: [`domain::commit_signature::verified_signer`]
/// (one per commit, concurrent — cheap for the common "unsigned" case,
/// which short-circuits before any I/O) checked first, [`access::avatar_by_commit_email`]'s
/// weaker author-email match only for whatever's left unverified. Not
/// folded into [`commit_table`]/[`commit_table_with_stats`] themselves:
/// both stay plain sync `fn`s (no `ctx`), reusable from `remote_repo`'s
/// federation-peer view too, which fetches its commits from a synced
/// mirror rather than a local repo but shares the exact same rendering.
/// `bool` is `true` only when the account came from
/// [`domain::commit_signature::verified_signer`] (a real GPG check against
/// that account's own registered keys) — `false` for the weaker author-
/// email fallback. [`author_cell`] uses this to tell "verified" from
/// "unverified" apart in the signature indicator, not just to pick an
/// avatar.
#[derive(Default)]
pub(crate) struct AvatarMatches {
    by_sha: HashMap<String, (User, bool)>,
    /// Shas this instance signed itself. Not in `by_sha` because the
    /// instance is not an account: it has no avatar to show and no profile
    /// to link, only a badge saying the commit was built here.
    instance_signed: std::collections::HashSet<String>,
}

impl AvatarMatches {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn get(&self, sha: &str) -> Option<&(User, bool)> {
        self.by_sha.get(sha)
    }

    fn insert(&mut self, sha: String, value: (User, bool)) {
        self.by_sha.insert(sha, value);
    }

    pub(crate) fn is_instance_signed(&self, sha: &str) -> bool {
        self.instance_signed.contains(sha)
    }
}

pub(crate) async fn commit_avatars(ctx: &AppContext, git_dir: &Path, commits: &[CommitInfo]) -> AvatarMatches {
    avatars_for(
        ctx,
        git_dir,
        commits
            .iter()
            .map(|c| (c.sha.clone(), c.signature_status, c.author_email.clone())),
    )
    .await
}

/// Same GPG-then-email tiered lookup as [`commit_avatars`], for
/// [`RefInfo`]'s branch/tag tips instead of a `CommitInfo` log page — the
/// two share this instead of each spawning their own `JoinSet` pair, since
/// [`ref_table`] needs the exact same matching [`commit_table`] gets, not a
/// weaker email-only stand-in.
pub(crate) async fn ref_avatars(ctx: &AppContext, git_dir: &Path, refs: &[RefInfo]) -> AvatarMatches {
    avatars_for(
        ctx,
        git_dir,
        refs.iter()
            .map(|r| (r.sha.clone(), r.signature_status, r.author_email.clone())),
    )
    .await
}

async fn avatars_for(
    ctx: &AppContext,
    git_dir: &Path,
    entries: impl IntoIterator<Item = (String, char, String)>,
) -> AvatarMatches {
    let backend = ctx.backend();
    let cipher = Arc::clone(&ctx.cipher);

    let mut set = JoinSet::new();
    for (sha, signature_status, author_email) in entries {
        let backend = Arc::clone(&backend);
        let cipher = Arc::clone(&cipher);
        let git_dir = git_dir.to_path_buf();
        set.spawn(async move {
            // `cached_signer`, not `verified_signer`: rendering a page is a
            // read, and the computing variant turned every uncached row
            // into a bucket write plus a `gpg` spawn inside the request.
            // A miss falls back to the neutral "signed" badge and is
            // computed out of band, so the next view has the exact verdict.
            let cached = domain::commit_signature::cached_signer(&backend, &cipher, &sha, signature_status)
                .await
                .ok()
                .flatten();
            let signer = match cached {
                Some(signer) => signer,
                // Miss: compute it out of band so the next view is a hit,
                // and render this one with whatever the neutral badge says.
                None => {
                    domain::commit_signature::warm_signer(
                        Arc::clone(&backend),
                        Arc::clone(&cipher),
                        git_dir.clone(),
                        sha.clone(),
                        author_email.clone(),
                    );
                    None
                }
            };
            (sha, author_email, signer)
        });
    }
    let mut gpg_username_by_sha: HashMap<String, String> = HashMap::new();
    let mut instance_signed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut fallback_email_by_sha: HashMap<String, String> = HashMap::new();
    while let Some(result) = set.join_next().await {
        let Ok((sha, author_email, signer)) = result else {
            continue;
        };
        match signer {
            Some(CommitSigner::User(username)) => {
                gpg_username_by_sha.insert(sha, username);
            }
            Some(CommitSigner::Instance) => {
                instance_signed.insert(sha.clone());
                // The *author* is still the account that asked for the
                // merge, and their avatar belongs on the row like on any
                // other commit. An instance signature says who built the
                // commit, never who wrote it, so it has no business
                // erasing the author — which is exactly what leaving this
                // arm without an email fallback did: the row dropped to a
                // generated identicon.
                if !author_email.is_empty() {
                    fallback_email_by_sha.insert(sha, author_email);
                }
            }
            None if !author_email.is_empty() => {
                fallback_email_by_sha.insert(sha, author_email);
            }
            None => {}
        }
    }

    // Distinct usernames only: the same author's commits collapse to one
    // `get_user` instead of one per sha.
    let distinct_usernames: BTreeSet<String> = gpg_username_by_sha.values().cloned().collect();
    let mut user_set = JoinSet::new();
    for username in distinct_usernames {
        let backend = Arc::clone(&backend);
        let cipher = Arc::clone(&cipher);
        user_set.spawn(async move { access::get_user(backend.as_ref(), &cipher, &username).await.ok()?.0 });
    }
    let mut users_by_username: HashMap<String, User> = HashMap::new();
    while let Some(result) = user_set.join_next().await {
        if let Ok(Some(user)) = result {
            users_by_username.insert(user.username.clone(), user);
        }
    }

    let email_matches =
        access::avatar_by_commit_email(&backend, &cipher, fallback_email_by_sha.values().cloned()).await;

    let mut out = AvatarMatches::new();
    out.instance_signed = instance_signed;
    for (sha, username) in gpg_username_by_sha {
        if let Some(user) = users_by_username.get(&username) {
            out.insert(sha, (user.clone(), true));
        }
    }
    for (sha, email) in fallback_email_by_sha {
        if let Some(user) = email_matches.get(&email) {
            out.insert(sha, (user.clone(), false));
        }
    }
    out
}

/// [`commit_table`]/[`commit_table_with_stats`]/[`ref_table`]'s author cell:
/// the matched account's own avatar (and real `username`, not the free-text
/// `author_name` git carries) when this sha resolved to one via
/// [`commit_avatars`]/[`ref_avatars`] (GPG-verified or author-email match,
/// in that order), otherwise the plain identicon keyed on `author_name` —
/// same fallback as before either existed, for the (usual, for any repo
/// with outside contributors) case of no matching account. Also renders
/// [`signature_indicator`] next to it — same signal `webui::repo::commit`'s
/// own badge shows on the single-commit page, just compact enough for a
/// table row.
fn author_cell(sha: &str, author_name: &str, signature_status: char, avatars: &AvatarMatches) -> Markup {
    html! {
        @match avatars.get(sha) {
            Some((user, _)) => (crate::webui::identicon::account_avatar(&user.username, user.avatar_content_type.as_deref())),
            None => (crate::webui::identicon::identicon(author_name)),
        }
        " " (author_name)
        (signature_indicator(
            signature_status,
            avatars.get(sha).filter(|(_, verified)| *verified).map(|(u, _)| u.username.as_str()),
            avatars.is_instance_signed(sha),
        ))
    }
}

/// Compact signature badge for a commit-table row: `verified_by` is
/// `Some(username)` only when [`AvatarMatches`]' `bool` was `true` for this
/// sha (a real GPG check against that account's own registered keys, not
/// just an author-email guess). `signature_status` only says whether a
/// signature is there at all, so on its own it can never read as more than
/// "unverified" — and empty for `'N'` (unsigned): no badge, same as the
/// single-commit page's own `webui::repo::commit::signature_badge`.
///
/// "unverified" also covers a row whose verdict simply isn't computed yet
/// (`domain::commit_signature::cached_signer` missed and
/// `warm_signer` is filling it in) — indistinguishable here from one that
/// was checked and matched nobody, and deliberately so: both mean this
/// server is not vouching for the signature right now.
fn signature_indicator(signature_status: char, verified_by: Option<&str>, instance_signed: bool) -> Markup {
    html! {
        @if let Some(username) = verified_by {
            " " span class="badge ok" title=(format!("verified signature ({username})")) { "verified" }
        } @else if instance_signed {
            // Same word as an account's own signature, a different colour:
            // both are genuinely verified signatures, and a row this narrow
            // is the wrong place to spell out whose — the commit's own page
            // says that in full. The `title` carries it regardless, so the
            // distinction never rests on colour alone, which would be lost
            // on a screen reader and on anyone who can't tell the two hues
            // apart.
            " " span class="badge accent" title="verified signature (built and signed by this instance)" { "verified" }
        } @else {
            @if signature_status != 'N' {
                " " span class="muted" title="signed, but not by any registered account's key" { "unverified" }
            }
        }
    }
}

/// A label pill: border+text colored, background left transparent — same
/// "colored outline, not a filled block" shape `.decoration` already uses,
/// just with a per-label color inlined instead of a fixed CSS var (the
/// color is dynamic per-repo data, can't be precomputed into a class, same
/// reasoning `page_chrome.rs`'s language bar documents for its own per-
/// language hue). Shared by issue and MR label rendering — `RepoConfig.labels`
/// is one core-owned vocabulary, not owned by either view.
#[cfg(any(test, feature = "test-util"))]
pub fn label_pill(name: &str, color: &str) -> Markup {
    html! {
        span class="decoration" style={"border-color:" (color) ";color:" (color)} { (name) }
    }
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn label_pill(name: &str, color: &str) -> Markup {
    html! {
        span class="decoration" style={"border-color:" (color) ";color:" (color)} { (name) }
    }
}

pub(crate) fn split_refs(refs: Vec<RefInfo>) -> (Vec<RefInfo>, Vec<RefInfo>) {
    refs.into_iter().partition(|r| r.name.starts_with("refs/heads/"))
}

/// `show_author`: cgit's own branches/tags table credits whoever's behind
/// each ref (see `RefInfo::author_name`'s own doc comment on tagger-vs-
/// commit-author) — opt-in per caller rather than always-on, since the
/// only other caller (`remote_repo`'s federation-peer summary) renders a
/// remote's own refs and has no reason to grow an extra column just
/// because this one did. `avatars`: [`ref_avatars`]'s output, unused (pass
/// `&AvatarMatches::new()`) when `show_author` is `false`.
pub(crate) fn ref_table(
    repo_name: &str,
    refs: &[RefInfo],
    limit: usize,
    show_author: bool,
    avatars: &AvatarMatches,
    locale: Locale,
) -> Markup {
    html! {
        div class="table-scroll" {
        table class="list" {
            tr {
                th { (t::name_column(locale)) } th { (t::commit_column(locale)) } th { (t::message_column(locale)) }
                @if show_author { th { (t::author_column(locale)) } }
                th { (t::date_column(locale)) }
            }
            @if refs.is_empty() {
                tr { td colspan=(if show_author { "5" } else { "4" }) { em { (t::none_hint(locale)) } } }
            }
            @for r in refs.iter().take(limit) {
                @let short = r
                    .name
                    .strip_prefix("refs/heads/")
                    .or_else(|| r.name.strip_prefix("refs/tags/"))
                    .unwrap_or(&r.name);
                tr {
                    td { a href=(repo_link(repo_name, "tree", short).to_string()) { (short) } }
                    td {
                        a href=(repo_link(repo_name, "commit", "").sha(&r.sha).to_string()) {
                            code { (validate::short_sha(&r.sha)) }
                        }
                    }
                    td { (r.subject) }
                    @if show_author {
                        td { (author_cell(&r.sha, &r.author_name, r.signature_status, avatars)) }
                    }
                    td class="muted" { (r.date) }
                }
            }
        }
        }
    }
}

pub(crate) fn commit_table(repo_name: &str, commits: &[CommitInfo], avatars: &AvatarMatches, locale: Locale) -> Markup {
    html! {
        div class="table-scroll" {
        table class="list" {
            tr { th { (t::commit_column(locale)) } th { (t::message_column(locale)) } th { (t::author_column(locale)) } th { (t::date_column(locale)) } }
            @for c in commits {
                tr {
                    td {
                        a href=(repo_link(repo_name, "commit", "").sha(&c.sha).to_string()) {
                            code { (validate::short_sha(&c.sha)) }
                        }
                    }
                    td { (c.subject) (decoration_badges(&c.decoration)) }
                    td { (author_cell(&c.sha, &c.author_name, c.signature_status, avatars)) }
                    td class="muted" { (c.date) }
                }
            }
        }
        }
    }
}

/// [`commit_table`] plus a cgit-style change-stat column (files changed,
/// `+insertions/-deletions`) — its own function rather than an extra
/// parameter on `commit_table`: fetching `CommitStats` costs one extra
/// `--shortstat` pass (see `git::browse::log_recent_with_stats`'s own doc
/// comment), which only the summary tab's bounded "recent commits" list
/// pays for. `commit_table`'s other five call sites (tree/log/activity/
/// compare/merges) keep using plain [`CommitInfo`], no `git log` shape
/// change forced on them for a column they don't render.
pub(crate) fn commit_table_with_stats(
    repo_name: &str,
    commits: &[(CommitInfo, Option<gitquarry_core::git::browse::CommitStats>)],
    avatars: &AvatarMatches,
    locale: Locale,
) -> Markup {
    html! {
        div class="table-scroll" {
        table class="list" {
            tr {
                th { (t::commit_column(locale)) } th { (t::message_column(locale)) } th { (t::author_column(locale)) }
                th { (t::changes_column(locale)) } th { (t::date_column(locale)) }
            }
            @for (c, stats) in commits {
                tr {
                    td {
                        a href=(repo_link(repo_name, "commit", "").sha(&c.sha).to_string()) {
                            code { (validate::short_sha(&c.sha)) }
                        }
                    }
                    td { (c.subject) (decoration_badges(&c.decoration)) }
                    td { (author_cell(&c.sha, &c.author_name, c.signature_status, avatars)) }
                    td class="muted" {
                        @if let Some(s) = stats {
                            (s.files_changed) " "
                            span class="diff-add" { "+" (s.insertions) }
                            " "
                            span class="diff-del" { "-" (s.deletions) }
                        }
                    }
                    td class="muted" { (c.date) }
                }
            }
        }
        }
    }
}

/// Renders `git log`'s `%D` decoration (`"HEAD -> main, tag: v1.0, origin/main"`)
/// as small badges next to the commit subject.
#[cfg(any(test, feature = "test-util"))]
pub fn decoration_badges(decoration: &str) -> Markup {
    html! {
        @if !decoration.is_empty() {
            @for d in decoration.split(", ") {
                " " span class="badge accent" { (d) }
            }
        }
    }
}
#[cfg(not(any(test, feature = "test-util")))]
fn decoration_badges(decoration: &str) -> Markup {
    html! {
        @if !decoration.is_empty() {
            @for d in decoration.split(", ") {
                " " span class="badge accent" { (d) }
            }
        }
    }
}

/// Colors unified diff lines cgit-style: `+`/`-` lines get a tinted
/// background (`diff-add-line`/`diff-del-line` — distinct from the
/// `diff-add`/`diff-del` classes [`commit_table`] uses for its insertion/
/// deletion count badges, an unrelated feature that keeps its own solid
/// text color), `@@ ... @@` blue, `+++`/`---` headers gray. Line content is
/// wrapped in `<code class="language-*">` (`super::diff::code_fragment`) so
/// `highlight.js` (client-side, vendored) tokenizes it; the background tint
/// keeps its `.hljs-*` colors visible on top instead of a solid block.
#[cfg(any(test, feature = "test-util"))]
pub fn color_diff(diff: &str) -> String {
    color_diff_impl(diff)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(super) fn color_diff(diff: &str) -> String {
    color_diff_impl(diff)
}

fn color_diff_impl(diff: &str) -> String {
    let mut out = String::with_capacity(diff.len());
    for row in super::diff::parse(diff) {
        match row {
            super::diff::Row::Meta(l) => {
                out.push_str("<span class=\"diff-meta\">");
                out.push_str(&esc(l));
                out.push_str("</span>");
            }
            super::diff::Row::Hunk(l) => {
                out.push_str("<span class=\"diff-hunk\">");
                out.push_str(&esc(l));
                out.push_str("</span>");
            }
            super::diff::Row::Line { class, path, text, .. } => {
                match class {
                    "diff-add" => out.push_str("<span class=\"diff-add-line\">"),
                    "diff-del" => out.push_str("<span class=\"diff-del-line\">"),
                    _ => {}
                }
                out.push(super::diff::prefix_char(class));
                out.push_str(&super::diff::code_fragment(path, text));
                if !class.is_empty() {
                    out.push_str("</span>");
                }
            }
        }
        out.push('\n');
    }
    out
}

/// One row of a [`side_by_side_diff`] table, built while scanning
/// [`super::diff::parse`]'s rows. Cells carry pre-escaped, `<code
/// class="language-*">`-wrapped HTML (`super::diff::code_fragment`), so
/// rendering uses `PreEscaped` rather than letting maud re-escape it.
enum SplitRow {
    /// A full-width row: `diff --git`/`index`/mode lines, `+++`/`---`
    /// headers, "\ No newline at end of file" — a four-column layout has no
    /// natural left/right split for these.
    Full(String),
    /// A `@@ ...@@` hunk header — full-width too, but its own color, same
    /// distinction [`color_diff`] draws.
    Hunk(String),
    /// An unchanged line, same content and (necessarily identical count,
    /// possibly different *value* — earlier adds/dels in the same hunk can
    /// have shifted one side already) line number on both sides.
    Context { old: u32, new: u32, html: String },
    /// A run of consecutive removals paired against a run of consecutive
    /// additions (git's own diff already groups a hunk's changes this way),
    /// each with its own side's line number. Zipped row by row; the shorter
    /// side pads with a blank gutter and cell, GitHub-style.
    Change(Vec<(u32, String)>, Vec<(u32, String)>),
}

/// Same diff [`color_diff`] renders as a single `+`/`-` stream, laid out as
/// an old-vs-new table instead, each side with its own line-number gutter
/// (GitHub/GitLab's own split-view convention): consecutive `-` lines
/// paired against the following consecutive `+` lines, one pair per row.
/// This is a purely positional pairing (git's raw diff carries no
/// line-to-line correspondence within a hunk), not a real similarity match,
/// so an unrelated removal and addition can end up side by side on the same
/// row.
#[cfg(any(test, feature = "test-util"))]
pub fn side_by_side_diff(diff: &str) -> Markup {
    side_by_side_diff_impl(diff)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(super) fn side_by_side_diff(diff: &str) -> Markup {
    side_by_side_diff_impl(diff)
}

fn side_by_side_diff_impl(diff: &str) -> Markup {
    let mut rows: Vec<SplitRow> = Vec::new();
    let mut dels: Vec<(u32, String)> = Vec::new();
    let mut adds: Vec<(u32, String)> = Vec::new();

    fn flush_change(rows: &mut Vec<SplitRow>, dels: &mut Vec<(u32, String)>, adds: &mut Vec<(u32, String)>) {
        if !dels.is_empty() || !adds.is_empty() {
            rows.push(SplitRow::Change(std::mem::take(dels), std::mem::take(adds)));
        }
    }

    for row in super::diff::parse(diff) {
        match row {
            super::diff::Row::Meta(l) => {
                flush_change(&mut rows, &mut dels, &mut adds);
                rows.push(SplitRow::Full(esc(l)));
            }
            super::diff::Row::Hunk(l) => {
                flush_change(&mut rows, &mut dels, &mut adds);
                rows.push(SplitRow::Hunk(esc(l)));
            }
            super::diff::Row::Line {
                old,
                new,
                class,
                path,
                text,
            } => {
                let fragment = super::diff::code_fragment(path, text);
                match class {
                    "diff-add" => adds.push((
                        new.expect("diff-add rows always carry a new-side line number"),
                        fragment,
                    )),
                    "diff-del" => dels.push((
                        old.expect("diff-del rows always carry an old-side line number"),
                        fragment,
                    )),
                    _ => {
                        flush_change(&mut rows, &mut dels, &mut adds);
                        rows.push(SplitRow::Context {
                            old: old.expect("context rows always carry an old-side line number"),
                            new: new.expect("context rows always carry a new-side line number"),
                            html: fragment,
                        });
                    }
                }
            }
        }
    }
    flush_change(&mut rows, &mut dels, &mut adds);

    html! {
        table class="diff-split" {
            // `table-layout: fixed` (base.css) makes these authoritative
            // instead of a mere hint: without it, an unwrapped long line
            // (`white-space: pre`, no wrap) grows its own column past its
            // share at the other columns' expense, since auto layout sizes
            // columns from content width first. Fixed layout reads column
            // widths from the first row only, which here can just as
            // easily be a `colspan="4"` header row — a `colgroup` is what
            // pins them regardless of which row happens to render first.
            colgroup { col class="w-5"; col class="w-45"; col class="w-5"; col class="w-45"; }
            @for row in &rows {
                @match row {
                    SplitRow::Full(l) => tr { td class="diff-meta" colspan="4" { (PreEscaped(l)) } }
                    SplitRow::Hunk(l) => tr { td class="diff-hunk" colspan="4" { (PreEscaped(l)) } }
                    SplitRow::Context { old, new, html } => tr {
                        td class="lineno" { (old) }
                        td { (PreEscaped(html)) }
                        td class="lineno" { (new) }
                        td { (PreEscaped(html)) }
                    }
                    SplitRow::Change(dels, adds) => {
                        @for i in 0..dels.len().max(adds.len()) {
                            tr {
                                @match dels.get(i) {
                                    Some((n, d)) => {
                                        td class="lineno" { (n) }
                                        td class="diff-del-line" { (PreEscaped(d)) }
                                    }
                                    None => {
                                        td class="lineno" {}
                                        td {}
                                    }
                                }
                                @match adds.get(i) {
                                    Some((n, a)) => {
                                        td class="lineno" { (n) }
                                        td class="diff-add-line" { (PreEscaped(a)) }
                                    }
                                    None => {
                                        td class="lineno" {}
                                        td {}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Every `#123` in `text` that names a real merge request on `target`,
/// mapped to its link. What isn't in the map isn't linked, which is how a
/// mention of a number that doesn't exist stays plain text instead of
/// becoming a dead link.
///
/// Separate from the rendering so the substitution can happen inside
/// [`crate::webui::markdown::render_with_mentions`], on the parsed event
/// stream: only the parser knows whether a given `#123` is prose or the
/// contents of a code fence. Resolving here scans the raw text, so a number
/// that only ever appears in code costs one lookup and is then never used.
pub(crate) async fn resolve_number_mentions(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    text: &str,
) -> std::collections::HashMap<u64, String> {
    let spans = crate::webui::markdown::scan_number_mentions(text);
    let mut numbers: Vec<u64> = spans.iter().map(|(_, n)| *n).collect();
    numbers.sort_unstable();
    numbers.dedup();

    let mut resolved = std::collections::HashMap::new();
    for number in numbers {
        if domain::merge_request::get(ctx, target, number)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            resolved.insert(number, repo_link(repo_name, "merge", "").number(number).to_string());
        }
    }
    resolved
}

/// Inline edit (a collapsed `<details>`, native zero-JS toggle) + delete
/// controls for one comment — `can_edit` (strictly author-only, no
/// override, see `i18n::shared::not_comment_author`'s own doc comment) and
/// `can_delete` (author-only too, but `Developer`+ on the repo also
/// qualifies — moderating someone else's comment away is different from
/// rewriting it) gate the edit/delete forms independently.
/// `mutation_prefix` is `"/_issues"` or `"/_merges"`; the two POST targets
/// are `{mutation_prefix}/comment_edit/{repo}` and
/// `{mutation_prefix}/comment_delete/{repo}`, the same "action first, repo
/// as the trailing catch-all" shape the rest of those routes use (see
/// `merges::mutation_url` for why the repo is in the path). Used by an MR's own flat thread and
/// its annotated inline diff comments ([`super::merges::annotated_diff`]); an
/// issue's own comments render this inline instead, predating this shared
/// extraction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn comment_edit_delete_controls(
    mutation_prefix: &str,
    repo_name: &str,
    repo_uid: &gitquarry_core::storage::uid::Uid,
    number: u64,
    comment_id: &str,
    body: &str,
    can_edit: bool,
    can_delete: bool,
    headers: &HeaderMap,
    locale: Locale,
) -> Markup {
    if !can_edit && !can_delete {
        return html! {};
    }
    let csrf_html = csrf_field(headers);
    html! {
        @if can_edit {
            details {
                summary { (t::edit_comment_button(locale)) }
                form method="post" action=(format!("{mutation_prefix}/comment_edit/{}", url_encode(repo_name))) {
                    (PreEscaped(&csrf_html))
                    input type="hidden" name="repo_uid" value=(repo_uid);
                    input type="hidden" name="number" value=(number);
                    input type="hidden" name="comment_id" value=(comment_id);
                    textarea name="body" rows="4" cols="60" { (body) }
                    p { button type="submit" { (t::save_comment_button(locale)) } }
                }
            }
        }
        @if can_delete {
            form method="post" action=(format!("{mutation_prefix}/comment_delete/{}", url_encode(repo_name))) class="inline-form" data-confirm=(t::delete_comment_confirm(locale)) {
                (PreEscaped(&csrf_html))
                input type="hidden" name="repo_uid" value=(repo_uid);
                input type="hidden" name="number" value=(number);
                input type="hidden" name="comment_id" value=(comment_id);
                button type="submit" { (t::delete_comment_button(locale)) }
            }
        }
    }
}
