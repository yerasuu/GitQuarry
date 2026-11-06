//! Struct definitions only — [`Peer`], [`AccessToken`] — plus each entity's
//! own `#[derive(Entity)]` storage shape (collection prefix, key field,
//! secondary indexes). Business logic lives in [`crate::access`].
//!
//! Both used to live in `gitquarry-core`'s `src/federation/entities.rs`;
//! this is a fresh implementation, not a moved file (see
//! `docs/federation-binary-split-concept.md` section 2 — deprecate, don't
//! migrate), but the storage shape is unchanged: `.conf/peers`/`.conf/tokens`
//! stay the same collection prefixes in the same bucket, only the owning
//! *code* moved crates.

use gitquarry_wire::Uid;
use s3db::Entity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// How much a linked peer is allowed to do (section 6, moderation layer) —
/// granular in place of the old single-binary predecessor's binary
/// linked/no-linked. Ordered (`BrowseOnly < BrowseAndFork < Full`) so a
/// call site can gate on "at least this tier" with a plain comparison.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerAccessLevel {
    BrowseOnly,
    BrowseAndFork,
    #[default]
    Full,
}

/// An admin-linked instance (federation layer 1): unidirectional, no
/// coordination with the other side — this instance "follows" it, same as
/// Mastodon following doesn't require the followed instance to do
/// anything. Never referenced by uid elsewhere (unlike `User`/`Group` in
/// core), so no `#[entity(index)]` field is needed: `name` is both the
/// display label and the S3 key segment.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Entity)]
#[entity(collection = ".conf/peers")]
pub struct Peer {
    #[entity(key)]
    pub name: String,
    /// `https://otherhost:8080` — for linking to the peer's own web UI and
    /// for the `/_api/instance`/`/_api/repos` round-trip.
    pub web_base_url: String,
    /// `https://otherhost` or `ssh://otherhost:2222` — for fetching a
    /// specific repo's content (federation layer 2), never used for the
    /// discovery API itself.
    pub git_base_url: String,
    /// `format_unix_secs`-formatted, same as `AccessToken::created_at` —
    /// display only, nothing keys off it.
    #[serde(default)]
    pub linked_at: String,
    /// `Full` for an admin/moderator manual add (`domain::peer::add`'s
    /// direct caller); `BrowseOnly` for a gossip auto-link
    /// (`gossip::run_auto_link_tick`) — corroborated-by-gossip is a weaker
    /// trust signal than an explicit admin decision, so auto-link only
    /// grants the minimum capability. A moderator can raise it afterward
    /// (`/_peers`' access-level control). `#[serde(default)]` makes an
    /// existing stored `Peer` (from before this field existed) come back
    /// as `Full` — every peer that could exist before this field shipped
    /// was, by definition, a manual add.
    #[serde(default)]
    pub access_level: PeerAccessLevel,
    /// Cuts off browse/fork/PR-proxy immediately regardless of
    /// `access_level` (section 6) — checked *before* `access_level` at
    /// every enforcement site, never a separate bypassable code path.
    #[serde(default)]
    pub banned: bool,
    /// Mandatory, always non-empty whenever `banned` is `true` — validated
    /// at the ban action itself (`domain::peer::ban`), never left as a
    /// loose `Option` elsewhere. Always public (`GET /_api/banned-peers`,
    /// no auth) — a ban's reason is never a private admin note.
    #[serde(default)]
    pub ban_reason: Option<String>,
    /// `format_unix_secs`-formatted, same convention as `linked_at`.
    #[serde(default)]
    pub banned_at: Option<String>,
    /// Username of the admin/moderator who banned this peer — public
    /// alongside `ban_reason` (`GET /_api/banned-peers`), so it's visible
    /// *who* made the call, not just why.
    #[serde(default)]
    pub banned_by: String,
    /// Overrides the instance-wide rate-limit ceiling for requests
    /// attributed to this specific peer (section 6) — `None` means "use
    /// the instance default". Attribution is IP-based and heuristic, never
    /// a cryptographic guarantee (see `rate_limit`'s own module doc
    /// comment): whoever shares this peer's outbound IP gets the same
    /// treatment.
    #[serde(default)]
    pub rate_limit_override: Option<u32>,
    /// `true` only when `domain::peer::add` was called from
    /// `gossip::run_auto_link_tick`, never from a manual `/_peers` add —
    /// distinct from `access_level` (which a moderator can freely raise or
    /// lower afterward) because sister-instance elevation eligibility
    /// (section 7) depends on *how* this peer was originally linked, not
    /// its current access level. `#[serde(default)]` makes a
    /// pre-this-field `Peer` come back `false` (manually linked), same
    /// "every peer that could exist before this field shipped was, by
    /// definition, a manual add" reasoning `access_level`'s own doc
    /// comment uses.
    #[serde(default)]
    pub auto_linked: bool,
    /// Admin toggled "become sisters" on *this* side — section 7's mutual
    /// consent requires both sides to independently set this before
    /// [`Self::sister_confirmed`] ever becomes `true` on either.
    #[serde(default)]
    pub sister_requested: bool,
    /// `true` only once both sides have confirmed — this is what actually
    /// activates ban-list sync (`domain::sister`), never `sister_requested`
    /// alone.
    #[serde(default)]
    pub sister_confirmed: bool,
    /// `None` if this instance banned `name` itself; `Some(sister_name)` if
    /// it arrived via that sister's ban-event push or the periodic
    /// reconciliation pull (section 7) — `banned_by`/`ban_reason` still
    /// hold the *original* actor/reason either way, this field only tracks
    /// provenance. `total_bans_issued` on `/_api/instance` counts only
    /// `None` here, so two sisters' identical, mirrored ban lists don't
    /// each inflate the other's accountability count for what is really
    /// one decision.
    #[serde(default)]
    pub synced_from_sister: Option<String>,
}

/// Fine-grained personal access token (federation layer 4) — created by a
/// user, scoped to an explicit repo list plus a `read`/`read+write`
/// ceiling. Never a permission grant of its own: the *real* access
/// decision is always made by `gitquarry-core` via `POST /_internal/authorize`,
/// which re-checks `owner`'s actual current permission on every request —
/// this token can't outlive or exceed whatever access `owner` genuinely has
/// at the moment it's used, and federation never even receives that
/// permission value, only a yes/no.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Entity)]
#[entity(collection = ".conf/tokens")]
pub struct AccessToken {
    #[serde(default)]
    #[entity(key)]
    pub uid: Uid,
    /// Username, not a uid — federation never resolves this to a `User`
    /// (it has no such type at all); it's forwarded verbatim as the
    /// `username` field of every `/_internal/*` call to core, which does
    /// the actual resolution.
    #[serde(default)]
    pub owner: String,
    /// Display-only label the owner chose ("peer A - browsing") — never
    /// parsed, never shown to anyone but `owner`.
    #[serde(default)]
    pub label: String,
    /// SHA256 hex of the secret, generated once at creation and never
    /// stored/shown again — see `access::tokens::generate_secret`. Fast
    /// unsalted digest, not Argon2: the secret is already 160 bits of
    /// randomness, never a human-chosen value, so a slow KDF buys nothing
    /// here and would tax every API request that validates a token.
    /// `#[entity(index)]` gets this struct a generated
    /// `find_by_token_hash`/`token_hash_claim_op`/`release_token_hash` for
    /// free — see `s3db-macros`.
    #[serde(default)]
    #[entity(index)]
    pub token_hash: String,
    /// Never empty and never "all repos" — the whole point of "fine-
    /// grained": a token lists exactly which repos (by core's own uid) it
    /// can touch.
    #[serde(default)]
    pub repos: BTreeSet<Uid>,
    /// `false` = read-only, capped at core's `Permission::Reporter`
    /// regardless of `owner`'s real tier. `true` = read+write, capped only
    /// at `owner`'s own real permission — never higher. The actual
    /// enforcement of this ceiling lives in whichever web handler calls
    /// `/_internal/authorize` (federation never computes a `Permission`
    /// itself, see this struct's own doc comment).
    #[serde(default)]
    pub write: bool,
    /// `format_unix_secs`-formatted, same as `Peer::linked_at`.
    #[serde(default)]
    pub created_at: String,
    /// `format_unix_secs`-formatted; lexicographically comparable against
    /// a fresh timestamp in the same format, no date parsing needed to
    /// check expiry.
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
}
