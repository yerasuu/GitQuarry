//! Deterministic per-username SVG identicon — GitHub's original scheme
//! (a symmetric 5x5 grid, hashed from the seed), computed on every render
//! rather than stored anywhere: no upload, no fetch, no third party ever
//! sees a username or email to generate one. Gravatar was considered and
//! rejected early on — it leaks usernames/emails to a third party, cutting
//! against this project's everything-stays-encrypted posture — but a
//! self-hosted upload (`auth::web::avatar`, stored encrypted in this
//! project's own bucket, never a third party) doesn't have that problem, so
//! it exists alongside this as an opt-in override; this stays the fallback
//! for every account that never uploads one, see [`account_avatar`].

use maud::{Markup, html};
use sha2::{Digest, Sha256};

/// `seed` is typically a username — same seed always produces the same
/// icon, different seeds almost always differ (a 5x5 symmetric grid has
/// 2^15 possible patterns, plenty for visually distinguishing usernames in
/// a list, not for cryptographic uniqueness). No size here: the rendered
/// square takes its side length from the surrounding text (`.avatar` in
/// `base.css`), so it keeps its proportion to whatever name it sits beside
/// under both densities and all three text sizes. The one call site that
/// needs a fixed pixel size asks for it explicitly, via
/// [`account_avatar_sized`].
pub(crate) fn identicon(seed: &str) -> Markup {
    identicon_impl(seed, None)
}

/// `avatar_content_type`: the account's `User::avatar_content_type` if
/// already in hand — `Some` renders an `<img>` pointed at
/// `auth::web::avatar::handler_avatar_get` (`/_avatar/<username>`) instead
/// of the generated identicon. Callers that only have a bare username (no
/// loaded `User`, e.g. a repo list rendered from `RepoConfig` alone) keep
/// calling [`identicon`] directly rather than paying for a lookup just for
/// this — the identicon fallback is deliberately cheap enough that skipping
/// an avatar in those spots isn't a correctness issue, only a cosmetic one.
pub(crate) fn account_avatar(username: &str, avatar_content_type: Option<&str>) -> Markup {
    account_avatar_impl(username, avatar_content_type, None)
}

/// [`account_avatar`] at one explicit pixel size, for the single place an
/// avatar isn't sitting beside a name: the account settings page, where it
/// is the subject of the page rather than an ornament on a row. Everywhere
/// else takes the `em`-relative default, so the avatar keeps its proportion
/// to the name it belongs to under both densities and all three text sizes.
pub(crate) fn account_avatar_sized(username: &str, avatar_content_type: Option<&str>, size_px: u32) -> Markup {
    account_avatar_impl(username, avatar_content_type, Some(size_px))
}

fn account_avatar_impl(username: &str, avatar_content_type: Option<&str>, size_px: Option<u32>) -> Markup {
    let class = if size_px.is_some() {
        "avatar avatar-fixed"
    } else {
        "avatar"
    };
    match avatar_content_type {
        Some(_) => html! {
            img src=(format!("/_avatar/{}", crate::webui::url_encode(username)))
                width=[size_px] height=[size_px] class=(class) alt="";
        },
        None => identicon_impl(username, size_px),
    }
}

fn identicon_impl(seed: &str, size_px: Option<u32>) -> Markup {
    let digest = Sha256::digest(seed.as_bytes());
    let hue = u32::from(digest[0]) * 360 / 256;
    // Only the left 3 columns are hashed; columns 3-4 mirror columns 1-0,
    // same left-right symmetry GitHub's original identicons use.
    let mut cells = Vec::new();
    for row in 0..5u32 {
        for col in 0..3u32 {
            let bit_index = row * 3 + col;
            let byte = digest[1 + (bit_index / 8) as usize];
            let filled = (byte >> (bit_index % 8)) & 1 == 1;
            if filled {
                cells.push((row, col));
                if col != 2 {
                    cells.push((row, 4 - col));
                }
            }
        }
    }
    html! {
        svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 5 5" width=[size_px] height=[size_px]
            class=(if size_px.is_some() { "avatar avatar-fixed" } else { "avatar" })
            aria-hidden="true" focusable="false" {
            rect x="0" y="0" width="5" height="5" fill="var(--surface)";
            @for (row, col) in cells {
                rect x=(col) y=(row) width="1" height="1" fill=(format!("hsl({hue} 55% 45%)"));
            }
        }
    }
}
