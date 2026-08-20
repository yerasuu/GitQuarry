# `mirror::external` extraction

## Context

Builds on `docs/external-mirror-concept.md` (Track A ref-mirror + Track B PR-sync,
both implemented). This doc adds exactly one thing: pulling `mirror::external`
(everything under `crates/gitquarry-core/src/mirror/external/`) out of
`gitquarry-core` into its own crate — `linear-humming-locket.md`'s own audit
(`docs/tracker-binary-split-concept.md`'s sibling plan) had explicitly deferred
this ("fuera de alcance... necesitaría su propio doc de coupling-inventory").
This is that doc.

**Not the tracker/ci/federation/artifacts shape.** Those are all
compile-time-optional (`feature = "..."`), config-toggled (`[section]` in
`gitquarry.toml` → `AppContext.<x>: Option<Link>`), always-on network services
reached over loopback HTTP. `mirror::external` is none of these things today:
there's no `[mirror]` config section, no `Option<Link>` gate — a mirror is
per-repo state an owner adds via the UI, and it syncs with zero extra deploy
step. The actual coupling that made tracker's shape work (pure data CRUD, zero
filesystem/subprocess dependency) doesn't hold here: `sync_mirror_once` shells
out to real `git push`/`fetch`/`ls-remote` against the *same bare-repo
directory* (`ctx.open_mirror`'s `mirrors_root`) the kernel's own push path
locks and mutates. A persistent separate process reached only over a network
API would mean re-inventing every git plumbing op as an RPC, and solving
cross-process locking on a directory core already owns the locking for.
Instead this follows the shape the codebase *already chose* for exactly this
problem: `mirror::hook::spawn_mirror_sync` already runs sync as a **detached
subprocess** (`gitquarry sync-mirrors`, `sync_cli.rs`) sharing the same local
filesystem and S3 bucket, best-effort, never blocking the push it followed.
This doc formalizes that into a real separate crate/binary instead of a hidden
subcommand of the core binary reusing a placeholder-stuffed `AppContext`.

**Core must keep working with zero regard for whether this binary exists,
is running, or is even installed** — same "basic git functions never block on
an optional subsystem" invariant every other split already honors, stated
explicitly here because refactoring `mirror::hook`/`mirror::mod.rs` itself is
in scope if that's what a clean cut needs (not just mechanical file moves).
`spawn_mirror_sync` already degrades gracefully (`Command::spawn()` failure →
log and continue, push still succeeds) — that property must survive unchanged.

Four things settled by inventorying actual coupling before drafting (not
assumed — verified with grep against real call sites):

1. `mirror/external/mod.rs`, `access/mod.rs`, `domain.rs` lean on core for:
   `AppContext`/`ResolvedRepo`, `storage::{Backend,Cas,Uid,REPOS_PREFIX,
   INDEX_PREFIX}`, `crypto::Cipher`, `ctx.open_mirror(resolved)`, and a
   *reverse* edge — `mirror::hook.rs:517` (kernel, pre-receive path) calls
   `mirror::external::access::list_external_mirrors` to decide whether to
   spawn a sync. That reverse edge is the one real blocker to moving `access`
   above core in the dependency graph (see the index-relocation decision
   below).
2. `pr_sync/mod.rs` is the one file with real `domain::pull_request` coupling
   (`create`/`close`/`mark_merged_externally`, plus direct
   `storage::pull_requests::*` reads/writes bypassing the domain layer) — zero
   coupling to `domain::remote_fork` despite the original plan's blanket claim
   (grep-verified: no matches). It already depends on the leaf crates
   `gitquarry-forge-clients` and `gitquarry-i18n` from Fase 0 — no change
   needed there.
3. Git plumbing splits cleanly in two: `push_to_remote_url`, `ls_remote_refs`,
   `list_refs_with_sha`, `fetch_ref_from_remote_url`, `redact_url` are called
   **exclusively** from `mirror::external` (grep-verified, zero other
   callers) — these move out of `gitquarry-core::git` entirely. `is_ancestor`,
   `scratch_dir`, `pack_objects_for_push`, `update_ref` are shared kernel
   primitives (`mirror::hook`, `domain::pull_request`, `domain::remote_fork`,
   `mirror::mod.rs` all call them too) — these stay in core; the new crate
   depends on core for them, same as `gitquarry-domain` already does for
   `AppContext`/`storage`.
4. The `.conf/index/mirrors-enabled/<uid>.json` hint index
   (`access::set_mirrors_enabled_index`/`list_repos_with_enabled_mirrors`,
   currently inside `access/mod.rs`) is *already* the cheap
   "does this repo have any enabled mirror" primitive the periodic tick uses
   to know which repos to visit — `mirror::hook.rs`'s own need (decide
   whether to spawn a sync) is the exact same question, just currently
   answered the expensive way (`list_external_mirrors` + `.iter().any(...)`,
   a full decrypt-and-list). Relocating this one boolean index down into
   core's own storage layer resolves the reverse-dependency blocker *and*
   fixes that inefficiency in the same move.

## Design decisions (settled, not open)

| Question | Decision | Why |
|---|---|---|
| Shape | New crate `gitquarry-mirror-external`, **intermediate crate** (depends on `gitquarry-core`, same direction as `gitquarry-domain` — never the reverse), not a `tracker`/`ci`-style `Option<Link>` service. Compiles to its own binary reusing today's exact invocation shape: spawned as a subprocess with `GITQUARRY_BUCKET`/`GITQUARRY_REPO_UID`/`GITQUARRY_CONFIG`/`GIT_DIR` env vars. | Matches the coupling reality (bare-repo filesystem + subprocess git plumbing, not pure data CRUD) instead of forcing the tracker shape onto a case that doesn't fit it. |
| What moves | `mirror::external::{mod,access,domain,pr_sync}` in full, plus the 5 exclusive git-plumbing fns out of `gitquarry-core::git`, plus `sync_cli.rs`'s role (rewritten, see below). | Everything with zero reverse callers except the one index read (next row). |
| The `access::list_external_mirrors` reverse edge | `set_mirrors_enabled_index`/`has_enabled_mirror(uid) -> bool`/`list_repos_with_enabled_mirrors` move **into `gitquarry-core::storage`** (new small module, sibling to `storage::settings::visible_index` — same "hint, not fact" shape). `mirror::hook.rs` calls this core-owned primitive directly (zero new dependency). The new crate's `domain.rs` calls `gitquarry_core::storage::mirrors_index::set(...)` after every create/update/delete — a normal downstream call, no cycle. | The index's whole purpose was already "answer this exact question cheaply" — moving *only* the index (not the full CRUD/validation/secrets surface) to core is the narrow, non-speculative cut, same reasoning `tracker-binary-split-concept.md` used to keep `closing_issue_numbers` a tiny duplicated primitive rather than pulling in a whole crate for one function. |
| Git plumbing split | 5 fns (`push_to_remote_url`, `ls_remote_refs`, `list_refs_with_sha`, `fetch_ref_from_remote_url`, `redact_url`) move to a new `gitquarry-mirror-external::git_remote` module. 4 shared fns (`is_ancestor`, `scratch_dir`, `pack_objects_for_push`, `update_ref`) stay in `gitquarry-core::git`, called via `gitquarry_core::git::*` from the new crate. | Grep-verified exclusivity in both directions — no speculative "shared git crate" invented for functions only one side ever calls. |
| Own runtime state | **Deviation, found during implementation**: NOT a standalone `MirrorSyncState` as originally planned here — `AppContext::open_mirror` (locking, `SyncStrategy` dispatch, GC touch) is real synchronization logic with exactly one implementation that must stay that way; reimplementing it against a narrower type would risk two independent lock/sync paths touching the same bare repo drifting apart. Instead, core gains a small `AppContext::for_mirror_sync(...)` constructor that fills every field this process never touches (`ssh_bind`, `oidc`, `federation`/`ci`/`artifacts`/`tracker`, ...) with a harmless placeholder — same placeholder values `sync_cli.rs` used inline, just centralized in one core-owned function instead of duplicated in this crate's `main.rs`. | Placeholder-field construction is safe to factor out; `open_mirror`'s own locking/sync behavior is not safe to duplicate. Confirmed by reading `open_mirror`/`open_mirror_common` before committing to the narrower-type design this row originally proposed. |
| Per-push trigger | Unchanged behavior: `mirror::hook::spawn_mirror_sync` still fires a detached, best-effort subprocess right after a push, still never blocks the push, still degrades to "sync happens on the next sweep instead" on any failure (missing binary, spawn error) — same log-and-continue this already does. Only change: the spawned exe is no longer `current_exe()` re-invoked with a hidden `sync-mirrors` subcommand, but the new crate's own binary, resolved via a new `mirror_sync_exe_path` field (config-driven, defaulting to a sibling path next to the core binary — same directory convention `deploy/` already installs multi-binary setups into). | Zero behavior change to the push path itself; only the target executable changes. Keeps the "basic git functions never block on this" invariant explicit and unchanged, per direct instruction. |
| Periodic tick | Moves out of `gitquarry-server/src/serve.rs`'s in-process `tokio::spawn(external::run_periodic_sync(...))` into the new binary's own `daemon` subcommand (own `tokio::time::interval` loop, reads `list_repos_with_enabled_mirrors` from core's relocated index, calls the moved `sync_repo_mirrors` per repo) — **not** N subprocess spawns per tick, one long-lived process. `gitquarry-server` still spawns this daemon itself at startup (`std::process::Command`, best-effort, logged-not-fatal on failure) so a default deploy keeps today's zero-extra-step behavior; an operator who wants it as a genuinely separate lifecycle (own restart/scaling) can instead run `gitquarry-mirror-external daemon` themselves and tell core not to auto-spawn it via a config flag. | Preserves today's "add a mirror in the UI, it just works" UX by default (no doc/tracker-style "configure `[section]` first or the feature is off") while still making the process boundary real for anyone who wants to manage it independently — resolves the tension between "this is deploy-optional plumbing" and "there's no existing on/off toggle for it, so introducing one would be a behavior change" flagged in the audit. |
| Rendering | Unchanged: `webui::repo_settings::mirrors` keeps talking to `AppContext`-level `domain::{list,get,set,delete}`-shaped calls — those become thin wrappers in `gitquarry-webui`/`gitquarry-server` over the new crate's own functions (same in-process Rust call as today, just crossing one more crate boundary, no IPC). Mirrors are managed synchronously from the web request (`save_mirror`'s existing "sync now" button already accepts blocking inline), so there's no reason to route *configuration* through the subprocess/daemon at all — only the actual git-plumbing sync work moves. | The settings-tab code path never needed process isolation; only the git-plumbing sync engine did. Splitting config-CRUD into the new crate is still correct (dependency-direction cleanup), but it stays an in-process library call from `webui`/`server`, exactly like `gitquarry-domain::release` today. |
| Wire types | None needed. This never crosses a network/serialization boundary — the new crate is a plain Rust dependency of `gitquarry-webui`/`gitquarry-server` (config CRUD) and a subprocess with env-var config (sync engine), not an HTTP peer. | No IPC exists here to need `gitquarry-wire` DTOs; keeping it a library avoids inventing a boundary that isn't real. |

## Structure once split

```
crates/gitquarry-mirror-external/
├── Cargo.toml              (deps: gitquarry-core, gitquarry-forge-clients, gitquarry-i18n, s3db, s3db-crypto, tokio, anyhow, serde)
├── src/
│   ├── main.rs             (subcommands: `sync --repo <uid>` == today's sync_cli.rs role; `daemon` == today's run_periodic_sync)
│   ├── git_remote.rs        (moved: push_to_remote_url, ls_remote_refs, list_refs_with_sha, fetch_ref_from_remote_url, redact_url)
│   ├── access/mod.rs        (moved as-is minus the mirrors-enabled index, which relocates to core)
│   ├── domain.rs            (moved; refresh_mirrors_enabled_index now calls gitquarry_core::storage::mirrors_index::set)
│   ├── pr_sync/mod.rs        (moved as-is)
│   └── engine.rs            (moved: plan_ref_sync/sync_mirror_once/sync_repo_mirrors/run_periodic_sync, was mirror/external/mod.rs)

crates/gitquarry-core/src/
├── git/mod.rs               (loses the 5 exclusive fns; keeps is_ancestor/scratch_dir/pack_objects_for_push/update_ref)
├── storage/mirrors_index.rs (new: relocated mirrors-enabled hint index — has_enabled_mirror/set/list_repos_with_enabled_mirrors)
├── mirror/hook.rs            (has_enabled_mirror check now reads storage::mirrors_index directly, no more access:: call;
│                              spawn_mirror_sync targets the new binary's resolved path instead of current_exe())
└── config/mod.rs             (new: mirror_sync_exe_path, auto_spawn_mirror_daemon: bool default true)

crates/gitquarry-webui/src/webui/repo_settings/mirrors.rs
└── (touched: gitquarry_core::mirror::external::* calls become gitquarry_mirror_external::* calls)

crates/gitquarry-server/src/serve.rs
└── (touched: tokio::spawn(external::run_periodic_sync(...)) replaced by a best-effort
    std::process::Command spawn of `gitquarry-mirror-external daemon`, gated by auto_spawn_mirror_daemon)
```

## Implementation

### Phase 1 — relocate the mirrors-enabled index into core (unblocks everything else)

- New `crates/gitquarry-core/src/storage/mirrors_index.rs`: `set(backend, cipher, uid, enabled)`, `has_enabled_mirror(backend, cipher, uid) -> bool`, `list_repos_with_enabled_mirrors(backend) -> Result<Vec<Uid>>` — moved byte-for-byte from `access/mod.rs`'s `mirrors_enabled_index`/`set_mirrors_enabled_index`/`list_repos_with_enabled_mirrors`.
- `mirror::hook.rs:517-523` rewritten to call `storage::mirrors_index::has_enabled_mirror(...)` directly — drops the full `list_external_mirrors` + `.iter().any(...)` scan entirely (a real efficiency fix, not just a refactor for its own sake).
- `mirror::external::domain::refresh_mirrors_enabled_index` (still in core at this point, moves in Phase 3) updated to call the new core-owned function instead of `access::set_mirrors_enabled_index`.
- Commit: `refactor(mirror): relocate the mirrors-enabled index to storage`.

### Phase 2 — split git plumbing

- Nothing moves yet (the new crate doesn't exist). Just confirm via `cargo build` that `push_to_remote_url`/`ls_remote_refs`/`list_refs_with_sha`/`fetch_ref_from_remote_url`/`redact_url` truly have zero callers outside `mirror::external` (this doc's own audit, re-verified at execution time — the same discipline Fase 1's domain-scope correction used).
- Commit: none yet, folded into Phase 3.

### Phases 3–5 — crate move, binary wiring, webui dependency swap

**Deviation, found during implementation**: these three landed as one commit,
not three. `gitquarry-webui`'s `repo_settings/mirrors.rs`/`repo/mod.rs` and
`gitquarry-server/src/serve.rs` all call `gitquarry_core::mirror::external::*`
directly — the moment the old module is deleted from core, both of those
crates stop compiling until they're updated in the same commit. There's no
compilable intermediate state to split at, unlike Phase 1 (a pure addition)
or Phase 2 (no code change at all). Splitting further would mean shipping a
commit that doesn't build, which is worse than one larger atomic commit.

- New crate `gitquarry-mirror-external`: `access.rs`/`domain.rs`/`pr_sync/mod.rs`/
  `engine.rs` (renamed from `mod.rs`) moved with import substitutions
  (`crate::` → `gitquarry_core::`), the 5 exclusive git fns moved into the
  new crate's own `git_remote.rs`, 3 exclusive `validate_mirror_*` fns moved
  into its own `validate.rs`.
- `main.rs`: `sync --repo <uid>` subcommand replaces `sync_cli::run()`'s
  role, built via `gitquarry_core::app::AppContext::for_mirror_sync(...)`
  (see the deviation noted above) instead of a hand-built struct literal.
  `daemon` subcommand wraps the moved `run_periodic_sync` loop, resolving
  `mirrors_root` from `GITQUARRY_MIRRORS_ROOT` (passed by `gitquarry`'s own
  server process) rather than re-deriving `--mirrors-dir`/default-path logic
  a second time.
- `mirror::hook::spawn_mirror_sync` retargeted to the new binary's path
  (`config::mirror_sync_exe_path`, defaulting next to the core binary), now
  passing the repo uid as a `--repo` CLI arg instead of a `GITQUARRY_REPO_UID`
  env var (matches the new binary's own arg-based subcommand shape).
- `gitquarry-server/src/serve.rs`: drop `tokio::spawn(external::run_periodic_sync(...))`;
  add `spawn_mirror_sync_daemon`, a best-effort `Command::new(mirror_sync_exe_path).arg("daemon")`
  spawn, logged-not-fatal on failure, gated by `config::auto_spawn_mirror_daemon`
  (default `true`).
- `gitquarry-core/src/mirror/external/` (old module) and `sync_cli.rs`
  deleted.
- `gitquarry-webui/src/webui/repo_settings/mirrors.rs`, `repo/mod.rs`,
  `repo_settings/mod.rs`: `gitquarry_core::mirror::external::*` call sites
  become `gitquarry_mirror_external::*` (in-process library calls, both
  `gitquarry-webui`/`gitquarry-server` add the new dependency).
- Commit: `feat(mirror): split mirror::external into its own crate and binary`.

## Explicitly out of scope

- `docs/external-mirror-concept.md`'s own feature content (Track A/B behavior, conflict policy, anti-loop bookkeeping) — unaffected, this doc only moves *where the code lives and runs*, not what it does.
- Any change to how `gitquarry-ci`/`gitquarry-federation`/`gitquarry-artifacts`/`gitquarry-tracker`'s `Option<Link>` shape works — this is a deliberately different shape (subprocess + intermediate crate, not a loopback service), not a fifth instance of that pattern.
- `mirror::hook`/`mirror::gc`/`mirror::mod.rs`/`mirror::sync_cli`'s own kernel logic (bucket-backed mirror lifecycle, pre-receive hook body, GC) — stays in core, confirmed kernel/non-abstractable by the original audit; only the one reverse-dependency read (Phase 1) and the spawn target (Phase 4) touch this code.

## Critical files (new/touched)

- `crates/gitquarry-mirror-external/{Cargo.toml,src/{main,git_remote,access,domain,engine,validate}.rs,src/pr_sync/mod.rs}` (new crate)
- `crates/gitquarry-core/src/storage/mirrors_index.rs` (new, relocated index)
- `crates/gitquarry-core/src/mirror/hook.rs` (touched: index read, spawn target)
- `crates/gitquarry-core/src/git/mod.rs` (touched: 5 fns removed)
- `crates/gitquarry-core/src/config/mod.rs` (new: `mirror_sync_exe_path`, `auto_spawn_mirror_daemon`)
- `crates/gitquarry-server/src/serve.rs` (touched: daemon spawn replaces in-process tokio::spawn)
- `crates/gitquarry-webui/src/webui/repo_settings/mirrors.rs` (touched: crate import swap)

## Phase 6 — sync before serving (pull and push)

**Added after Phase 2 shipped, on explicit request**: Track A's sync was
push-triggered (async, *after* a push already landed) plus a 15-minute
daemon tick — nothing ran a fresh sync *before* a client actually reads or
writes the repo. `git::ssh`/`git::http` both funnel every `upload-pack`
(clone/fetch) and `receive-pack` (push) through one shared call site right
before `AppContext::open_mirror` — `mirror::sync_external_before_serving`
hooks in there, in both files, at the exact point already common to all
three request kinds (`InfoRefs`/`UploadPack`/`ReceivePack` in `http.rs`,
both `git_subcmd` branches in `ssh.rs`).

This one call runs `gitquarry-mirror-external sync --repo <uid>` — the same
binary/subcommand the post-push spawn already uses — but **synchronously,
bounded by a 5s timeout** (`mirror::SYNC_BEFORE_SERVE_TIMEOUT`), not
detached. A hard requirement carried over from every other decision in this
doc: an external forge being slow or down must never turn into an unbounded
hang on the single most latency-sensitive path in core. Any timeout,
non-zero exit, or spawn failure just logs and falls through to serving with
whatever the local mirror already has — identical to today's "no external
mirror configured" behavior. Zero overhead for the common case: the first
thing the function does is check `storage::settings::has_enabled_mirror`
(the same indexed hint from Phase 1), returning immediately without ever
resolving or spawning the binary when a repo has no enabled mirror.

Deliberately still a synchronous *wait*, not fire-and-forget, even though
that's a step away from "core never depends on this crate's availability
for correctness" — the request handler doesn't error or degrade
functionally if the sync fails, it just serves slightly staler data, so the
"never blocks basic functions" invariant is preserved as "never fails",
while genuinely adding the latency needed to make "sync before you read/write"
true rather than aspirational.

Not covered: the pre-receive *hook* itself (`mirror::hook.rs`, which runs
after the push's objects are already received, deciding accept/reject) does
not get a second sync call — by the time `receive-pack` starts running, the
pre-serve sync above has already run once for this same request, and
running it twice per push would double the worst-case latency for no
additional freshness.

Commit: `feat(mirror): sync external mirrors before serving upload/receive-pack`.

## Phase 7 — reverted to in-process (library, not a process) — **supersedes Phases 3–6's subprocess/binary shape**

**Explicit correction from the user, after Phase 6 shipped**: separating
`gitquarry-mirror-external` into its own *crate* was meant to help code
organization and let the subsystem grow independently — it was never meant
to also become a separately-*run* process. Phases 3–6 above conflated the
two (crate boundary = process boundary), the same assumption that made
sense for tracker/ci/federation/artifacts but doesn't have to hold here.
This phase keeps the crate, removes every subprocess/binary concern it grew
along the way.

- **`gitquarry-mirror-external` loses its `[[bin]]` target and `main.rs`
  entirely** — pure library crate now, same shape as `gitquarry-domain`.
- **The one real constraint a subprocess was solving for the post-push
  trigger doesn't survive**: `mirror::hook::run_pre_receive_hook` runs as a
  short-lived process git kills the moment it exits, so it can't hold an
  in-process `tokio::spawn`ed task past its own return the way the
  long-running main server can — only a genuinely separate OS process
  could outlive it. Rather than keep a subprocess just for this one caller,
  the immediate post-push trigger is dropped outright. What replaces it:
  the repo gets synced the next time anyone reads or writes it (the
  pre-serve hook below, already a hard requirement from a separate,
  earlier ask) or on the next periodic sweep (now in-process, see below) —
  losing "immediately after this specific push" in exchange for never
  running a second binary at all.
- **The pre-serve sync (`mirror::sync_external_before_serving`, called from
  `git::ssh`/`git::http`) and the periodic sweep both already run inside
  the long-lived main server process** (`ssh.rs`/`http.rs` request handlers,
  and `serve::run`'s own startup respectively) — neither actually needed a
  subprocess to begin with once the post-push case above is out of the
  picture. Both become plain in-process calls.
- **The core↔mirror-external cycle problem this doc's Context section
  already flagged (`gitquarry-mirror-external` depends on `gitquarry-core`,
  so core can't depend back) is real and doesn't go away just because the
  call is now in-process** — solved with a small hook-point trait instead
  of a subprocess boundary: `gitquarry_core::mirror::MirrorSyncHook`
  (`#[async_trait]`, `Send + Sync`), a new `AppContext.mirror_sync:
  Option<Arc<dyn MirrorSyncHook>>` field (`None` = nothing wired, same
  "`None` = feature fully off" pattern every other optional link uses), and
  `gitquarry-server` — the one crate that already depends on both core and
  gitquarry-mirror-external — implements it
  (`gitquarry-server/src/mirror_sync.rs::MirrorExternalHook`) and wires
  `ctx.mirror_sync = Some(Arc::new(MirrorExternalHook))` at startup. This is
  the concrete shape of "core has types/traits, server wires the
  externalized implementation into the main service" the whole crate
  reorganization conversation had already converged on independently.
- `MirrorExternalHook::sync_before_serving` owns the `tokio::time::timeout`
  bound (5s) itself now — core's `sync_external_before_serving` just calls
  whatever's wired (or nothing, if `None`) and trusts the implementation to
  never propagate an error or hang; the timeout constant moved from
  `gitquarry-core::mirror` to `gitquarry-server::mirror_sync` accordingly.
- **Dead once the subprocess model is gone**: `AppContext::for_mirror_sync`
  (was only for the subprocess entrypoint's placeholder context — no
  subprocess, no need for a placeholder context at all now, the real
  already-running `AppContext` is passed straight through),
  `config::mirror_sync_exe_path`/`auto_spawn_mirror_daemon` (no binary path
  to resolve, no "should I spawn a daemon" toggle — the periodic sweep is
  just another `tokio::spawn` alongside `gc::run_periodic_sweep`, always
  on), `mirror::hook::spawn_mirror_sync`/`inherited_git_vars` (the dropped
  post-push trigger and its now-orphaned env-scrubbing helper).
- `async-trait` moves back from a dev-dependency to a real one in
  `gitquarry-core` (the crate itself now needs `#[async_trait]` for
  `MirrorSyncHook`, not just its test fakes) and appears fresh in
  `gitquarry-server` (for the trait impl).

Commit: `refactor(mirror): call the sync engine in-process instead of spawning a binary`.

## Verification

- `cargo build --workspace` + `cargo test --workspace` at each phase boundary — Phase 1 in particular: the build failing on a stray `access::` import is the real confirmation the index relocation didn't miss a caller, same discipline the domain-crate scope correction relied on.
- `cargo clippy --workspace --all-targets` / `cargo fmt --check`, `unsafe_code = forbid`, no exceptions in the new crate.
- Manual (post-Phase-7, supersedes Phase 6's subprocess-oriented check): push to a repo with an enabled mirror, confirm the *next* clone/fetch/push against that repo picks up the sync (pre-serve hook) without hanging even if the external forge is unreachable (bounded by `MirrorExternalHook`'s 5s timeout). Confirm the periodic sweep runs as an ordinary in-process task from server startup (no second process to find/kill in `ps`).
- Existing `tests/mirror_external.rs`, `mirror_external_access.rs`, `mirror_external_pr_sync.rs` move to the new crate's own `tests/` (renamed `engine.rs`/`access.rs`/`pr_sync.rs`), plus the mirror-specific cases split out of core's own `tests/git.rs`/`validate.rs` into new `tests/git_remote.rs`/`validate.rs` here — all still built directly against `gitquarry_core::app::AppContext`, per the deviation noted above.

## Plan de commits (atómico)

0. [x] `docs: add mirror-external-extraction-concept design doc`
1. [x] `refactor(mirror): relocate the mirrors-enabled index to storage`
2. [x] `feat(mirror): split mirror::external into its own crate and binary`
3. [x] `feat(mirror): sync external mirrors before serving upload/receive-pack`
4. [x] `refactor(mirror): call the sync engine in-process instead of spawning a binary`
