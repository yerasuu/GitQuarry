# GitQuarry — guidelines

Git remote over S3. Rust. Only ssh:// and git-smart-HTTP are publicly supported for push/pull, plus a read-only web browsing UI over the same server. No git:// daemon, no direct S3 remote helper: both were dropped, not kept opt-in.

One binary: `gitquarry` (`crates/gitquarry-server`), built on `gitquarry-core` (ssh/git-smart-HTTP/domain/storage) plus `gitquarry-webui` (browsing UI, login/settings/search) and `gitquarry-mirror-external` (external forge mirroring). `develop`/`master` are the stable core deployment and carry nothing else.

Optional modules live on their own branches, each a snapshot of the last state where that module still built, to be reintegrated one at a time: `feature/cicd` (gitquarry-ci + SDKs), `feature/federation` (peer discovery/gossip, remote browse/fork, MR-proxy, fine-grained tokens), `feature/artifacts` (package registry), `feature/tracker` (issues/board/milestones). The commit that removed each one from `develop` is the exact recipe for putting it back.

## Config

- App config (`gitquarry.toml`): project root. NEVER under `.conf/`.
- `.conf/` = S3 bucket namespace only (global.json, encryption.json, per-repo config.json, users/<name>.json, groups/<name>.json, index/...). Different namespace, not local filesystem. No whole-bucket blob for users/groups — one object per entity, plus small lookup indexes (pubkey/email/namespace).
- Config resolution order: env var > cwd > git root > user global. No extra fallback unless asked.

## Encryption

- Everything uploaded to S3 goes encrypted (XChaCha20Poly1305, AAD = S3 key). Mandatory. No opt-out, no disable flag.
- `passphrase` in config, required, Argon2id + random salt per bucket. Fixed KDF params, not configurable.
- Salt DOES go unencrypted in bucket (not secret). Early verifier: bad passphrase = clear error at startup, not scattered failures.
- A decrypt failure is a hard error (wrong passphrase / corrupt / moved from another key), never a silent fallback to plaintext parsing or to an older format. Nothing plaintext was ever written, so there is no legacy to self-heal; a change to the *envelope* (the wrapped-data-key move) is an explicit operator command that proves the old key opens the bucket before it rewrites anything, never something a server does on startup.
- Never delete real bucket data without asking. Not even own test data without warning first.

## Storage

- `s3db`/`s3db-crypto`/`s3db-macros` are a separate project (`https://git.yerasuu.com/yerasuu/s3db`), consumed as a git dependency pinned by tag — never a floating branch. Changing the engine means committing and tagging there first, then bumping the tag here. Fetching it needs `net.git-fetch-with-cli` (already in `.cargo/config.toml`), and the build needs that instance reachable — which it is not while its own bucket is being migrated, so that build has to happen before the server is stopped.
- New entities use `s3db` (`Collection`/`Entity`/`SecondaryIndex`), not the ad-hoc `Remote`-prefix pattern — except LFS objects, release files, and git repo data itself (packs/manifest), which stay on `Remote` (large binary blobs / git-native storage, not JSON entities).
- `#[derive(Entity)]`'s `#[entity(collection = "...")]` needs a compile-time string literal — can't take a per-repo uid at runtime. For a per-repo-scoped collection, build it directly with `Collection::new_owned(prefix: String)` instead (see `settings::owner_index`/`visible_index`, `access::repo_permission_index`).

## Code

- Comments in English. Explain WHY (non-obvious constraint, specific bug, invariant), never WHAT.
- Validate at the edge (`validate.rs` in each crate): repo path, rev, sha, tree-path. Everything from the network ends up as FS path + S3 key + git arg — sanitize before touching any of it.
- External args to git always after `--end-of-options` (except upload-pack/receive-pack, they don't support it).
- `unsafe_code = forbid`, clippy `all = warn`. No bypasses.
- No speculative abstractions/features. Fix scoped to the request, not extras along the way.
- Whole workspace follows `rustfmt` (`rustfmt.toml`: only `max_width = 120`, otherwise defaults). Run `cargo fmt` before committing.
- No oversized files. A module outgrowing its siblings gets split into a directory module (`foo/mod.rs` + submodules), not left to keep growing.
- Tests follow [the Rust book's organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html): unit tests stay unit tests (same module tree, so private items stay reachable) but live in their own file — `#[cfg(test)] mod tests;` in the module, body in a sibling `tests.rs`. Never an inline `mod tests { ... }` block that dwarfs the code above it. Every crate but the binary is a library, so `tests/` integration tests import it directly and exercise only its public surface.

## Commits

- Atomic: one logical change per commit, split if concerns mix. Message is one line, imperative, no body — no bullet lists, no multi-paragraph explanation of what changed. If it doesn't fit one line, the commit is probably too big: split it.
- Multi-feature request (e.g. several gaps picked at once): commit each feature right after finishing it, before starting the next. Don't batch everything into one commit at the end.
- Never `Co-Authored-By: Claude` or AI mention in message.

## Versioning

SemVer, read the way it applies below 1.0 (SemVer §4 + Cargo's rule: in
`0.y.z` the breaking slot is `y`). Derive the number from the commits in the
range, never by feel:

| Commits in the range | Bump |
|---|---|
| anything breaking (`feat!:`, `fix!:`, or a `BREAKING CHANGE:` footer) | `0.(y+1).0` |
| `feat`, `fix`, `perf` | `0.y.(z+1)` |
| only `docs`/`chore`/`style`/`refactor`/`test`/`ci`/`build` | no release at all |

A breaking change has to say so in the commit itself — the `!` or the
footer — or the number can't be derived without reading the diff.

**What "breaking" means differs per repo**, because their public surface
does. `gitquarry` is a server: its surface is CLI flags, `gitquarry.toml`
keys, ssh/HTTP routes, and the bucket layout — the merge-request rename
moved routes *and* an S3 prefix, which is why it was `0.4.0`. `s3db` is a
library: its surface is the exported Rust API, and `cargo semver-checks`
answers the question mechanically.

Cutting one:

- One version for the whole workspace (`[workspace.package]`), never a crate
  at a time — both repos.
- `tag-changelog/vX.Y.Z.md` before the bump: `webui::changelog` pulls it in
  with `include_str!` keyed on `CARGO_PKG_VERSION`, so a version without its
  notes doesn't compile. `CHANGELOG.md` is the other half and stays grouped
  by area.
- Commit the notes and the bump separately, then tag `vX.Y.Z` — annotated
  and signed, on `master`, after the `develop` -> `master` PR.
- An `s3db` release is tagged there first, then its tag bumped in
  `gitquarry`'s four manifests. That repin alone is a `chore` and does not
  earn a version of its own.

## Security / risky actions

- Never destructive (rm, delete S3, git reset --hard, etc) without confirming first — measure twice.
- Changes to `.gitignore`/`.conf`/passphrase: warn about impact, don't assume.
