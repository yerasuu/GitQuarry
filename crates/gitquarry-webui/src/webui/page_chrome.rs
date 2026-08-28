//! Shared page chrome: the title bar, tabs bar, breadcrumbs, and the outer
//! `<html>` layout (plus its CSS) every page renders into.

use super::raw_link::raw_link;
use super::repo_link::repo_link;

/// The tab links themselves — the familiar ` | `-separated inline row above
/// `mobile.css`'s breakpoint, a plain stacked vertical list below it
/// (`.tabs-list a` forces `display: block`) either way. Shared by
/// [`tabs_bar`] and `merges`'s own status filter — duplicating this
/// list/separator markup risked one copy drifting (a missing
/// `aria-hidden`) in a way a shared helper can't.
///
/// `toggle`, when `Some((id, label))`, additionally wraps a hamburger
/// checkbox/label around the list so it starts *collapsed* on mobile,
/// `:checked` revealing it — worth the extra tap only for a row with
/// enough entries that showing them all plainly would otherwise eat a lot
/// of vertical space (the main tabs bar's 9). The status filter's 3
/// already read fine as a short always-visible stack, so it passes `None`:
/// a toggle there would hide content that "collapsing" was never actually
/// saving room on.
///
/// `id` must be unique *on the page* when given: more than one of these
/// can render together (the merge-request list shows the main tabs bar and
/// its own status filter at once), and `:checked ~` needs each row's own
/// checkbox/list pair matched unambiguously — not shared with a sibling
/// row's.
pub(crate) fn tab_links(toggle: Option<(&str, &str)>, tabs: &[(&str, String, String)], active: &str) -> maud::Markup {
    maud::html! {
        @if let Some((id, label)) = toggle {
            input type="checkbox" id=(id) class="tabs-toggle-input";
            label for=(id) class="tabs-toggle-btn" aria-label=(label) { (super::icons::menu()) }
        }
        span class="tabs-list" {
            @for (i, (tab_id, href, label)) in tabs.iter().enumerate() {
                @if i > 0 {
                    // `aria-hidden`, own element (not a bare text node): a
                    // vertical mobile list hides these via CSS, which only
                    // works on something with its own box to hide — a raw
                    // `" | "` text node has none.
                    span class="tab-sep" aria-hidden="true" { " | " }
                }
                a class=[(active == *tab_id).then_some("active")] aria-current=[(active == *tab_id).then_some("page")] href=(href) { (label) }
            }
        }
    }
}

/// The tabs bar itself — the row [`header_bar`] and [`admin_header`] both
/// need below their title bar. `tabs` is `(id-to-match-against-active,
/// href, label)`; pulled out so the two callers can't drift into slightly
/// different markup for what's meant to be the same tabs-bar chrome.
/// `right`: pre-rendered HTML (star/text-size buttons, currently only
/// [`header_bar`] uses this) for a right-aligned cell in the same row —
/// empty skips the cell entirely rather than rendering an empty one.
#[cfg(any(test, feature = "test-util"))]
pub fn tabs_bar(
    tabs: &[(&str, String, String)],
    active: &str,
    right: &maud::Markup,
    nav_label: &str,
    toggle_label: &str,
) -> maud::Markup {
    maud::html! {
        // `role="navigation"`: this is a real nav menu wearing a `<table>`
        // for its visual layout — without the role override, screen readers
        // announce it as a data table (row/column counts) instead of a nav
        // landmark reachable via landmark-jump commands.
        table class="tabs" role="navigation" aria-label=(nav_label) {
            tr {
                td { (tab_links(Some(("tabs-toggle", toggle_label)), tabs, active)) }
                @if !right.0.is_empty() {
                    td class="tabs-right" { (right) }
                }
            }
        }
    }
}
#[cfg(not(any(test, feature = "test-util")))]
fn tabs_bar(
    tabs: &[(&str, String, String)],
    active: &str,
    right: &maud::Markup,
    nav_label: &str,
    toggle_label: &str,
) -> maud::Markup {
    maud::html! {
        table class="tabs" role="navigation" aria-label=(nav_label) {
            tr {
                td { (tab_links(Some(("tabs-toggle", toggle_label)), tabs, active)) }
                @if !right.0.is_empty() {
                    td class="tabs-right" { (right) }
                }
            }
        }
    }
}

/// Looks up the display label for whichever `id` is currently `active` —
/// used for the "GitQuarry / {label}" title so the breadcrumb shows
/// translated text instead of the (deliberately untranslated, English-only)
/// tab id.
fn active_tab_label<'a>(tabs: &'a [(&str, String, String)], active: &'a str) -> &'a str {
    tabs.iter()
        .find(|(id, _, _)| *id == active)
        .map(|(_, _, label)| label.as_str())
        .unwrap_or(active)
}

/// The red-underlined "GitQuarry / {title}" title table, shared by
/// [`header_bar`] and [`admin_header`] — `atom_link` and `sub_rows` are the
/// only two things that differ between a repo page and an admin page.
/// `owner`: the repo's namespace slug (user or group, whichever it is —
/// `/_profile/<slug>` already resolves either) linked in place of the
/// site-brand home link `header_bar` used to put there — the standalone
/// "owner: ..." row this replaced is gone, not moved elsewhere. `None` for
/// [`admin_header`] (not a repo page — nothing to link there), which keeps
/// the old "GitQuarry / <label>" home-link behavior unchanged.
///
/// `ref_switcher`: pre-rendered cgit-style branch dropdown, right-aligned
/// next to the title — replaces the old plain "@ <rev>" sub-row with
/// something that also lets a viewer switch branches from wherever they
/// are, not just from the refs tab. `None` for the same non-repo pages
/// `owner` is `None` for, and for a repo page with nothing to switch
/// between (see [`header_bar`]'s own `ref_switcher` construction).
fn title_bar(
    title: &str,
    owner: Option<&str>,
    badge: Option<maud::Markup>,
    atom_link: Option<&str>,
    ref_switcher: Option<maud::Markup>,
    sub_rows: maud::Markup,
) -> maud::Markup {
    maud::html! {
        // `role="presentation"`: `h1` above already carries the real
        // heading semantics, so this table is pure visual layout — without
        // the override, screen readers would additionally announce it as a
        // 1-row data table on top of the heading.
        table class="header" role="presentation" {
            tr {
                td class="main" {
                    h1 {
                        @if let Some(owner) = owner {
                            a href=(format!("/_profile/{}", super::escaping::url_encode(owner))) { (owner) }
                        } @else {
                            a href="/" { "GitQuarry" }
                        }
                        " / " (title)
                    }
                    @if let Some(b) = badge { (b) }
                }
                @if let Some(r) = ref_switcher {
                    td class="ref-switch-cell" { (r) }
                }
                @if let Some(l) = atom_link {
                    td class="atom" { a href=(l) { "atom" } }
                }
            }
            (sub_rows)
        }
    }
}

/// Same two-bar treatment [`header_bar`] gives repo pages (red-underlined
/// title, then a gray tabs bar right below it) — for the admin pages that
/// aren't about a repo (`/_settings`, `/_users`). Without the tabs bar these
/// looked like a different, unfinished site: just a heading bolted onto the
/// topnav with nothing marking where chrome ends and content starts. `active`
/// is `"settings"` or `"users"`, same active-tab convention as [`header_bar`].
pub(crate) fn admin_header(active: &str, locale: gitquarry_i18n::Locale) -> String {
    use gitquarry_i18n::web::chrome;
    let tabs: &[(&str, String, String)] = &[
        (
            "settings",
            "/_settings/footer".to_string(),
            chrome::settings_tab(locale),
        ),
        ("users", "/_users".to_string(), chrome::users_tab(locale)),
        ("groups", "/_groups".to_string(), chrome::groups_tab(locale)),
        ("audit", "/_audit".to_string(), chrome::audit_tab(locale)),
    ];
    maud::html! {
        (title_bar(active_tab_label(tabs, active), None, None, None, None, maud::html! {}))
        (tabs_bar(tabs, active, &maud::html! {}, &chrome::page_tabs_nav_label(locale), &chrome::page_tabs_toggle_label(locale)))
    }
    .into_string()
}

/// Section ids/hrefs/labels for [`account_settings_page`]'s sidebar —
/// `"profile"`/`"appearance"`/`"security"`/`"keys"`/`"favorites"`/`"pat"`,
/// each a top-level `/_account/<id>` route (no username in the URL: unlike
/// the public [`crate::auth::web::profile`] page, every one of these only
/// ever operates on the logged-in session's own account, resolved
/// server-side — there's nothing here a URL segment could usefully name).
fn account_sections(locale: gitquarry_i18n::Locale) -> [(&'static str, &'static str, String); 6] {
    use gitquarry_i18n::web::chrome;
    [
        ("profile", "/_account/profile", chrome::account_profile_section(locale)),
        (
            "appearance",
            "/_account/appearance",
            chrome::account_appearance_section(locale),
        ),
        ("security", "/_account/security", chrome::security_tab(locale)),
        ("keys", "/_account/keys", chrome::keys_tab(locale)),
        ("favorites", "/_account/favorites", chrome::favorites_tab(locale)),
        ("pat", "/_account/pat", chrome::pat_tab(locale)),
    ]
}

/// GitHub-style settings panel: a left sidebar naming every section
/// ([`account_sections`]), the given section's own content to its right —
/// replaces the old shared-tab-bar-with-the-public-profile-page treatment
/// ([`admin_header`]'s sibling this used to be) now that these pages have
/// their own `/_account/*` url space instead of living under
/// `/_profile/<user>/*`. `content` is each page's own body markup; this is
/// the whole page body (sidebar + content together), not a `header` slot
/// like [`admin_header`]/[`header_bar`] return — the two-column flex layout
/// needs both sides in one unbroken markup tree, not a `header`/`body`
/// string pair [`layout`] concatenates without knowing about the grid
/// between them.
pub(crate) fn account_settings_page(
    active: &str,
    content: maud::Markup,
    locale: gitquarry_i18n::Locale,
) -> maud::Markup {
    use gitquarry_i18n::web::chrome;
    let sections = account_sections(locale);
    // The page's own `h2` comes from the same tuple the sidebar entry does,
    // so the two can never drift apart and no page needs a second copy of
    // its own title as a separate message. Without it every account page
    // opened straight into an `h3`/`h4` with no heading above it, leaving
    // the section unnamed for anyone not reading the sidebar's active row.
    let title = sections
        .iter()
        .find(|(id, _, _)| *id == active)
        .map(|(_, _, label)| label.clone())
        .unwrap_or_default();
    maud::html! {
        div class="settings-layout" {
            nav class="settings-sidebar" aria-label=(chrome::settings_sidebar_nav_label(locale)) {
                ul {
                    @for (id, href, label) in &sections {
                        li {
                            a class=[(active == *id).then_some("active")] aria-current=[(active == *id).then_some("page")] href=(href) { (label) }
                        }
                    }
                }
            }
            div class="settings-content" {
                h2 class="settings-title" { (title) }
                (content)
            }
        }
    }
}

/// One settings block: a card with its own `h3`, optional explanatory
/// prose, and whatever form/table `body` is. Every `/_account/*` page (and
/// the admin `/_settings`) is a stack of these rather than a flat run of
/// `h4` + bare `<form>` — a long page like `security` was otherwise a wall
/// of controls with nothing marking where one concern ended and the next
/// began.
///
/// `heading` is an `h3` because [`account_settings_page`] already owns the
/// page's `h2`: the old pages started at `h4` with nothing above them,
/// skipping two levels for anyone navigating by headings.
pub(crate) fn settings_section(heading: &str, prose: Option<&str>, body: maud::Markup) -> maud::Markup {
    maud::html! {
        section class="card card-pad settings-section" {
            h3 { (heading) }
            @if let Some(prose) = prose {
                p class="muted" { (prose) }
            }
            (body)
        }
    }
}

/// One labelled control: `label` above it, hint below when there is one.
/// The `div.field` markup predates this (`/_settings`, the MR forms) but
/// had no CSS rule at all, so it rendered as a bare block and every account
/// page skipped labels entirely in favour of placeholders — which vanish on
/// first keystroke, aren't announced as labels, and give no click target.
pub(crate) fn field(label: &str, hint: Option<&str>, control: maud::Markup) -> maud::Markup {
    maud::html! {
        div class="field" {
            label { (label) }
            (control)
            @if let Some(hint) = hint {
                span class="hint" { (hint) }
            }
        }
    }
}

/// Bundles [`header_bar`]'s less-central params — same reasoning as
/// [`crate::auth::web::groups::PageChrome`] (not in scope here, see that struct's
/// own doc comment): adding `locale` as an 8th positional arg would trip
/// clippy's `too_many_arguments`.
pub(crate) struct HeaderExtras<'a> {
    /// The repo's free-text public name (`RepoConfig::display_name`) — shown
    /// as the page heading in place of the raw slug when set. `None` for a
    /// repo not yet backfilled (`domain::repo::backfill_slug`) or predating
    /// the field, in which case [`header_bar`] falls back to `bare_name`.
    pub(crate) display_name: Option<&'a str>,
    pub(crate) description: Option<&'a str>,
    pub(crate) archived: bool,
    pub(crate) visibility: gitquarry_core::storage::entities::Visibility,
    /// Branch names for the cgit-style ref switcher next to the title —
    /// empty for views with no meaningful "current branch" to switch away
    /// from (settings, the error-retry paths, an empty repo). Names only
    /// (not [`gitquarry_core::git::browse::RefInfo`]): the switcher just
    /// needs something to link, not a sha/subject/date per branch.
    pub(crate) branches: &'a [String],
    pub(crate) menu_extra: &'a str,
    /// Pre-rendered markup slotted between the title and the tabs — empty
    /// for every view except the ones that actually have something to put
    /// there (the summary view's language bar in `airy` density, mockup-
    /// positioned above the tabs rather than in the sidebar). Kept as an
    /// explicit per-call opt-in instead of always-present chrome so views
    /// that don't set it don't pay for computing whatever it would've shown.
    pub(crate) below_header: &'a str,
    /// Pre-rendered markup slotted *after* the main tabs bar — a sub-nav
    /// row, not the summary language bar `below_header` above is for (that
    /// one sits between the title and the tabs on purpose, mockup-
    /// positioned; a sub-nav reads backwards up there, it has to follow the
    /// tab it belongs to). Same "empty for every view but the ones that
    /// need it" opt-in as `below_header`.
    pub(crate) sub_tabs: &'a str,
    pub(crate) locale: gitquarry_i18n::Locale,
}

/// Title bar + tabs, cgit-style: repo name on top, tabs below (active tab
/// highlighted), small non-tab links (atom feed) off to the side.
pub(crate) fn header_bar(repo_name: &str, rev: &str, active: &str, extras: HeaderExtras<'_>) -> String {
    // `owner` (settings/tags/releases still read it off `HeaderExtras`
    // for their own page body) isn't read here anymore — the title below
    // now gets its owner-or-group segment straight from `repo_name` itself
    // (always `<namespace>/<repo>`, see [`gitquarry_core::validate`]'s own
    // invariant), not from this separately-resolved display name.
    let HeaderExtras {
        display_name,
        description,
        archived,
        visibility,
        branches,
        menu_extra,
        below_header,
        sub_tabs,
        locale,
    } = extras;
    use gitquarry_core::storage::entities::Visibility;
    use gitquarry_i18n::web::chrome;
    let mut tab_defs: Vec<(&str, String)> = vec![
        ("about", chrome::about_tab(locale)),
        ("summary", chrome::summary_tab(locale)),
        ("refs", chrome::refs_tab(locale)),
        ("log", chrome::log_tab(locale)),
        ("tree", chrome::tree_tab(locale)),
        ("stats", chrome::stats_tab(locale)),
        ("releases", chrome::releases_tab(locale)),
    ];
    tab_defs.push(("merges", chrome::merges_tab(locale)));
    tab_defs.push(("settings", chrome::settings_tab(locale)));
    let tabs: Vec<(&str, String, String)> = tab_defs
        .iter()
        .map(|(view, label)| {
            let rev_for_link = if matches!(*view, "settings" | "releases" | "merges") {
                ""
            } else {
                rev
            };
            (
                *view,
                repo_link(repo_name, view, rev_for_link).to_string(),
                label.clone(),
            )
        })
        .collect();
    let atom_link = raw_link("atom", repo_name, "").to_string();

    let badge = match visibility {
        Visibility::Private => Some(
            maud::html! { span class="badge" { (super::icons::lock()) " " (chrome::private_visibility_badge(locale)) } },
        ),
        Visibility::Limited => Some(
            maud::html! { span class="badge" { (super::icons::lock()) " " (chrome::limited_visibility_badge(locale)) } },
        ),
        Visibility::Public => None,
    };
    let sub = maud::html! {
        @if archived {
            tr { td class="sub" colspan="2" { span class="badge" { (super::icons::archive()) " " (chrome::archived_notice(locale)) } " " (chrome::archived_notice_detail(locale)) } }
        }
        @if let Some(desc) = description && !desc.is_empty() {
            tr { td class="sub" colspan="2" { (desc) } }
        }
    };
    // cgit-style: the current branch is a switcher, not plain text — shown
    // only when there's actually a rev and more than the one branch to
    // switch to (a single-branch repo has nothing to switch between).
    let ref_switcher = (!rev.is_empty() && !branches.is_empty()).then(|| {
        maud::html! {
            details class="dropdown ref-switch" {
                summary { (super::icons::branch()) " " (rev) }
                div class="dropdown-menu" {
                    @for name in branches {
                        a class=[(name == rev).then_some("active")] href=(repo_link(repo_name, active, name).to_string()) { (name) }
                    }
                }
            }
        }
    });
    let menu_extra = maud::PreEscaped(menu_extra.to_string());
    let below_header = maud::PreEscaped(below_header.to_string());
    let sub_tabs = maud::PreEscaped(sub_tabs.to_string());
    // `repo_name` is always `<namespace>/<repo>` (see `title_bar`'s own doc
    // comment on `owner`) — split once rather than plumbing a second,
    // separately-resolved owner string through just for this.
    let (owner_slug, bare_name) = repo_name.rsplit_once('/').unwrap_or(("", repo_name));
    let owner_slug = (!owner_slug.is_empty()).then_some(owner_slug);
    let title = display_name.unwrap_or(bare_name);
    maud::html! {
        (title_bar(title, owner_slug, badge, Some(&atom_link), ref_switcher, sub))
        (below_header)
        (tabs_bar(&tabs, active, &menu_extra, &chrome::page_tabs_nav_label(locale), &chrome::page_tabs_toggle_label(locale)))
        (sub_tabs)
    }
    .into_string()
}

/// `display_name`: the repo's `RepoConfig::display_name`, rendered as the
/// first crumb in place of the slug the crumb's own href still uses — same
/// split [`header_bar`] makes for the title above it.
///
/// `leaf_view`: view to use for the link of the last segment (the actual
/// file/folder being shown) — "tree" while navigating folders, "blob" while
/// viewing a file. Intermediate segments are always folders.
pub(crate) fn breadcrumbs(
    repo_name: &str,
    display_name: Option<&str>,
    rev: &str,
    path: &str,
    leaf_view: &str,
    locale: gitquarry_i18n::Locale,
) -> String {
    use gitquarry_i18n::web::chrome;
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    let last = segments.len().saturating_sub(1);
    let mut acc = String::new();
    let crumbs: Vec<(String, &str)> = segments
        .into_iter()
        .enumerate()
        .map(|(i, seg)| {
            if i > 0 {
                acc.push('/');
            }
            acc.push_str(seg);
            let view = if i == last { leaf_view } else { "tree" };
            (repo_link(repo_name, view, rev).path(&acc).to_string(), seg)
        })
        .collect();
    // `<nav><ol>`, not `<p><span>`: a screen reader's landmark-jump and
    // list-navigation commands both need real semantics here, not a
    // paragraph of inline spans that happens to look like a path.
    maud::html! {
        nav aria-label=(chrome::breadcrumb_nav_label(locale)) {
            ol class="breadcrumb" {
                li class="breadcrumb-item" {
                    a href=(repo_link(repo_name, "tree", rev).to_string()) { (super::repo_label(repo_name, display_name)) }
                }
                @for (i, (href, seg)) in crumbs.iter().enumerate() {
                    li class="breadcrumb-sep" aria-hidden="true" { "/" }
                    @if i == last {
                        li class="breadcrumb-item current" aria-current="page" { (seg) }
                    } @else {
                        li class="breadcrumb-item" { a href=(href) { (seg) } }
                    }
                }
            }
        }
    }
    .into_string()
}

/// CSS class for a permission-tier pill — `Guest`/`Reporter`/`Developer`/
/// `Maintainer` still use `.decoration` + a `-tier` modifier (see
/// `style/base.css`'s escalating-grayscale reasoning, kept as gitquarry's
/// own: `styler` has no graduated-severity badge variant). `Owner`, the
/// top tier, now uses `styler`'s `.badge.accent` directly instead of the
/// plain `.decoration` it used to share with topic pills/admin labels —
/// those moved to `.badge.accent` too, so this keeps the same visual
/// weight without depending on a class this file no longer owns for that
/// purpose.
pub(crate) fn permission_badge_class(perm: gitquarry_core::auth::entities::Permission) -> &'static str {
    use gitquarry_core::auth::entities::Permission;
    match perm {
        Permission::Guest => "decoration decoration-guest",
        Permission::Reporter => "decoration decoration-reporter",
        Permission::Developer => "decoration decoration-developer",
        Permission::Maintainer => "decoration decoration-maintainer",
        Permission::Owner => "badge accent",
    }
}

/// The translated name of a [`gitquarry_core::auth::entities::Permission`]
/// tier — shared by every page that shows a tier to a reader
/// (`auth::web::groups`' member rows, `auth::web::group`'s member list), so
/// the five words are translated in exactly one place.
pub(crate) fn permission_label(
    perm: gitquarry_core::auth::entities::Permission,
    locale: gitquarry_i18n::Locale,
) -> String {
    use gitquarry_core::auth::entities::Permission;
    match perm {
        Permission::Guest => gitquarry_i18n::web::auth::groups::groups_permission_guest(locale),
        Permission::Reporter => gitquarry_i18n::web::auth::groups::groups_permission_reporter(locale),
        Permission::Developer => gitquarry_i18n::web::auth::groups::groups_permission_developer(locale),
        Permission::Maintainer => gitquarry_i18n::web::auth::groups::groups_permission_maintainer(locale),
        Permission::Owner => gitquarry_i18n::web::auth::groups::groups_permission_owner(locale),
    }
}

/// Each topic as a small link to `/?q=<topic>` — reuses the repo index's own
/// search box as the filter (it already matches topics, not just names), so
/// there's no separate `?topic=` param to validate/parse. Shared by the
/// index and `/_profile`, the two places a repo's topics show up.
#[cfg(any(test, feature = "test-util"))]
pub fn topic_badges(topics: &std::collections::BTreeSet<String>) -> String {
    maud::html! {
        @for t in topics {
            " " a class="badge accent" href=(format!("/?q={}", super::escaping::url_encode(t))) { (t) }
        }
    }
    .into_string()
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn topic_badges(topics: &std::collections::BTreeSet<String>) -> String {
    maud::html! {
        @for t in topics {
            " " a class="badge accent" href=(format!("/?q={}", super::escaping::url_encode(t))) { (t) }
        }
    }
    .into_string()
}

/// A `[data-copy]` button — `style/app.js`'s delegated click handler writes
/// `value` to the clipboard and briefly swaps the button's own text to
/// `data-copied-label`, no other glue needed. Only worth reaching for where
/// the value is otherwise awkward to select by hand in full (a PAT secret,
/// not a short SHA already sitting right there as visible text).
pub(crate) fn copy_button(value: &str, locale: gitquarry_i18n::Locale) -> maud::Markup {
    maud::html! {
        // `.secondary`, not styler's default solid-accent `.btn`: the copy
        // action is a minor utility, not the primary thing on the page —
        // same reasoning `actions.rs`'s star/watch buttons use.
        button type="button" class="btn secondary copy-btn" data-copy=(value)
            data-copied-label=(gitquarry_i18n::shared::copied_label(locale)) {
            (gitquarry_i18n::shared::copy_button(locale))
        }
    }
}

/// One `:root`-scoped block of CSS custom-property declarations per theme —
/// `src/web/style/{light,dark}.css`, each just the variable list, no
/// selector wrapper — plus `base.css`, the actual component styles, which
/// only ever reference `var(--x)` and so never need to change when a theme
/// is added. Adding a third theme later (a user-chosen accent, a
/// high-contrast mode, ...) is one more `.css` file plus one more
/// `[data-theme='...']` block below, not a rewrite of this constant or of
/// `base.css`.
///
/// `dense.css`/`airy.css` are a second, independent axis (spacing/font-size/
/// radius/line-height/letter-spacing — see [`super::density_pref`]), not a
/// color scheme, so it composes more simply than theme: `dense.css` is just
/// part of the plain `:root` defaults (no `prefers-*` media query exists for
/// density), and `airy.css` only needs the one explicit-override block.
///
/// `mobile.css` is a third axis (screen size, not user-chosen like the two
/// above — see its own doc comment) — full rules under its own
/// `@media (max-width: 640px)`, not bare custom-property declarations like
/// the theme/density files, so it needs no wrapper here and just goes last:
/// after `base.css`, so equal-specificity ties between the two resolve in
/// its favor regardless of whichever theme/density is also active.
///
/// Composed with `concat!`/`include_str!` (both compile-time), not a runtime
/// `format!`: `STYLE` stays a plain `&'static str`, no allocation on every
/// response for what's static content.
///
/// Default (no explicit pick) still follows `prefers-color-scheme`;
/// [`layout`]'s `<html data-theme>` — set from the `theme` cookie
/// ([`super::theme_pref::handler_toggle_theme`]) or the account default
/// ([`super::theme_pref::effective_theme`]) — overrides that in either
/// direction, no client script needed: `[data-theme=dark]` forces dark even
/// in a light OS, and `:not([data-theme=light])` inside the media query
/// keeps the OS's dark mode from winning once someone's explicitly picked
/// light.
///
/// `style/vendor/styler.css` — a pinned build of the shared `styler`
/// framework (`ssh://localhost-git:2222/yerasuu/styler`, tag `v0.2.0`;
/// re-vendor with that repo's own `scripts/vendor.sh` after a tag bump, no
/// Cargo-native pin exists for a non-crate asset) — goes *first*, so every
/// rule below it wins any same-specificity collision by source order alone
/// (e.g. this file's own `.btn`/`table.list` over styler's `.btn`/`table`):
/// existing pages stay pixel-identical, while everything styler has that
/// this file doesn't (toast, modal, accordion, ...) becomes available for
/// later, page-by-page adoption. `light.css`/`dark.css` alias a handful of
/// styler's own token names (`--text`, `--ok`, `--warn`, ...) to values
/// already defined here, so those components render in this exact
/// palette instead of styler's own default one.
///
/// Served as its own `/_assets/style.css` (see
/// `webui::handler_style_css`), not spliced into `<head>`: at ~150KB it was
/// 96% of every HTML response, re-sent on every navigation and re-brotli'd
/// per request — measured at +21ms on a page that otherwise rendered in
/// 1.8ms. As a file it compresses once, caches for a year against its own
/// `ETag`, and lets the CSP drop `style-src 'unsafe-inline'`.
pub(crate) const STYLE: &str = concat!(
    include_str!("style/vendor/styler.css"),
    ":root {\n",
    include_str!("style/light.css"),
    include_str!("style/dense.css"),
    "}\n",
    "@media (prefers-color-scheme: dark) {\n",
    "  :root:not([data-theme='light']) {\n",
    include_str!("style/dark.css"),
    "  }\n",
    "}\n",
    ":root[data-theme='dark'] {\n",
    include_str!("style/dark.css"),
    "}\n",
    ":root[data-density='airy'] {\n",
    include_str!("style/airy.css"),
    "}\n",
    include_str!("style/base.css"),
    include_str!("style/mobile.css"),
    // Last: the per-render utility classes (widths, language colours,
    // `display:inline`) a view picks by name instead of writing a `style`
    // attribute the CSP would have to allow. See the file's own header.
    include_str!("style/utilities.css"),
);

/// General cgit/kernel.org-style layout: sans-serif, dense tables with thin
/// borders, dark red accent, monospace only in code/diff blocks.
///
/// `topnav_auth`: pre-rendered login-state snippet (from
/// [`crate::auth::web::login::topnav`]), already HTML — empty string when OIDC
/// isn't configured at all, matching this page's look from before login
/// existed.
///
/// `theme`: [`super::theme_pref::effective_theme`]'s result —
/// `"dark"`/`"light"`/`""`, written straight onto `<html data-theme>` so
/// [`STYLE`]'s CSS picks it up before first paint (no flash of the wrong
/// theme waiting on JS that doesn't exist here anyway).
///
/// `density`: [`super::density_pref::effective_density`]'s result —
/// `"dense"`/`"airy"`/`""` — same mechanism, a second independent axis (see
/// [`STYLE`]'s doc comment).
///
/// `data-accent="crimson"` is static, not a preference — always on, no
/// cookie/account default/toggle — it's this project's identity (matches
/// the mockup's brand red, and the red `--accent` already in
/// `light.css`/`dark.css`), not a per-viewer pick the way theme/density
/// are. Briefly `"teal"`: this preset's `--accent-500` sits close to
/// `vendor/styler.css`'s own `--danger-500`, which once made the active
/// nav tab and a form-validation error read as near-identical reds — but
/// `light.css`/`dark.css`'s own `--danger` stays deliberately un-aliased
/// to `--accent` (see their own comments), so the two are still visually
/// distinct where it matters most (that alias is what actually caused the
/// original collision, not this preset choice alone).
/// Who the copyright line names, and from when. Constants rather than a
/// setting: this is the project's own notice, not something a deploy
/// customises — a fork changes it here, in the same commit it changes the
/// name everywhere else.
/// The quarry-block mark, as one `<path d>`: the topbar's brand link draws
/// it inline (filled by CSS, so it follows the accent), and
/// `webui::handler_favicon_svg` wraps this same string in a standalone SVG
/// document for the tab icon. One copy — a redrawn logo can't end up
/// matching in the header and stale in the tab.
pub(crate) const BRAND_MARK_PATH: &str = "M12 1.5 2 7v10l10 5.5L22 17V7L12 1.5Zm0 2.3 7.6 4.18L12 12.1 4.4 7.98 12 3.8Zm-8 5.86 7.2 3.95v7.7L4 17.2V9.66Zm9.2 11.65v-7.7L20.4 9.66v7.54l-7.2 4.11Z";

const COPYRIGHT_HOLDER: &str = "Yerasuu";
const COPYRIGHT_YEAR: &str = "2026";

/// The footer's "source" link (`ServerSettings::source_link`, already
/// resolved to an href — see [`resolve_source_link_href`]) — a
/// process-wide `OnceLock`, not a `layout` parameter: `layout` renders on
/// every single page (~35 call sites across the crate), and this one link
/// changes only when an admin edits `/_settings` and the process restarts
/// to apply it (same "restart to apply" contract every other `ServerSettings`
/// field already has), so it isn't worth threading a new argument through
/// every one of them. `gitquarry-server`'s `serve::run` calls
/// [`init_source_link`] once at startup, right after resolving
/// `ServerSettings`; unset (tests, or before that runs) reads as "no link".
static SOURCE_LINK: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Called once at startup with the already-resolved href — see
/// [`resolve_source_link_href`]. A second call (there shouldn't be one) is
/// silently ignored rather than panicking: a `OnceLock` staying at whatever
/// value it first got is a safer failure mode for page chrome than crashing
/// the server over it.
pub fn init_source_link(href: Option<String>) {
    let _ = SOURCE_LINK.set(href);
}

/// `ServerSettings::source_link`'s raw stored value (either an absolute
/// `http(s)://` URL or a `namespace/name` repo path) resolved to what
/// actually belongs in `href=`. Pulled out of [`init_source_link`]'s call
/// site so it's independently testable without a real `OnceLock` write.
pub fn resolve_source_link_href(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        Some(raw.to_string())
    } else {
        Some(format!("/{raw}"))
    }
}

pub fn layout(
    title: &str,
    header: &str,
    body: &str,
    topnav_auth: &str,
    theme: &str,
    density: &str,
    locale: gitquarry_i18n::Locale,
) -> String {
    use gitquarry_i18n::web::chrome;
    maud::html! {
        (maud::DOCTYPE)
        html lang="en"
            data-theme=[(!theme.is_empty()).then_some(theme)]
            data-density=[(!density.is_empty()).then_some(density)]
            data-accent="crimson" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " - GitQuarry" }
                link rel="icon" type="image/svg+xml" href="/_assets/favicon.svg";
                link rel="stylesheet" href=(super::style_css_url());
                script src="/_assets/styler.js" defer {}
                script src="/_assets/app.js" defer {}
            }
            body data-error-generic=(chrome::async_request_failed(locale)) {
                a class="skip-link" href="#content" { (chrome::skip_to_content(locale)) }
                div id="topbar" {
                    div class="inner" {
                        span class="brand" {
                            a href="/" {
                                svg viewBox="0 0 24 24" aria-hidden="true" {
                                    path d=(BRAND_MARK_PATH) {}
                                }
                                "GitQuarry"
                            }
                        }
                        // Pure-CSS collapse (no JS in this codebase at all):
                        // a hidden checkbox + its `<label>` as the visible
                        // hamburger toggle, `nav` shown via the sibling
                        // `:checked ~ nav` selector below `base.css`'s
                        // existing 640px breakpoint — full-width `nav` stays
                        // as-is above it.
                        input type="checkbox" id="nav-toggle" class="nav-toggle-input";
                        label for="nav-toggle" class="nav-toggle-btn" aria-label=(chrome::menu_toggle_label(locale)) { (super::icons::menu()) }
                        nav {
                            a href="/" { (chrome::repos_nav(locale)) } " · "
                            a href="/_new" { (chrome::new_repo_nav(locale)) } " · "
                            a href="/_groups" { (chrome::groups_tab(locale)) }
                            (maud::PreEscaped(topnav_auth))
                            " · "
                            a href="/_settings/footer" { (chrome::settings_tab(locale)) }
                            " · "
                            (super::theme_pref::render_theme_toggle(theme))
                            " · "
                            (super::density_pref::render_density_toggle(density))
                        }
                    }
                }
                main id="content" tabindex="-1" {
                    // A message parked by `flash::redirect` on the response
                    // before this one. Rendered here rather than by each
                    // handler so a mutation only has to redirect somewhere
                    // sensible to have its outcome shown.
                    (maud::PreEscaped(super::flash::render_current(locale)))
                    (maud::PreEscaped(header))
                    (maud::PreEscaped(body))
                }
                // Outside `main`, so "skip to content" lands on the page's
                // own content and a screen reader reaching the end of it
                // isn't handed the same three links on every page.
                footer class="site-footer" {
                    span { (chrome::footer_copyright(locale, COPYRIGHT_YEAR, COPYRIGHT_HOLDER)) }
                    @if let Some(link) = SOURCE_LINK.get().and_then(|v| v.as_deref()) {
                        " · "
                        a href=(link) { (chrome::source_nav(locale)) }
                    }
                    " · "
                    a href="/_license" rel="license" { "AGPL-3.0-or-later" }
                    " · "
                    a href="/_changelog" { (chrome::changelog_nav(locale)) " " (env!("CARGO_PKG_VERSION")) }
                }
            }
        }
    }
    .into_string()
}
