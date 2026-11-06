# Changelog

Every release of GitQuarry, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
SemVer as it reads below 1.0: in `0.y.z`, `y` is the breaking slot. For a
server, "breaking" means CLI flags, `gitquarry.toml` keys, ssh/HTTP routes or
the bucket layout.

Each entry says what an operator or user will notice. The technical record —
modules, functions, constants, deploy steps — is in
[`tag-changelog/vX.Y.Z.md`](tag-changelog/), which is also what `/_changelog`
serves for the running version.

## [0.6.0] — 2026-10-15

### Breaking

- The Azure Blob Storage and Google Cloud Storage drivers are gone:
  `driver = "azure"` or `"gcs"` in `gitquarry.toml` no longer loads. Move
  such a bucket to S3, R2 or the local driver with v0.5.1 before upgrading.
- s3db's write spool no longer lives in `./local` under whatever directory a
  process happened to start in: by default each bucket gets its own
  directory under the user's data dir (`~/.local/share/gitquarry/<bucket>`
  on Linux), and a relative `resilience.local_dir` resolves against the
  directory holding `gitquarry.toml`. To keep writes still pending in the
  old place, point `local_dir` at it.
- A filesystem (`path:`) backup destination still in the old `files` format
  must be converted with `s3db-local-migrate <dir>` before the next backup.
- Release assets are stored under a new name per upload. Assets uploaded
  before keep working, but an older version can't serve the ones uploaded by
  this one, so don't roll back once you've added assets.
- Federation: a sister handshake is now verified by calling the other
  instance back at the address registered for it. Both instances need this
  version and `own_web_base_url`/`own_git_base_url` set in
  `gitquarry-federation.toml`. Sisters already confirmed keep working.

### Added

- Passkeys. Register them from `/_account/security` — a physical security
  key, or this device, a password manager or wallet, or a phone — and sign
  in with one alone from `/_login`, or use one instead of the TOTP code.
  They turn on once the instance's public URL is set in
  `/_settings/network`.
- Federation is back as an optional module: a separate
  `gitquarry-federation` process for peer discovery, browsing and forking
  repos from other instances, cross-instance merge requests and
  fine-grained tokens, enabled with a `[federation]` section in
  `gitquarry.toml`.
- External mirrors can sync only the branches matching a list of glob
  patterns. Tags always sync.
- `gitquarry repack --max-pack-size` caps each output pack, 1 GiB by default.
- `gitquarry serve --mirrors-large-repo-threshold` (also an instance
  setting, default 1G): local mirrors at least that large are never evicted
  for age, only for the total size cap.
- Writes that hooks and CLI commands leave spooled are replayed by the
  server at startup and every minute; stale storage transactions are swept
  every 10 minutes instead of only at startup.
- "Sign out everywhere" on `/_account/security` ends every web session of
  your account, the current one included.

### Changed

- The first-run setup wizard lives at `/_setup`; `/` redirects there.
- Pushes, merges and mirror pulls to any branch count as repo activity, not
  only those to the default branch.
- Clones and fetches over HTTP stream instead of being held in memory, and
  a push is written to disk (up to 16 GiB) before the repo is locked, so a
  slow upload no longer holds up everyone else on that repo.
- The server refuses to start when the instance settings in the bucket
  can't be read or decrypted, instead of quietly starting on defaults.
- The federation process logs like the main server (`RUST_LOG`, `info` by
  default).
- Release builds carry no debug info. The Docker build context leaves out
  `target/`, `.git/`, `.env` and `gitquarry.toml`: mount the config at run
  time.

### Fixed

- A per-repo permission granted from the admin user page now shows on the
  repo's permission grid, where it can be revoked. Grants made that way
  before this release are indexed by a migration that runs at startup.
- Merge request comments and reviews no longer show up as a phantom MR #0.
- Release actions validate the tag before using it.
- Logging in from a repo's settings returns you to those settings.
- `/_peers` needs the bootstrap token on an instance with no login method
  yet, like every other admin page.
- A data migration with nothing to do says so.
- The setup wizard loads its stylesheet and scripts, and works when re-run
  against an existing local bucket.
- HTTP clones and fetches from git clients that compress large requests
  (many refs) work.
- Malformed git or LFS requests over HTTP get a 400 or 413 instead of a 500.
- A file whose path ends like a git endpoint (say, one named `info/refs`)
  can be browsed instead of answering 404.
- The first step of an HTTP push no longer counts against the clone rate
  limit or triggers an external-mirror sync.
- Push webhooks and events fire for every push; they were often skipped.
- Pushing a tag first into an empty repo no longer makes it the repo's HEAD.
- Startup no longer wipes the refs of locally cached federation peer repos.
- LFS: two clients locking the same file at once can't both get the lock,
  listing locks by path works for files in subdirectories, and lock storage
  errors are reported instead of ignored. LFS objects uploaded right after a
  push are counted against the quota.
- Deleting or restoring a repo while it's being changed retries instead of
  leaving it half hidden.
- Release assets over 2 MiB upload.
- External mirrors: a branch deleted on both sides and later recreated syncs
  again. Only open merge requests between the repo's own branches are
  exported, and one the forge keeps rejecting is given up after 5 attempts.
  Imports no longer skip merge requests updated at the same instant as the
  last one, and a failed import is retried.
- Merging a merge request with conflicts shows an error instead of a page
  nested in the page.
- Bulk close/reopen of merge requests works.
- The appearance settings and personal access token forms submit again.
- Forking a repo from a federation peer works and checks the name like a
  local fork.
- Label colours show again.
- More of the UI follows the chosen language: the page's language tag, the
  peer fork form, the diff view toggle and several save confirmations.
- Federation: sister instances reach each other through the main server's
  port; your own ban echoed back by a sister stays yours, so unbanning still
  propagates; revoking a token says when it failed; the merge-request API
  reports server failures as such instead of as a bad token or a missing
  repo; the rate limiter no longer grows without bound.

### Security

- The setup wizard no longer prints stored secrets (storage secret key,
  passphrase) back into the page. A blank field keeps the stored value only
  for the same destination.
- Federation: a confirmed sister instance can't have its shared secret
  replaced, and gossip announcements are capped and checked before any
  address they name is contacted.
- The setup wizard at `/_setup` opens only through the link printed in the
  server log, which carries a one-time token that changes on every start.
  Before, whoever reached the port first could configure the instance and
  create the admin.
- Removing an SSH key or a passkey signs that account out of every web
  session.
- With a public URL starting `https://`, cookies always carry `Secure`, even
  when the reverse proxy doesn't send `X-Forwarded-Proto`.
- Admin forms on an instance with no login method yet are CSRF-protected by
  the bootstrap grant; they used to fail.
- With `trust_forwarded_for` on, the anonymous git rate limit keys on the
  last `X-Forwarded-For` entry, the one your proxy added, so a client can't
  pick its own bucket. Your proxy must append (or set) that header.
- Changing the permission of a repo's maintainer or owner needs the owner
  tier; a maintainer could demote an owner.
- Login and preference redirects refuse targets with control characters or
  whitespace, which could send you to another site.
- A crafted file name can't inject markup into the code view.
- A GPG key upload holding more than one key is refused; the extra key's
  signatures would have verified as the first one's.
- Authors of merge requests and comments imported from an external forge are
  shown as `host/login`, so a local user with the same name gets no rights
  over them.
- Federation: a sister can only lift bans it set here, and its ban lists are
  validated; a gossip candidate that changes address starts over; a
  cross-instance merge request needs read access to its source repo;
  proxied federation requests are capped at 2 MiB; the internal token is
  compared in constant time; `git://` peer URLs are refused.
- rustls 0.23.46 (RUSTSEC-2026-0346 to 0349). RUSTSEC-2023-0071 (RSA timing)
  is reviewed and accepted: RSA is only used to verify signatures here.

## [0.5.1] — 2026-09-08

### Added

- Syntax highlighting runs in the browser now, through a vendored
  `highlight.js`, in the blob view, every diff view and markdown fenced code
  blocks. It covers far more languages than the old server-side highlighter,
  and the blob view's `#L42` line permalinks come from the same pass.
- The side-by-side diff matches GitHub's split layout: each side has its own
  line numbers, the columns stay 50/50 whatever the content, and long lines
  wrap instead of forcing a horizontal scrollbar.
- `app.js`, `styler.js` and the highlighter scripts are served under
  content-hashed URLs, so a changed asset gets a new URL instead of a stale
  year-long cache.
- `default_bucket` in `gitquarry.toml` names the bucket the instance serves,
  so extra `[buckets.*]` sections can exist purely as backup destinations or
  mirror targets without every command needing `--bucket`.

### Changed

- Every form target carries the repo in its own URL (`/_merges/merge/{repo}`,
  `/_releases/edit/{repo}`, `/_tags/delete/{repo}`, …), and a stray `GET`
  sends the reader to that repo's page instead of a blank 405.
- A rejected action explains itself on the page it returns you to (the merge
  request, the releases list) instead of a bare error page. A merge request
  refused for a branch that isn't on the server offers the branches that
  are.
- The commit signature badge says only what this instance can stand behind:
  verified by a registered account's key, verified as built by this
  instance, or unverified. The grades taken from git's own check against the
  system keyring ("valid but untrusted", "invalid", "revoked") are gone —
  that keyring is never populated, so they claimed more than was checked.
- A commit whose diff runs past the page's ceiling renders the part that fits
  and links to the raw patch for the rest.

### Removed

- The server-side `syntect` highlighter.

### Fixed

- Creating a repo goes to the new repo's page instead of pasting that page's
  HTML into the form.

## [0.5.0] — 2026-08-23

### Breaking

- Building from source needs Rust 1.98 or newer.

### Added

- Groups have direct members with a role, replacing teams, and can nest as
  subgroups. A permission on a parent group reaches every subgroup and repo
  under it. Each group has its own page listing its repos, subgroups and
  members.
- Repos dashboard split into personal, group and granted-access sections.
- A repo can carry a free-text display name alongside its URL slug, shown
  wherever the repo is listed or titled. Existing repos get one backfilled
  the first time they are listed.
- Backups: replicate the whole bucket to another bucket at the same keys, or
  write a versioned snapshot of the configuration, with retention pruning on
  the destination. Scheduled from the `/_settings` backup section or run once
  with the new `gitquarry backup` / `gitquarry restore` commands.
- Bucket data migrations, applied automatically at startup or by hand with
  `gitquarry migrate`. The first one moves existing groups from teams to
  direct members.

### Changed

- Every error, warning and lifecycle message goes through structured
  logging on stderr, filtered by `RUST_LOG` (default `info`), for the server,
  the pre-receive hook and every maintenance command alike.

### Fixed

- A closed or merged merge request's title, description and labels can no
  longer be edited.
- A merge request form rejects a stale repo reference instead of acting on
  the wrong repo.
- Two repo cards on the same page no longer share the DOM id of their
  star/watch counters.
- Repo path segments reject unescaped URL characters instead of producing a
  broken link.
- The repos dashboard link moved out of the account dropdown to its own spot
  in the navigation.

## [0.4.5] — 2026-08-22

### Added

- Every real git push over ssh or HTTP is recorded in the audit log as
  `repo.push`, not only actions taken from the web UI.
- Repo index: a stats strip, a browse-by-topic sidebar, an ssh clone hint on
  each card, and a card/compact-table density toggle. A new-repo button next
  to the search form, a pickaxe logo and a groups link in the top bar.
- An instance-level source-code link, shown in the footer.
- Settings, account, admin and destructive actions (deletes, merge request
  close/merge) ask for confirmation and submit without a full page reload —
  `/_settings` included, which used to make F5 ask to resubmit.
- The home page's recent-activity rail describes push, release and merge
  request events, and repo cards show the last push time.

### Changed

- Repo settings sections and each `/_settings` section live behind a
  sidebar, every section with its own page.
- All UI text moved into the `gitquarry-i18n` crate.

### Fixed

- A merge landing on the default branch counts as a push for the last-push
  time, like a real push.
- A group's creator becomes its first owner, and a new group shows up
  immediately.
- The add-SSH-key form no longer corrupts the page on its error path.
- Several style fixes on the index page and buttons.
- The startup error for a failed cipher fetch says the fetch was already
  retried.

## [0.4.4] — 2026-08-22

### Added

- Per-account default language in `/_account/appearance`, alongside text
  size, theme and density.
- Repo settings split into one page per section with a sub-navigation,
  instead of 13 forms stacked on one page.

### Fixed

- A failed async form submit rolls back what it had optimistically
  repainted and shows a toast with the server's own error message.
- An error inside the success handler no longer rolls back a change the
  server had already applied.
- Two forms repainting the same part of the page can no longer race each
  other.
- A failed copy to the clipboard shows a toast.
- Error pages render in the visitor's language instead of the default one.

## [0.4.3] — 2026-08-21

### Security

- Responses send `Strict-Transport-Security` and a `Permissions-Policy`
  denying geolocation, camera, microphone, payment and USB.
- Git pack request bodies over HTTP are capped at 16 GiB.

### Performance

- Web responses are compressed (gzip, brotli, zstd).
- Static scripts are served with an `ETag` and a one-year cache.
- Storage requests go through s3db's resilient backend: concurrency caps,
  retry with backoff, a circuit breaker and a raw-bytes cache, tunable under
  `[buckets.<name>.resilience]` in `gitquarry.toml`.
- Fewer git subprocesses per repo summary page and per mirror sync; a mirror
  whose manifest hasn't changed skips the sync entirely.
- Highlighting and markdown rendering run off the async runtime.
- Recently used mirrors are re-synced in the background at startup.
- Visible-repo listing is cached for 10 seconds.
- `TCP_NODELAY` on the ssh listener.

## [0.4.2] — 2026-08-21

### Added

- The create-release form suggests the repo's tags that have no release
  yet.

### Fixed

- A merged merge request whose source branch still existed showed an empty
  diff, stat and commit list.

## [0.4.1] — 2026-08-21

### Changed

- s3db pinned to `v0.4.3`, whose `s3db-inspect` gains `rebind` to re-seal an
  object copied to a new key, and names chunk-sealed objects instead of
  reporting them as decrypt failures.

## [0.4.0] — 2026-08-21

### Breaking

- Pull requests are now merge requests everywhere: routes `/_pulls*` →
  `/_merges*`, audit actions `pull_request.*` → `merge_request.*`, storage
  `repos/<uid>/pulls/` → `repos/<uid>/merges/`.
- The bucket encryption envelope changed. A bucket whose
  `.conf/encryption.json` predates the wrapped data key needs
  `s3db-inspect … migrate-bootstrap --apply` once; old and new binaries cannot
  open each other's buckets.
- The repo name index changed encoding. Run
  `gitquarry backfill-repo-index --bucket <name> --apply` once, or existing
  repos won't resolve by name.

### Added

- `trust_forwarded_for` in `gitquarry.toml` (default off): behind a reverse
  proxy, the anonymous rate limit keys on the forwarded client address.

### Changed

- Repeated wrong passwords slow an account's logins down (doubling up to
  10 s) instead of locking it, so knowing a username isn't enough to lock its
  owner out.
- Enabling or disabling two-factor ends existing sessions, as changing the
  password already did.
- New password hashes use Argon2id at 64 MiB, t=3, p=1.

### Security

- Admin pages and repo settings were open on an instance with no login
  method yet. They now need a token printed at startup, exchanged once at
  `/_bootstrap`, which stops working once a login method exists.
- Release asset uploads check the session before reading the body.
- OIDC login requires `email_verified` and honours two-factor.
- Mirror tokens are no longer visible in `git`'s command line.
- `/_login_totp` requires a CSRF token.
- GPG signature checks reject expired, revoked and bad signatures that
  `gpg`'s exit status reported as fine.
- TOTP codes can't be replayed within their time window.
- HTTP git auth no longer reveals through timing whether a username exists.
- `/_metrics` no longer exposes the build version.
- Forge clients don't follow redirects.
- Download file names are sanitized in `Content-Disposition`.

## [0.3.0] — 2026-08-21

### Added

- External mirrors to GitHub, GitLab or Codeberg — push, pull or both — with
  per-ref status on the repo's settings tab. Only real fast-forwards sync; a
  divergence is reported and never forced, and a branch deleted on one side
  is not re-created from the other.
- What a mirror pulls in passes the same checks as a push: protected
  branches and tags, required signatures, the LFS ceiling and the owner's
  quota.
- Optional two-way merge request sync per mirror — title, description,
  state, merges and comments — without comments echoing between sides.
- An instance-wide host allowlist for mirror targets. The built-in forges
  are always allowed; self-hosted GitLab or Gitea hosts are added in the
  instance settings.
- Mirror configuration and manual syncs are recorded in the audit log.

### Changed

- Forks and template copies no longer inherit merge requests, LFS locks or
  mirrors.
- Configuring a mirror needs the owner tier instead of maintainer.
- Deleting a repo stops its outward mirroring.

### Security

- A fork carried away the original's mirror credential.

## [0.2.0] — 2026-08-20

### Added

- Merge commits built by the server are signed with a key the instance keeps
  for itself, shown as verified in a colour distinct from an account's own
  signature.
- Fast-forward merge, which creates no commit, so the author's own commits
  and signatures land. Greyed out once the branches diverge.
- A merge request that can't merge cleanly lists the conflicting files and
  the commands to resolve them on its own page.
- A footer with copyright, licence and running version, linking to
  `/_changelog` (the release notes for the running version) and `/_license`.
- An account can change its own primary email where OIDC isn't configured.

### Changed

- Account settings split into profile (avatar, emails), security (password,
  two-factor, recovery codes) and keys (SSH, GPG). Keys moved to the new
  `/_account/keys`.
- Account, instance and repo settings pages share one card layout with
  labelled fields.

## [0.1.1] — 2026-08-19

### Fixed

- The container image failed to build: `deploy/Dockerfile` still copied a
  directory that left with the optional CI module.

## [0.1.0] — 2026-08-19

First public release.

### Git hosting

- Git repositories served straight from S3-compatible object storage, with
  no separate git server.
- Embedded SSH server with public-key authentication against named accounts.
- Git smart-HTTP. `ssh://` and smart-HTTP are the only transports.
- Git LFS: batch API plus large-object storage and transfer.

### Web UI

- cgit-inspired browsing: summary, refs, tree, blob, log, commit diff, blame
  and stats, plus a compare view between two refs.
- Commit signature verification badge.
- Create tags from the refs tab.
- Markdown with highlighted fenced code and GitHub-style alert blockquotes
  in READMEs and release notes.
- Light/dark theme with a session toggle and a per-account default.
- GitHub-style sidebar on the repo summary (about, community, releases) and
  a per-language size bar.
- Local identicon avatars, an inline icon set and collapsible mobile
  navigation.
- Full English/Spanish UI.

### Auth & permissions

- OIDC login alongside local admin and user accounts.
- Self-service account page for SSH keys and password.
- Five-tier permission ladder (guest, reporter, developer, maintainer,
  owner) and a per-repo permission grid.
- Two-factor authentication (TOTP) with enrollment, login enforcement and an
  admin recovery path.
- CSRF tokens on authenticated forms, standard security headers and
  per-username login rate limiting.

### Repos & groups

- Create empty repos from the web UI; public, private or limited
  visibility.
- Groups as shared namespaces with inherited permissions, and repo transfer
  between namespaces.
- Default branch selector, protected branches and tags, archive, move and
  delete.
- Stars, watches, forks, topics, an activity feed and public profile pages.
- Merge requests with merge, squash and rebase; a merged one still renders
  after its source branch is deleted.
- Releases with notes and binary assets.
- Instance-wide search across file names and commit messages.

### Storage & operations

- Everything in the bucket is encrypted at rest (Argon2id-derived
  XChaCha20Poly1305) under a mandatory per-bucket passphrase.
- Storage backends: S3, Cloudflare R2, local filesystem, Azure Blob Storage
  and Google Cloud Storage.
- First-run setup wizard.
- Server settings editable from the web UI and stored in the bucket, with
  conflict detection.
- `gitquarry fsck` and `gitquarry repack`.
- Audit log of admin and repo actions.
- Idle- and size-based eviction of local repo mirrors, and graceful
  shutdown.

[0.6.0]: tag-changelog/v0.6.0.md
[0.5.1]: tag-changelog/v0.5.1.md
[0.5.0]: tag-changelog/v0.5.0.md
[0.4.5]: tag-changelog/v0.4.5.md
[0.4.4]: tag-changelog/v0.4.4.md
[0.4.3]: tag-changelog/v0.4.3.md
[0.4.2]: tag-changelog/v0.4.2.md
[0.4.1]: tag-changelog/v0.4.1.md
[0.4.0]: tag-changelog/v0.4.0.md
[0.3.0]: tag-changelog/v0.3.0.md
[0.2.0]: tag-changelog/v0.2.0.md
[0.1.1]: tag-changelog/v0.1.1.md
[0.1.0]: tag-changelog/v0.1.0.md
