//! Project-wide i18n: a hand-rolled message catalog, not a crate
//! (`fluent`/`rust-i18n`/etc. aren't anywhere in this tree, even
//! transitively — adopting one would be a genuinely new dependency
//! subtree, unlike promoting an already-resolved crate the way TOTP's
//! `hmac`/`sha1` were). The one real requirement — each locale orders its
//! own arguments independently, since word order differs per language —
//! is fully solved by [`msg`]: it expands one message into a real
//! `match Locale { ... }` whose arms are each their own independent
//! `format!(...)` call, not one Rust format string reused across locales.
//!
//! **Convention every [`msg`] block must follow: no `_ =>` wildcard arm.**
//! Rust's own match-exhaustiveness is what makes a locale missing a
//! message a *compile* error at that exact call site — a wildcard arm
//! would silently swallow that guarantee. Nothing enforces this
//! mechanically yet beyond this doc comment; see `src/lib.rs`'s own tests
//! for a grep-based backstop once more than one catalog file exists.
//!
//! Catalogs are split by concern, mirroring how the rest of `src/` is
//! already split: [`shared`] holds messages reused across 2+ call sites
//! (this is where a currently-duplicated Spanish literal collapses onto
//! one function) — more catalog modules (`web`, `hook`, `lfs`, `crypto`,
//! `cli`) are added as their first real message lands, not speculatively
//! ahead of time.
//!
//! This lands the mechanism end-to-end on exactly one real message first
//! (see [`shared::cas_conflict`]); the other ~999 Spanish literals across
//! the project convert incrementally over many follow-up commits, not in
//! one pass — the catalog is meant to grow one message at a time,
//! alongside whichever call site is being converted.

pub mod shared;
pub mod web;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Locale {
    Es,
    En,
}

impl Locale {
    /// The language every hardcoded string in this codebase (comments,
    /// error catalogs, and — as of the web-template-copy i18n phase — page
    /// chrome) is actually written in today. Was `Es` while only
    /// `i18n::shared`'s error catalog existed and page templates were raw
    /// Spanish literals with no `Locale` awareness at all; flipped to `En`
    /// once template copy got wired to `Locale` too (2026-08-03, user's
    /// call), since leaving it `Es` would have silently reverted every
    /// visitor without a `locale` cookie/account preference back to
    /// Spanish — the opposite of the already-live English default UI.
    pub const DEFAULT: Locale = Locale::En;

    /// Parses a `locale` cookie value or a stored account preference —
    /// `None` for anything unrecognized, same "malformed input isn't an
    /// error, just doesn't match" doctrine `TextSize::parse` already uses.
    pub fn parse(s: &str) -> Option<Locale> {
        match s {
            "es" => Some(Locale::Es),
            "en" => Some(Locale::En),
            _ => None,
        }
    }

    /// The BCP 47 tag, for `<html lang>` — what screen readers and
    /// hyphenation pick a language by.
    pub fn code(self) -> &'static str {
        match self {
            Locale::Es => "es",
            Locale::En => "en",
        }
    }
}

/// Defines one catalog entry as a real function taking `Locale` first, then
/// whatever arguments the message needs — see this module's own doc comment
/// for why every arm must be written explicitly (no `_` wildcard).
#[macro_export]
macro_rules! msg {
    ($(#[$meta:meta])* $name:ident($($arg:ident : $ty:ty),*) { $($locale:pat => $fmt:literal),+ $(,)? }) => {
        $(#[$meta])*
        pub fn $name(locale: $crate::Locale, $($arg: $ty),*) -> String {
            #[allow(unused_variables)]
            match locale {
                $($locale => format!($fmt),)+
            }
        }
    };
}
