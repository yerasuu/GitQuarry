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
/// bearing: a user whose group tier and legacy team grants disagree on a
/// repo gets the highest one via `.max()` in `effective_permission`, and every
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
    /// other entities (`RepoConfig::owner`, `Group::members`, ...) will
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
    /// Manage linked federation peers (`/_peers`: link/unlink/ban/
    /// access-level) without full `is_admin` — a separate, narrower
    /// delegation (`docs/federation-binary-split-concept.md` section 6).
    /// `#[serde(default)]` so existing stored records deserialize as
    /// `false`, same pattern `is_admin` itself would need if added today.
    #[serde(default)]
    pub is_federation_moderator: bool,
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
    pub(crate) fn fingerprints(&self) -> impl Iterator<Item = String> + '_ {
        self.pubkeys.iter().filter_map(|k| key_fingerprint(k).ok())
    }
}

/// A long-lived, locally-scoped credential for git-smart-HTTP clone/push —
/// separate from federation's own fine-grained `AccessToken` (`.conf/tokens`,
/// owned by `gitquarry-federation`, resolved via loopback): this one lives
/// entirely in core, so it works with no `[federation]` config at all.
/// `scopes` is never empty — same "explicit repos only, never all" rule
/// federation's tokens follow — but carries a full [`Permission`] ceiling
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
    /// working, same reasoning as [`Group::members`].
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
/// `<group>/<repo>` the same way a personal one lives at
/// `<username>/<repo>`. Membership is direct and GitLab-shaped:
/// [`Self::members`] maps a user uid to the one [`Permission`] they hold on
/// *every* repo in the group, with no per-repo entry for any of them.
/// [`Permission::Owner`] in that map is also what "may run the group
/// itself" means (see `access::can_manage_group`), so there is no separate
/// owner list beside it.
///
/// [`Self::name`] is the group's *full path* (`parent/child` for a
/// subgroup), which is simultaneously its repo namespace and its own
/// storage key — so the ancestor chain of any group is readable straight off
/// the name (`access::ancestor_group_names`) instead of walking
/// [`Self::parent`] pointers on a permission check, which is a hot path.
/// `parent` is still stored: it keys the child-group index
/// (`access::add_child_group_index`), which is the only way to enumerate a
/// group's subgroups without scanning every group in the bucket.
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
    /// User uid → the tier that user holds across the whole group, not
    /// usernames: a deleted user simply stops resolving (via
    /// `access::resolve_user_uid`) instead of a stale username silently
    /// matching whichever *different* user later claims that name.
    #[serde(default)]
    pub members: BTreeMap<Uid, Permission>,
    /// The enclosing group's uid, `None` for a top-level group. Only ever
    /// set at creation (`auth::domain::group::create`) — there is no
    /// move-subgroup flow, since [`Self::name`] carries the path and moving
    /// one would mean rewriting every descendant's name, namespace claim and
    /// owned repos' `full_name` in one non-atomic sweep.
    #[serde(default)]
    pub parent: Option<Uid>,
    /// Same cap as [`User::storage_quota_bytes`], for every repo this group
    /// owns instead of a personal namespace. Not inherited by a subgroup,
    /// which owns its own repos: `access::quota::check` compares against
    /// whichever group actually owns the repo being pushed to.
    #[serde(default)]
    pub storage_quota_bytes: Option<u64>,
    /// Pre-[`Self::members`] membership, kept only so that upgrading a
    /// bucket doesn't silently revoke everyone: [`Self::role_of`] still
    /// honors it, and `migrations::group_members` (applied automatically at
    /// server startup) folds it into `members` — subset-scoped teams become
    /// explicit per-repo grants on each member's own `User::repos`, the only
    /// lossless place for them — then clears both fields. Renamed rather than dropped so every
    /// call site that used to read them is a compile error instead of
    /// silently reading an empty default.
    #[serde(default, rename = "owners", skip_serializing_if = "BTreeSet::is_empty")]
    pub legacy_owners: BTreeSet<Uid>,
    /// See [`Self::legacy_owners`].
    #[serde(default, rename = "teams", skip_serializing_if = "Vec::is_empty")]
    pub legacy_teams: Vec<Team>,
}

impl Group {
    /// `user_uid`'s tier across every repo in this group — the direct
    /// [`Self::members`] entry, raised by whatever the legacy fields still
    /// grant: a `legacy_owners` entry reads as [`Permission::Owner`], and a
    /// legacy team covering *every* repo in the group (`Team::repos` unset)
    /// reads as that team's own tier. A legacy team scoped to a subset of
    /// repos is deliberately absent here — group-wide is not what it
    /// granted; see [`Self::legacy_repo_role`].
    pub fn role_of(&self, user_uid: &Uid) -> Option<Permission> {
        let legacy_owner = self.legacy_owners.contains(user_uid).then_some(Permission::Owner);
        let legacy_team = self
            .legacy_teams
            .iter()
            .filter(|t| t.repos.is_none() && t.members.contains(user_uid))
            .map(|t| t.permission)
            .max();
        [self.members.get(user_uid).copied(), legacy_owner, legacy_team]
            .into_iter()
            .flatten()
            .max()
    }

    /// Every user with a group-wide tier here, [`Self::members`] raised by
    /// whatever the legacy fields still grant — what the group page renders
    /// and what the `group-members` data migration writes back into `members`. A user
    /// whose only legacy grant is a subset-scoped team is deliberately
    /// absent: they hold no group-wide tier at all (see
    /// [`Self::legacy_repo_role`]).
    pub fn effective_members(&self) -> BTreeMap<Uid, Permission> {
        let mut out = self.members.clone();
        let legacy = self
            .legacy_owners
            .iter()
            .chain(self.legacy_teams.iter().flat_map(|t| t.members.iter()));
        for uid in legacy {
            // `role_of` already folds this user's own `members` entry in, so
            // whatever it returns is never lower than what's already there.
            if let Some(perm) = self.role_of(uid) {
                out.insert(uid.clone(), perm);
            }
        }
        out
    }

    /// What the legacy subset-scoped teams alone grant `user_uid` on one
    /// specific repo — the half of [`Self::legacy_teams`] that
    /// [`Self::role_of`] can't express, since it applies to `repo_uid` and
    /// nothing else in the group.
    pub fn legacy_repo_role(&self, user_uid: &Uid, repo_uid: &Uid) -> Option<Permission> {
        self.legacy_teams
            .iter()
            .filter(|t| t.members.contains(user_uid))
            .filter(|t| t.repos.as_ref().is_some_and(|set| set.contains(repo_uid)))
            .map(|t| t.permission)
            .max()
    }
}

/// Pre-[`Group::members`] membership: a subset of a group's members sharing
/// one [`Permission`] on either every repo in the group (`repos: None`) or a
/// specific list of them (`repos: Some(..)`, repo uids). Nothing creates
/// these any more — see [`Group::legacy_teams`] for why they're still read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    /// User uids — see [`Group::members`]'s doc comment for why not usernames.
    #[serde(default)]
    pub members: BTreeSet<Uid>,
    pub permission: Permission,
    #[serde(default)]
    pub repos: Option<BTreeSet<Uid>>,
}
