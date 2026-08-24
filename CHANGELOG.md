# Changelog

This changelog covers user-facing features added since the project's first
commit. It does not attempt to enumerate every internal engineering step
(refactors, renames, bug fixes, performance work, or test changes) — only
functionality a repo admin or end user would notice. Entries are grouped by
area rather than by date or version, since the project has not yet settled
on a per-release versioning scheme.

## Git hosting & transport

- Commits this server builds for you — a merge request's merge, squash or
  rebase commit — are signed with a key the instance generates and keeps for
  itself. They show as verified, in a colour distinct from a signature made
  by an account's own key: the instance can only attest that it built the
  commit, never who wrote it.

- Serve git repositories directly from S3-compatible object storage, with no
  separate git server process required.
- Embedded SSH server with public-key authentication, later reworked around
  named user accounts with per-repo permissions.
- Added git smart-HTTP as a supported transport. The earlier plaintext
  `git://` daemon and the direct S3 remote-helper were both removed, leaving
  `ssh://` and smart-HTTP as the only supported transports.
- Git LFS support: batch API plus large-object storage and transfer.

## Web UI & browsing

- cgit-inspired browsing UI covering summary, refs, tree, blob, log, commit
  diff, blame, and stats views.
- Compare view between two arbitrary refs.
- Commit signature verification badge. It now says only what this instance
  can stand behind: verified by a registered account's key, verified as
  built by this instance, or unverified. The old grades taken from git's own
  check against the server's system keyring ("valid but untrusted",
  "invalid", "revoked") are gone — this process never populates that
  keyring, so they asserted more than had actually been checked. A verdict
  not yet computed also shows as unverified until it is.
- A commit whose diff runs past the page's ceiling renders the part that
  fits and links to the raw patch for the rest.
- A file's syntax highlighting can arrive on the second view: the first
  render of a long file serves it unstyled rather than blocking on the
  highlighter, and highlights it in the background.
- Create-tag form on the refs tab.
- Markdown rendering gained syntax-highlighted fenced code blocks and
  GitHub-style alert blockquotes (note/tip/important/warning/caution) in
  READMEs and release notes.
- Light/dark theme with a session toggle and a per-account default.
- GitHub-style right sidebar on the repo summary page (about, community,
  releases).
- Local identicon avatars, an inline icon set, and a collapsible mobile
  navigation.
- Per-language byte-size bar on the repo summary page.
- HTML rendering migrated from ad hoc string templates to a typed templating
  engine (maud) across the entire UI.
- Site footer carrying the copyright, the licence and the running version,
  linking to the changelog the binary itself was built from (`/_changelog`).
- Instance-level source-link setting shown in the footer.
- Repo index page: a stats strip, an activity/clone sidebar with a
  browse-by-topic list, a per-card ssh clone hint, and a card/compact-table
  density toggle. New-repo button next to the search form; pickaxe logo mark
  and a real groups link on the sticky topbar.
- Home page's recent-activity rail now describes push/release/MR events
  instead of raw repo names, and shows each repo's last-push time — a merge
  landing on the default branch now counts as a push there too.
- Settings, account, admin and destructive actions (deletes, MR close/merge)
  ask for confirmation before submitting, and submit without a full page
  reload.

## Internationalization

- Full English/Spanish bilingual UI, driven by a locale-aware message and
  template catalog threaded through every page.
- Per-account default language, set from account settings — the session-only
  locale override still takes precedence when set, same precedence order as
  theme/density.

## Auth & permissions

- The admin pages are closed on a fresh instance too. Until a login method
  exists there is no session to check, so opening them needs a token the
  process prints at startup, exchanged once at `/_bootstrap`. It dies with
  the process and stops working the moment the first account has a password.
- Repeated wrong passwords slow an account's login attempts down rather than
  locking it, so knowing a username isn't enough to keep its owner out.
- Account settings reorganized into profile (avatar and email addresses),
  security (password, two-factor, recovery codes) and keys (SSH and GPG),
  instead of one flat page mixing all three. Every settings page now uses
  the same card layout with labelled fields.
- Changing your own password asks for it twice, so a typo can't lock you out
  of an account with no other way in.
- An account can change its own primary email, but only where OIDC isn't
  configured — with OIDC that address decides which account a login lands
  on, so it stays admin-managed.

- OIDC login alongside local admin and local user accounts, with a unified
  admin page design.
- Self-service account page where users manage their own SSH keys and
  password.
- Per-repo SSH permission grid in repo settings, with per-user add/remove.
- Five-tier permission ladder (guest/reporter/developer/maintainer/owner),
  gating repo settings management to maintainers and owners.
- Two-factor authentication (TOTP): enrollment/disable UI, login
  enforcement, and an admin-initiated disable path for account recovery.
- Security hardening: CSRF double-submit tokens on authenticated forms,
  standard security response headers (CSP, X-Frame-Options, etc.), and
  per-username login rate limiting.
- Groups drop the old team abstraction for direct members with a role, and
  can now nest as subgroups under an existing group. A permission granted on
  a parent group reaches every subgroup and repo underneath it.

## Repos & organizations

- Create empty repositories from the web UI.
- Per-repo public/private/limited visibility.
- Groups: shared namespaces with inherited permissions, Forgejo-style group
  owners/teams, and repo transfer between namespaces.
- Editable group description page.
- Default branch selector, backed by the repo's real HEAD.
- Delete a repo from its settings tab, confirmed by typing its name.
- Repo settings reorganized into separate tabs (general, default branch,
  protected branches/tags, signed commits, topics, required reviewers,
  labels, LFS limit, permissions, mirrors, move, danger zone) instead of one
  long stacked page — same treatment account settings already had.
- Move a repo to a new namespace or name.
- Archive a repo: archived repos reject pushes and are badged throughout the
  UI.
- Protected branches and protected tags.
- Starred repos, with a dedicated favorites page and an opt-in public
  favorites list.
- Repo forking, an activity feed, and star/fork counts.
- Repo topics for discoverability.
- Watched repos alongside starred repos, with a watch button.
- Public user profile page listing a user's repos.
- Merge requests can be merged by fast-forward, which creates no commit at
  all: what lands is the author's own commits, carrying the author's own
  signatures. Offered alongside merge, squash and rebase, and greyed out
  once the branches have diverged.
- A merge request that can't merge cleanly says so on its own page, listing
  the conflicting files and the commands to resolve them locally, instead of
  turning the merge button into an error page.
- A merged MR still renders after its source branch is deleted.
- Repos dashboard split into personal, group and granted-access sections.
- A repo can carry a free-text display name alongside its URL slug, shown
  everywhere a repo is listed or titled; existing repos get one backfilled
  automatically the first time they're listed.

## External mirroring

- Mirror a repo to GitHub, GitLab or Codeberg — push, pull or both — with
  per-ref status on the repo's settings tab. Only real fast-forwards sync;
  a genuine divergence is reported and never forced, and a branch deleted
  on one side is not re-created from the other.
- What a mirror pulls in passes the same checks a push does: protected
  branches and tags, required signatures, the LFS blob ceiling and the
  owner's storage quota.
- Optional two-way merge request sync per mirror — title, description,
  open/closed state, merges and comments — with the bookkeeping that keeps
  a comment from echoing between the two sides.
- An instance-wide host allowlist decides where a mirror may point. The
  forges with a built-in client are always allowed; a self-hosted GitLab or
  Gitea is added in the instance settings.
- Configuring or hand-syncing a mirror needs the repo's owner tier and is
  recorded in the audit log.

## Releases & assets

- The create-release form suggests the repo's own tags, minus the ones that
  already have a release.
- Releases: create, edit, and delete releases with notes; list/detail views;
  and attach or download binary assets.

## Storage & encryption

- Everything stored in the bucket is encrypted at rest
  (Argon2id-derived XChaCha20Poly1305) under a mandatory per-bucket
  passphrase — there is no way to disable it.
- Pluggable storage backends: S3, Cloudflare R2, a local filesystem driver,
  Azure Blob Storage, and Google Cloud Storage.
- First-run setup wizard, including storage-driver selection and a
  double-entry admin password confirmation.

## Backup & disaster recovery

- Replicate the whole bucket to another bucket at the same keys, or snapshot
  it as a versioned point-in-time copy — configurable per-instance and
  scheduled, or triggered once by hand.
- Retention pruning on the backup destination, so scheduled runs don't grow
  it unbounded.
- `gitquarry backup`/`gitquarry restore` operator commands, and a matching
  backup settings section in the web UI to configure and trigger it.
- `default_bucket` in `gitquarry.toml` names the bucket the instance serves,
  so extra `[buckets.*]` sections can exist purely as backup destinations or
  mirror targets without every command needing `--bucket`.

## Search

- Instance-wide search across filenames and commit messages.

## Storage engine (s3db)

- Typed `Collection`, `SecondaryIndex`, and `Singleton` primitives for
  structured, queryable object storage.
- Versioning and multi-key create-only transactions, backed by a
  write-ahead log with crash recovery.
- Caching, a resilience/retry decorator stack, and sharded indexes for
  scale.
- Logical backup/restore, background garbage collection, and point-in-time
  reads.
- `derive(Entity)` macro that generates Collection/index boilerplate from
  struct definitions.

## Ops & admin tooling

- `trust_forwarded_for` in the config, for a deployment behind a reverse
  proxy: the anonymous git rate limit keys on the forwarded client address
  instead of the proxy's own. Off unless the proxy owns that header.
- `gitquarry fsck` to detect stray objects in a repo.
- `gitquarry repack` to consolidate a repo's packs.
- `gitquarry migrate` to run pending bucket data-version migrations by hand;
  the server applies the same migrations to itself automatically at startup.
- Audit log of admin and repo actions, now including a `repo.push` entry for
  every real git push, not just webui-driven actions.
- Idle- and size-based eviction of local repo mirrors.
- Graceful shutdown.
- Server settings editable from the web UI and stored in the bucket, with
  conflict detection to prevent concurrent overwrites.
- Name filter on the admin users/groups pages.
