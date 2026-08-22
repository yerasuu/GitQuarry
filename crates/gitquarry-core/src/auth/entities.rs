//! Struct definitions only — [`Permission`], [`User`], [`Team`], [`Group`] —
//! plus each entity's own `#[derive(Entity)]` storage shape (collection
//! prefix, key field, secondary indexes). Business logic (permission
//! resolution, index claims/lookups, CRUD wrappers) lives in
//! [`super::access`], which re-exports everything here so existing
//! `crate::auth::access::User`-style paths keep working.

use crate::auth::access::key_fingerprint;
use crate::storage::uid::Uid;
use s3db::Entity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// Access level for a repo, low to high. `Ord` (declaration order) is load-
/// bearing: a user in multiple teams that disagree on a repo's permission
/// gets the highest one via `.max()` in `effective_permission`, and every
/// `>=`/`<` gate elsewhere (push, release upload, settings management)
/// compares against this same order.
///
/// `Guest` grants no code-read rights at all — a placeholder for future
/// issue/wiki-only access (no such feature exists yet, so `Guest` currently
/// unlocks nothing; that's intentional, not a bug). `Reporter` is read-only,
/// `Developer` adds push — the old `Read`/`Write`, kept as `serde` aliases so
/// already-stored `.conf/users/*.json`/`.conf/groups/*.json` data keeps
/// deserializing under its old field names. `Maintainer` can manage this
/// one repo's settings; `Owner` can additionally delete/transfer it and
/// grant/revoke any tier on it — see `can_manage_repo_settings` and
/// `can_manage_repo_members`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Guest,
    #[serde(alias = "read")]
    Reporter,
    #[serde(alias = "write")]
    Developer,
    Maintainer,
    Owner,
}

/// An ASCII-armored OpenPGP public key plus its fingerprint, computed once
/// at [`crate::auth::domain::user::add_gpg_key`] time (via `gitquarry_gpg::fingerprint`,
/// which needs the real `gpg` binary — unlike [`crate::auth::access::key_fingerprint`]
/// for SSH keys, there's no pure-Rust way to derive one here) and persisted
/// alongside the key so nothing needs to re-invoke `gpg` just to render or
/// remove-by-fingerprint a key someone already added.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpgKeyEntry {
    pub armored: String,
    pub fingerprint: String,
}

/// A named identity: zero or more authorized SSH pubkeys, and per-repo
/// permissions that apply regardless of which of its keys authenticated the
/// connection.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Entity)]
#[entity(collection = ".conf/users")]
pub struct User {
    /// Stable id, minted once at `add_user` time and never changed — what
    /// other entities (`RepoConfig::owner`, `Group::owners`, ...) will
    /// reference instead of `username`, so a future rename can't strand
    /// them. Resolved back to the current `username` via [`Self::UID_INDEX`]
    /// (`.conf/users.index/uid/<uid>.json`), see `access::resolve_user_uid`.
    #[serde(default)]
    #[entity(index)]
    pub uid: Uid,
    #[entity(key)]
    pub username: String,
    /// Immutable once set at `add_user` time (no rename/set-email flow
    /// exists) — only `auth::domain::user::create` ever claims [`Self::EMAIL_INDEX`],
    /// via `email_claim_op`.
    #[serde(default)]
    #[entity(index(segment = "base64"))]
    pub email: Option<String>,
    /// Self-service additional emails, each independently claimed via
    /// `access::claim_secondary_email` (`access::SECONDARY_EMAIL_INDEX`) —
    /// same hand-rolled-index reasoning [`Self::pubkeys`]' own claim/release
    /// pair uses instead of `#[entity(index)]`: the derive's one-field-one-
    /// index model can't express "many values, each independently claimed"
    /// the way a `BTreeSet` needs. Unlike `email` (immutable, OIDC's own
    /// identity key), these exist purely so `auth::access::resolve_by_any_email`
    /// (commit-author-email matching for avatars/GPG verification) isn't
    /// stuck with only the one address a login-relevant `email` has to be.
    #[serde(default)]
    pub secondary_emails: BTreeSet<String>,
    /// Full openssh lines (`ssh-ed25519 AAAA... comment`). The comment is
    /// only so a human can identify the key in the UI; comparison against
    /// the key an SSH client offers is by fingerprint, not this raw string.
    #[serde(default)]
    pub pubkeys: Vec<String>,
    /// Self-service, same idea as `pubkeys` but for verifying release-asset
    /// signatures (`domain::release::verify_asset_signature`), not for
    /// authenticating a connection. No global uniqueness claim the way
    /// `pubkeys` needs (`access::claim_pubkey`): nothing resolves *who this
    /// is* from a GPG key the way an incoming `ssh://` connection resolves
    /// from a pubkey fingerprint, so two accounts happening to list the same
    /// key is harmless.
    #[serde(default)]
    pub gpg_keys: Vec<GpgKeyEntry>,
    /// Explicit per-repo grants, by repo uid — a deleted repo's stale entry
    /// simply never matches again (uids are never reused), rather than a
    /// stale name silently re-granting access to whatever unrelated new
    /// repo later claims that name.
    #[serde(default)]
    pub repos: BTreeMap<Uid, Permission>,
    /// Default permission for any repo not listed explicitly in `repos` —
    /// was a `"*"` sentinel key in that same map, split into its own field
    /// once `repos` became uid-keyed: no `Uid` value is safely reservable
    /// as an "obviously never a real repo" sentinel the way the string
    /// `"*"` was.
    #[serde(default)]
    pub default_permission: Option<Permission>,
    /// Argon2id PHC string (from [`gitquarry_auth_crypto::password::hash_password`]),
    /// never the raw password. `None`: this user can't log into the web UI
    /// with a password — still usable via SSH pubkey and/or an OIDC email
    /// that matches, same as before this field existed.
    #[serde(default)]
    pub password_hash: Option<String>,
    /// Bumped whenever this account's credentials change in a way that
    /// should not leave old sessions standing (today: `domain::user::set_password`).
    /// Every session cookie records the value current when it was issued
    /// (`auth::web::login::SessionCookie::epoch`) and
    /// `auth::web::login::current_user` rejects any cookie whose value no
    /// longer matches.
    ///
    /// The point is that sessions here are stateless sealed cookies with a
    /// 7-day TTL and no server-side store to delete from — without this
    /// counter, changing a password (the thing people do precisely because
    /// they think someone else has their session) would leave that someone
    /// else logged in for the rest of the week. `#[serde(default)]` so
    /// stored records predating the field read as `0`, which is also what a
    /// cookie predating it deserializes to: existing sessions stay valid
    /// across the upgrade and only a later bump invalidates them.
    #[serde(default)]
    pub session_epoch: u64,
    /// Full access to `/_settings`, `/_users`, every repo's "settings" tab,
    /// and every `private` repo regardless of `repos`/`access::effective_permission_for`
    /// — the bucket operator role, distinct from per-repo read/write. Only
    /// meaningful for the web UI; `ssh://` already gates per-repo via
    /// `repos`/group membership regardless of this flag.
    #[serde(default)]
    pub is_admin: bool,
    /// Repo uids this user starred — purely a personal bookmark list, no
    /// bearing on `repos`/visibility/anything else. Doesn't need to stay in
    /// sync with reality: a starred repo that gets deleted just fails to
    /// render on `/_profile/<user>/favorites` and is skipped, and — unlike
    /// when this stored a name — can never end up silently pointing at a
    /// different, unrelated repo that later reuses the old name.
    #[serde(default)]
    pub starred: BTreeSet<Uid>,
    /// Repo uids this user watches — same "personal bookmark, doesn't need
    /// to stay in sync with reality" tolerance as `starred`. Purely a
    /// visible watcher-count/social signal today: no notification delivery
    /// exists in this codebase, so watching a repo doesn't yet make
    /// anything happen beyond the toggle and the count.
    #[serde(default)]
    pub watched: BTreeSet<Uid>,
    /// Account-level default for the summary tab's text-size toggle (see
    /// `crate::webui::effective_text_size`) — `"sm"`/`"md"`/`"lg"`, or empty
    /// for "no personal pick yet". Only consulted when this session has no
    /// explicit `text-size` cookie override, so a logged-in user's own pick
    /// always wins over the global default without needing to re-click the
    /// toggle every session.
    #[serde(default)]
    pub text_size_default: String,
    /// Account-level default theme (see `crate::webui::effective_theme`) —
    /// `"dark"`/`"light"`, or empty for "no personal pick yet, follow
    /// `prefers-color-scheme`". Same "cookie beats account default beats
    /// nothing" precedence as `text_size_default`.
    #[serde(default)]
    pub theme_default: String,
    /// Account-level default density (see `crate::webui::effective_density`)
    /// — `"dense"`/`"airy"`, or empty for "no personal pick yet". Unlike
    /// `theme_default`, empty has no OS-level signal to follow — it just
    /// means the site default, dense. Same "cookie beats account default
    /// beats nothing" precedence as `theme_default`.
    #[serde(default)]
    pub density_default: String,
    /// `false` (default, private) unless this user opted in — when `true`,
    /// `/_profile/<user>/favorites` is readable by anyone, not just this
    /// user's own session, and `gitquarry_webui::auth::web::profile` links to it from the
    /// public profile page. Only ever flips via this user's own
    /// `/_profile/<user>/preferences` form, same self-service-only scope as
    /// `text_size_default`/`theme_default`.
    #[serde(default)]
    pub favorites_public: bool,
    /// Base32-encoded TOTP secret, set the moment enrollment starts —
    /// `None`: 2FA was never set up (or was disabled). `Some` while
    /// `totp_enabled` is still `false` means enrollment is pending: the
    /// secret/QR has been shown but no correct code has been submitted back
    /// yet, so `gitquarry_webui::auth::web::login` must not require it. Only
    /// `auth::domain::user::enable_totp`/`confirm_totp`/`disable_totp` ever touch
    /// either of these two fields.
    #[serde(default)]
    pub totp_secret: Option<String>,
    /// `true` only once a submitted code has verified against `totp_secret`
    /// — this, not `totp_secret.is_some()`, is what login gates on, so an
    /// abandoned enrollment (secret generated, never confirmed) can never
    /// turn into an unexpected login lockout.
    #[serde(default)]
    pub totp_enabled: bool,
    /// Hashed (Argon2id, same as `password_hash`) single-use TOTP recovery
    /// codes — the self-service break-glass path for a lost device, so
    /// `auth::domain::user::disable_totp`'s own doc comment ("this app has no
    /// other account-recovery flow either") stops being true. Populated
    /// once `confirm_totp` actually turns 2FA on (never during the pending-
    /// enrollment window `totp_secret.is_some() && !totp_enabled` covers),
    /// re-populated wholesale by `regenerate_recovery_codes`, cleared by
    /// `disable_totp`. A code is removed from this list the moment
    /// `consume_recovery_code` accepts it — one-time use, never replayable.
    #[serde(default)]
    pub totp_recovery_codes: Vec<String>,
    /// Highest 30s TOTP step already spent on a login, so the same code
    /// can't be used twice. A code is valid across its own step and one
    /// either side (`totp::SKEW_STEPS`), which without this leaves an
    /// observed code replayable for as long as that window lasts.
    /// `#[serde(default)]` — `0` for every account stored before this
    /// existed, which accepts any step from now on and starts tracking.
    #[serde(default)]
    pub totp_last_step: u64,
    /// Account-level default locale (`"es"`/`"en"`, or empty for "no
    /// personal pick yet") — see `crate::webui::effective_locale`. Same
    /// "cookie beats account default beats built-in default" precedence as
    /// `theme_default`/`text_size_default`.
    #[serde(default)]
    pub locale_default: String,
    /// Admin-set cap, in bytes, on the summed `RepoConfig::size_bytes` of
    /// every repo this user owns — `None` means unlimited (the default: no
    /// quota unless an admin sets one). Checked by
    /// `auth::access::quota::check` at the pre-receive hook and LFS upload;
    /// see that module's own doc comment for why it's a soft, already-over
    /// gate rather than predicting an incoming push's exact size.
    #[serde(default)]
    pub storage_quota_bytes: Option<u64>,
    /// MIME type of this user's uploaded avatar image (`"image/png"` /
    /// `"image/jpeg"` / `"image/webp"` — the only three
    /// `validate::sniff_avatar_content_type` accepts), or `None` if this
    /// user never uploaded one, in which case the UI falls back to a
    /// generated identicon instead. Set only by
    /// `auth::domain::user::set_avatar`/`remove_avatar`; the actual image
    /// bytes live in the bucket under `avatars/<username>`
    /// ([`crate::app::AppContext::avatar_remote`]), not in this entity —
    /// same "small JSON entity, large blob on `Remote`" split release assets
    /// use.
    #[serde(default)]
    pub avatar_content_type: Option<String>,
}

impl User {
    /// Fingerprints of this user's pubkeys. A pubkey string that fails to
    /// parse is skipped rather than erroring the whole user out — it just
    /// can't match anything, same as if it weren't there.
    // pub, not pub(crate): also used directly by the integration tests in
    // `tests/auth_access_pubkey.rs`, which link against this crate as an
    // external consumer and can't see pub(crate) items. Genuine production
    // callers (`access::pubkey::resolve_by_fingerprint`, `claim_pubkey`)
    // need this unconditionally, so it isn't cfg-gated like a pure
    // test-only accessor would be.
    pub fn fingerprints(&self) -> impl Iterator<Item = String> + '_ {
        self.pubkeys.iter().filter_map(|k| key_fingerprint(k).ok())
    }
}

/// A long-lived, locally-scoped credential for git-smart-HTTP clone/push.
/// `scopes` is never empty (explicit repos only, never all) and carries a
/// full [`Permission`] ceiling
/// per repo instead of a single read/write bool, since core already has
/// [`Permission`]'s 5 tiers and [`User::repos`] already uses this exact map
/// shape. The ceiling never grants more than `owner_uid`'s real, current
/// permission — see `git::http::authorize_impl`, which re-derives that on
/// every request and takes the lower of the two.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Entity)]
#[entity(collection = ".conf/pat")]
pub struct PersonalAccessToken {
    #[serde(default)]
    #[entity(key)]
    pub uid: Uid,
    /// [`User::uid`], not username — a renamed owner keeps the token
    /// working, same reasoning as [`Group::owners`].
    #[serde(default)]
    pub owner_uid: Uid,
    /// Display-only, chosen by the owner — never parsed.
    #[serde(default)]
    pub label: String,
    /// SHA256 hex of the secret (`gqpat_<40 hex>`), generated once at
    /// creation and never stored/shown again — see `access::pat::hash_token`.
    /// Fast unsalted digest, not Argon2: same reasoning as federation's
    /// `AccessToken::token_hash` — the secret is already 160 bits of
    /// randomness, never human-chosen, so a slow KDF buys nothing and would
    /// tax every git-smart-HTTP request that validates a token.
    #[serde(default)]
    #[entity(index)]
    pub token_hash: String,
    /// Never empty. Repo uid → the `Permission` ceiling this token grants on
    /// that repo — the actual enforcement (never exceeding `owner_uid`'s own
    /// current permission) happens in `git::http::authorize_impl`.
    #[serde(default)]
    pub scopes: BTreeMap<Uid, Permission>,
    /// `format_unix_secs`-formatted.
    #[serde(default)]
    pub created_at: String,
    /// `format_unix_secs`-formatted; lexicographically comparable against a
    /// fresh timestamp in the same format, no date parsing needed to check
    /// expiry.
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

/// A shared namespace multiple users can push into — a repo lives at
/// `<group-name>/<repo>` the same way a personal one lives at
/// `<username>/<repo>`. Repo access flows through [`Team`]s, not a flat
/// member list: a `Team` grants its members one `Permission` on either
/// every repo in the group or an explicit subset, so different teams can
/// see different slices of the same group (Forgejo calls this the same
/// thing). `owners` is separate from team membership — it's about managing
/// the group itself (see `access::can_manage_group_for`), not repo access;
/// an owner isn't automatically on any team, but gets implicit `Owner`
/// everywhere in the group regardless — see `access::effective_permission_for`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Entity)]
#[entity(collection = ".conf/groups")]
pub struct Group {
    /// Same idea as [`User::uid`] — minted once at `add_group` time,
    /// resolved back to the current `name` via [`Self::UID_INDEX`]
    /// (`.conf/groups.index/uid/<uid>.json`), see `access::resolve_group_uid`.
    #[serde(default)]
    #[entity(index)]
    pub uid: Uid,
    #[entity(key)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// User uids, not usernames — a deleted user simply stops resolving
    /// (via `access::resolve_user_uid`) instead of a stale username silently
    /// matching whichever *different* user later claims that name.
    #[serde(default)]
    pub owners: BTreeSet<Uid>,
    #[serde(default)]
    pub teams: Vec<Team>,
    /// Same cap as [`User::storage_quota_bytes`], for every repo this group
    /// owns instead of a personal namespace.
    #[serde(default)]
    pub storage_quota_bytes: Option<u64>,
}

/// A subset of a [`Group`]'s members sharing one [`Permission`] on either
/// every repo in the group (`repos: None`) or a specific list of them
/// (`repos: Some(..)`, repo uids — no longer names relative to the group's
/// own namespace: a uid is already globally unique, so there's no "local to
/// this group" scheme left to keep).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    /// User uids — see [`Group::owners`]'s doc comment for why not usernames.
    #[serde(default)]
    pub members: BTreeSet<Uid>,
    pub permission: Permission,
    #[serde(default)]
    pub repos: Option<BTreeSet<Uid>>,
}
