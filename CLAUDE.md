# GitQuarry — guidelines

Git remote over S3, in Rust. Public surface: `ssh://` and git-smart-HTTP for push/pull, plus a read-only browsing UI on the same server. No `git://` daemon, no S3 remote helper — both were dropped, not kept opt-in.

One binary, `gitquarry` (`crates/gitquarry-server`), over `gitquarry-core` (ssh, git-smart-HTTP, domain, storage), `gitquarry-webui` (browsing, login/settings/search) and `gitquarry-mirror-external`. `develop`/`master` carry that and nothing else; the optional modules sit on `feature/{cicd,federation,artifacts,tracker}`, each a snapshot of where that module last built, and the commit that removed one is the recipe for putting it back.

## Config

- `gitquarry.toml` lives at the project root, never under `.conf/`.
- `.conf/` is a bucket namespace, not a directory: `global.json`, `encryption.json`, per-repo `config.json`, `users/<name>.json`, `groups/<name>.json`, `index/…`. One object per entity plus small lookup indexes (pubkey/email/namespace), never a whole-bucket blob.
- Resolution order: env var > cwd > git root > user global. No further fallback.

## Encryption

- Everything uploaded is sealed (XChaCha20Poly1305, AAD = the object's S3 key). Mandatory, no opt-out. The salt is stored unsealed; it isn't secret.
- `passphrase` is required in config: Argon2id, random per-bucket salt, fixed params. A wrong one fails once at startup, not scattered later.
- A decrypt failure is a hard error — wrong passphrase, corruption, or an object moved from another key — never a silent fallback to plaintext or to an older format. Changing the envelope is an explicit operator command that proves the old key opens the bucket before rewriting anything.
- Never delete real bucket data without asking. Warn even for test data.

## Storage

- `s3db`/`s3db-crypto`/`s3db-macros` are a separate project (`https://git.yerasuu.com/yerasuu/s3db`), a git dependency pinned by tag, never a branch. Changing the engine means tagging there first, then bumping the tag here. Fetching needs `net.git-fetch-with-cli` (already set) and a reachable instance — which it is not while its own bucket is being migrated, so build before stopping the server.
- New entities use `s3db` (`Collection`/`Entity`/`SecondaryIndex`), not the `Remote` prefix pattern — except LFS objects, release files and git data (packs/manifest), which stay on `Remote`.
- `#[entity(collection = "…")]` needs a literal, so a per-repo collection is built with `Collection::new_owned(prefix)` — see `settings::owner_index`, `access::repo_permission_index`.

## Code

- Comments in English, explaining WHY (a constraint, a bug, an invariant), never WHAT.
- Validate at the edge (`validate.rs` per crate): repo path, rev, sha, tree path. Network input ends up as an FS path, an S3 key and a git argument.
- External arguments to git go after `--end-of-options` (upload-pack/receive-pack excepted — they don't take it).
- `unsafe_code = forbid`, clippy `all = warn`, no bypasses. `cargo fmt` before committing (`max_width = 120`).
- No speculative abstraction: fix what was asked, nothing alongside it. No oversized files — a module outgrowing its siblings becomes a directory module.
- Unit tests keep the module tree but live in a sibling `tests.rs` (`#[cfg(test)] mod tests;`), never an inline block dwarfing the code above it. Every crate but the binary is a library, so `tests/` imports it and exercises the public surface.

## Commits

- One logical change each. Message is one line, imperative, no body. Doesn't fit? Too big — split it.
- Several features in one request: commit each as it lands, never batched at the end.
- Never `Co-Authored-By: Claude` or any AI mention.

## Versioning

SemVer as it reads below 1.0 (§4 plus Cargo's rule: in `0.y.z` the breaking slot is `y`). Derive the number from the commits in the range, never by feel:

| Commits in the range | Bump |
|---|---|
| anything breaking (`feat!:`, `fix!:`, `BREAKING CHANGE:` footer) | `0.(y+1).0` |
| `feat`, `fix`, `perf` | `0.y.(z+1)` |
| only `docs`/`chore`/`style`/`refactor`/`test`/`ci`/`build` | no release |

A breaking change says so in the commit, or the number can't be derived without reading the diff. What breaks differs per repo: `gitquarry` is a server, so its surface is CLI flags, `gitquarry.toml` keys, ssh/HTTP routes and the bucket layout; `s3db` is a library, so it's the exported API and `cargo semver-checks` answers mechanically.

Cutting one: one version for the whole workspace (`[workspace.package]`), never a crate at a time. Write `tag-changelog/vX.Y.Z.md` first — `webui::changelog` pulls it in with `include_str!` on `CARGO_PKG_VERSION`, so a version without notes doesn't compile — then commit the bump separately and tag `vX.Y.Z`, annotated and signed, on `master` after the `develop` -> `master` PR. `CHANGELOG.md` is the other half of the notes and stays grouped by area. An `s3db` repin alone is a `chore` and earns no version.

## Security

- Nothing destructive (`rm`, S3 deletes, `git reset --hard`) without confirming first.
- Changes to `.gitignore`, `.conf/` or the passphrase: warn about the impact, don't assume.
