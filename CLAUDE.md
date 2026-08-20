# GitQuarry — guidelines

Git remote over S3. Rust. Only ssh:// and git-smart-HTTP are publicly supported for push/pull, plus a read-only web browsing UI over the same server. No git:// daemon, no direct S3 remote helper: both were dropped, not kept opt-in.

One binary: `gitquarry` (`crates/gitquarry-server`), built on `gitquarry-core` (ssh/git-smart-HTTP/domain/storage) plus `gitquarry-webui` (browsing UI, login/settings/search) and `gitquarry-mirror-external` (external forge mirroring). `develop`/`master` are the stable core deployment and carry nothing else.

Optional modules live on their own branches, each a snapshot of the last state where that module still built, to be reintegrated one at a time: `feature/cicd` (gitquarry-ci + SDKs), `feature/federation` (peer discovery/gossip, remote browse/fork, PR-proxy, fine-grained tokens), `feature/artifacts` (package registry), `feature/tracker` (issues/board/milestones). The commit that removed each one from `develop` is the exact recipe for putting it back. External GitHub/GitLab/Codeberg mirroring (`gitquarry-mirror-external` + `gitquarry-forge-clients`) is reintegrated and back on `develop`.

## Config

- App config (`gitquarry.toml`): project root. NEVER under `.conf/`.
- `.conf/` = S3 bucket namespace only (global.json, encryption.json, per-repo config.json, users/<name>.json, groups/<name>.json, index/...). Different namespace, not local filesystem. No whole-bucket blob for users/groups — one object per entity, plus small lookup indexes (pubkey/email/namespace).
- Config resolution order: env var > cwd > git root > user global. No extra fallback unless asked.

## Encryption

- Everything uploaded to S3 goes encrypted (XChaCha20Poly1305, AAD = S3 key). Mandatory. No opt-out, no disable flag.
- `passphrase` in config, required, Argon2id + random salt per bucket. Fixed KDF params, not configurable.
- Salt DOES go unencrypted in bucket (not secret). Early verifier: bad passphrase = clear error at startup, not scattered failures.
- No unencrypted-legacy-data self-heal. Prealpha: encryption has always been mandatory, nothing plaintext ever existed. A decrypt failure is a hard error (wrong passphrase / corrupt / moved from another key), never a silent fallback to plaintext parsing.
- Never delete real bucket data without asking. Not even own test data without warning first.

## Storage

- `s3db`/`s3db-crypto`/`s3db-macros` are a separate project now (`ssh://localhost-git:2222/yerasuu/s3db`), consumed as a git dependency pinned by tag — never a floating branch. Changing the engine means committing and tagging there first, then bumping the tag here. Its own design docs live in that repo, not in `docs/`. Fetching it needs `net.git-fetch-with-cli` (already in `.cargo/config.toml`), and the build needs that instance reachable.
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
- Tests follow [the Rust book's organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html): unit tests stay unit tests (same module tree, so private items stay reachable) but live in their own file — `#[cfg(test)] mod tests;` in the module, body in a sibling `tests.rs`. Never an inline `mod tests { ... }` block that dwarfs the code above it. The crate is a binary with no lib target, so `tests/` integration tests can't import it.

## Commits

- Atomic: one logical change per commit, split if concerns mix. Message is one line, imperative, no body — no bullet lists, no multi-paragraph explanation of what changed. If it doesn't fit one line, the commit is probably too big: split it.
- Multi-feature request (e.g. several gaps picked at once): commit each feature right after finishing it, before starting the next. Don't batch everything into one commit at the end.
- Never `Co-Authored-By: Claude` or AI mention in message.

## Security / risky actions

- Never destructive (rm, delete S3, git reset --hard, etc) without confirming first — measure twice.
- Changes to `.gitignore`/`.conf`/passphrase: warn about impact, don't assume.
