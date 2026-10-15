# GitQuarry — guidelines

Git remote over S3, in Rust. Public surface: `ssh://` and git-smart-HTTP for push/pull, plus a read-only browsing UI on the same server. No `git://` daemon, no S3 remote helper — both were dropped, not kept opt-in.

One binary, `gitquarry` (`crates/gitquarry-server`), over `gitquarry-core` (ssh, git-smart-HTTP, domain, storage), `gitquarry-webui` (browsing, login/settings/search), `gitquarry-mirror-external` and the optional `gitquarry-federation` (its own binary, enabled by `[federation]`). `develop`/`master` carry that and nothing else; the other optional modules sit on `feature/{cicd,artifacts,tracker}`, each a snapshot of where that module last built, and the commit that removed one is the recipe for putting it back.

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

- Comments are for a technical reader who can already read the code.
  Write one only when it tells them something the code can't: why
  this way and not the obvious one, a constraint from outside (git,
  S3, a protocol, a browser), an invariant other code relies on, a
  bug or race this avoids, what breaks if it changes, or where the
  rest of the story lives (`see #42`, a module, an RFC). Name the
  reason itself; no fixed wording or keyword is required, and none
  makes a comment useful by itself.
  - Don't narrate the code (`// loop over the refs`, `// return the
    result`), restate a name or type, mark sections, log history
    ("changed from X", "used to"; that is `git log`), or address the
    reader of the diff instead of the reader of the file.
  - Test: delete it. If a competent reader of this file loses nothing
    they couldn't get from the code in front of them, it goes.
  - `TODO`/`FIXME` say what is missing and what unblocks it;
    `SAFETY:` states the invariant that makes the code sound.
  - English, full sentences, as long as the reason needs and no
    longer. Fix or delete a comment that the code no longer matches.
  Caught by: `comment-restates-code`.
- Doc comments (`///`, `//!`) are the contract for callers: why the
  item exists, what the caller must uphold or can rely on (ownership,
  validity, ordering, idempotence, what it never does), and its
  `# Errors`/`# Panics`/`# Safety`/`# Examples`. Leave out what the
  signature already says: an `# Arguments`/`# Returns` section that
  restates the types is noise, one that states a precondition stays.
  Caught by: `doc-restates-signature`.
- Validate at the edge (`validate.rs` per crate): repo path, rev,
  sha, tree path. Network input ends up as an FS path, an S3 key
  and a git argument.
- External arguments to git go after `--end-of-options`
  (upload-pack/receive-pack excepted — they don't take it).
- `unsafe_code = forbid`, clippy `all = warn`, no bypasses.
  `cargo fmt` before committing (`max_width = 120`).
- No speculative abstraction: fix what was asked, nothing alongside
  it. No oversized files — a module outgrowing its siblings becomes
  a directory module. Custom macros only after a pattern repeats
  3+ times in this codebase, with a project-prefixed name.
- Type inference: don't annotate types or lifetimes the compiler
  can infer. Explicit only on public APIs or genuine ambiguity.
  Caught by: `trivial-type-annotation`, `explicit-lifetime-elision`.
- Reference > move > clone. Before `.clone()`, ask: can I borrow?
  Can I move? Clone only when ownership demands it.
  Caught by: `possible-borrow-instead-of-clone`, `redundant_clone`,
  `clone_on_ref_ptr`.
- Minimal trait bounds: if `Hash` isn't used in the body, it
  doesn't belong in the `where` clause. Remove what the compiler
  doesn't ask for. Caught by: `extra_unused_type_parameters`,
  `trait_duplication_in_bounds`.
- Selective derives: `#[derive(...)]` only what you need. Think
  before stacking. Manual `PartialEq` if the comparison should
  ignore fields. More than 4 derives triggers a review.
  Caught by: `excessive-derives`, `derivable_impls`.
- Errors with semantics: don't propagate `?` blindly. Map errors
  by business context. `NotFound` is not `PermissionDenied`.
  No `Err(_)` catch-all on closed enums without justification.
  Caught by: `wildcard-error-match`, `match_wild_err_arm`.
- Concurrency by case: `AtomicU32` for counters, `RwLock` if
  reads >> writes, channels for producer-consumer, `Weak` for
  parent links in `Arc` graphs. `Mutex` is the last resort.
  Caught by: `mutex_atomic`, `mutex_integer`, `mutex-on-primitive`.
- No intermediate `collect()`: chain iterators to the destination.
  If you need a `Vec`, pre-allocate with `with_capacity`.
  Caught by: `needless_collect`, `unnecessary_collect`.
- Async without blockers: no `block_on()` inside async contexts.
  Within the async runtime, everything is async. Block synchronously
  outside. Caught by: `block-on-in-async`.
- Structured logging with `tracing`: named fields, target per
  module. Never `println!` or `log::info!("Processing started")`
  in production code. Caught by: `println-in-production`.

## Tests

- Every test lives in the crate's `tests/` and exercises only the
  public API — no inline `mod tests {}`, no sibling `tests.rs`. Never
  widen visibility for a test (`#[cfg(any(test, feature = "test-util"))]
  pub`, `test-util` features): security first, the tested surface is
  the shipped one. A behaviour only reachable through a private item is
  tested through the public path that reaches it (a route, a command),
  or not at all.
- Pragmatic tests: happy path, real cases that break in production,
  `#[should_panic]`, `#[ignore]` for integration. No theoretical
  tests covering variants nobody constructs.
- No theoretically perfect mocks. Use fixtures, temp files, real
  data when feasible.
  
## Commits

- One logical change each. Message is one line, imperative, no body.
  Doesn't fit? Too big — split it.
- Several features in one request: commit each as it lands, never
  batched at the end.
- Never `Co-Authored-By: Claude` or any AI mention.
  Caught by: `ai-attribution` (scans commit messages).

## Versioning

SemVer as it reads below 1.0 (§4 plus Cargo's rule: in `0.y.z`
the breaking slot is `y`). Derive the number from the commits in
the range, never by feel:

| Commits in the range | Bump |
|---|---|
| anything breaking (`feat!:`, `fix!:`, `BREAKING CHANGE:` footer) | `0.(y+1).0` |
| `feat`, `fix`, `perf` | `0.y.(z+1)` |
| only `docs`/`chore`/`style`/`refactor`/`test`/`ci`/`build` | no release |

- A breaking change says so in the commit, or the number can't be
  derived without reading the diff.
- What breaks differs per repo: `gitquarry` is a server (CLI flags,
  `gitquarry.toml` keys, ssh/HTTP routes, bucket layout); `s3db`
  is a library (exported API — `cargo semver-checks` answers it).
- Tag notes are a technical record: terse bullets anchored in code
  (module, function, route, storage prefix, constant and value),
  facts read from the tree *at that tag*, no narrative.
- One version per workspace (`[workspace.package]`), never a crate
  at a time.
- Write `tag-changelog/vX.Y.Z.md` first — `webui::changelog`
  `include_str!`s it by `CARGO_PKG_VERSION`, so a version without
  notes doesn't compile — then the bump as its own commit, then
  tag `vX.Y.Z` annotated and signed on `master`, after the
  `develop` -> `master` PR.
- `CHANGELOG.md` is the other half of the notes: one section per release, newest first, Keep a Changelog headings (`Breaking`/`Added`/`Changed`/`Removed`/`Fixed`/`Security`), worded for operators and users, each version linking its `tag-changelog/vX.Y.Z.md`.
  An `s3db` repin alone is a `chore` and earns no version.

## Security

- Nothing destructive (`rm`, S3 deletes, `git reset --hard`) without
  confirming first.
- Changes to `.gitignore`, `.conf/` or the passphrase: warn about
  the impact, don't assume.

## Board tags

Every `kb` entry carries tags from this fixed set; add a new one only when
no existing tag fits, and record it here.

- Area, one or two per entry: `storage`, `crypto`, `ssh`, `http`, `webui`,
  `auth`, `federation`, `backup`, `mirror`, `security`, `release`, `ops`,
  `modules` (the parked `feature/*` modules), `brain` (the board itself).
- `s3db`: work on the storage engine. It has no board of its own; its
  tasks live here, are fixed there under a new tag, then repinned here.
- Security entries add standard tags (kb decision #72):
  - `cwe-N`: the MITRE CWE id of the weakness, one or more.
  - `owasp-aNN`: the OWASP Top 10:2025 category, only where it clearly fits.
  - `sev-high` / `sev-medium` / `sev-low`: qualitative (CVSS-scale)
    severity, on findings only, never on design decisions.
  - Process notes (audit scope, test policy) take no CWE.
- Board text is in Spain Spanish; tags, identifiers and code stay verbatim.

## Automation reference

`scripts/check-practices.sh` checks the practices above that clippy and
rustfmt don't: what is staged by default, `--all` for the whole tree,
the PR diff in CI. `PRACTICE` findings fail the run; `REVIEW` findings
are heuristics too weak to block and only ask a human to look.

| Practice | Enforcement |
|---|---|
| Comments that restate the code | `check-practices.sh` → `comment-restates-code` |
| Doc restates signature | `check-practices.sh` → `doc-restates-signature` |
| Trivial type annotations | `check-practices.sh` → `trivial-type-annotation` |
| Explicit elidable lifetimes | `check-practices.sh` → `explicit-lifetime-elision` |
| Clone where borrow suffices | Clippy `redundant_clone` + review `possible-borrow-instead-of-clone` |
| Unused trait bounds | Clippy → `extra_unused_type_parameters` |
| More than 4 behavioural derives | review `excessive-derives` (Debug, serde, `Entity` don't count) |
| Wildcard error match without a reason | `check-practices.sh` → `wildcard-error-match` |
| Mutex on primitives | Clippy + `check-practices.sh` → `mutex-on-primitive` |
| Intermediate `collect()` | Clippy → `needless_collect` |
| `block_on()` in async | `check-practices.sh` → `block-on-in-async` |
| `println!` outside CLI output | `check-practices.sh` → `println-in-production` |
| AI attribution in commits | `check-practices.sh` → `ai-attribution` |
| Format violations | `cargo fmt --check` |
| Semver breaks (libraries) | `cargo semver-checks` |

<!-- kanbrain:start -->
## Project memory: kanbrain

This project's tasks, to-dos, decisions and notes live in `.brain/`, managed only with `kb`, kanbrain's CLI.
The user follows them live on a kanban board: keep it showing what you're doing as you go, not at the end.

- Start of session: `kb context`, unless its output is already above (a session hook adds it).
- Work that changes the repository, or that the user should see, gets a task; questions and lookups don't. Find it
  (`search WORDS`) or create it (`add TITLE`), then `start ID` (`add --start TITLE` does both). "Todos", a plan or
  what's pending, in any language, are tasks here, one `add` each; your built-in to-do list isn't on the board, so
  keep it for steps within the task you've started. A task with real work gets a body, `add --body - TITLE`:
  **Goal** (what and why, a line or two) and **Done when** (what the user will check).
- While working: `comment ID TEXT` when a step is done, when you find a cause, and before you stop (one or two
  lines: what changed, what's next); `done ID` as soon as one is finished. Stopping before it is: `comment`, then
  `park ID`. Doing isn't limited for you: several of your tasks can run at once. Never park, finish or move a task
  you didn't start (the user's, or another session's).
- Splitting work between subagents: give each part that changes the repository its own task, and run the board
  yourself: `add --start` it before launching the subagent, then `comment` its result and `done` it. Subagents
  only do the work; lookups and research get no task. `kb` commands can run in parallel.
- Blocked on the user (a requirement, access, a choice that's theirs): `ask ID TEXT` parks the task with the
  question; don't guess. A choice you shouldn't make alone and weren't asked to (architecture, a dependency,
  storage, a public interface): `propose ID --body - TITLE`, Context / Options / Recommendation on stdin; it parks
  the task until the user accepts or rejects it. Then, unless the user said they're away, if your tool can run a
  command in the background and wake you when it exits, run `kb wait ID` (the task) that way and end your turn: it
  prints the answer, or that there's none yet (then stop, don't wait again). Otherwise go on with other work or
  stop; answers come first in `context`, under Answered: `show ID`, then pick the task up. If the user answers in
  the chat instead, `start` the task (that ends the wait). `accept`/`reject` a decision only when the user tells
  you to.
- Choosing between approaches within your task: `search` for earlier decisions (only accepted ones are in force),
  then `add --decision --link ID --body - TITLE` with Context / Decision / Consequences on stdin. Non-obvious
  findings: `add --note --link ID --body - TITLE`; a problem someone has to fix is a task instead, in the
  repository that has to change.
- `## Project` in `context` is the application's shared context. If it isn't described, or you're asked to set up
  or load the brain, describe it first, from the README and manifests: `project describe -` (what the app is, its
  services and how they connect, conventions they share: the big picture, in at most five lines, without tables,
  columns, function signatures, files or dependencies, which the code already says). Rewrite it when the big
  picture changes (a service added, another storage), not after every change. With several repositories, this
  one's role too: `project notes -`. How to build, test and run this repository, and its code style, go in this
  file, outside this section: add them as you learn them.
- Also: `show ID` (one entry in full), `describe ID TEXT`, `rename ID TITLE`, `tag ID +TAG -TAG` (add or remove
  tags), `link ID OTHER…` (an id here, or `REPO#ID` in another repository of the project); `-` reads text from
  stdin. Never edit the files in `.brain/` by hand: `kb` keeps them consistent.
<!-- kanbrain:end -->
