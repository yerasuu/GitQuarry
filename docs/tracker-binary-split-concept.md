# `gitquarry-tracker` binary split

## Context

Builds on `docs/tracker-concept.md` (the tracker feature roadmap: workflow states,
sprints, hierarchy/dependencies, transition history, reporting, automation). That
doc is unaffected by this one — it stays scoped per-repo (no cross-repo/portfolio
boards, a call it makes explicitly) and every phase it describes is still built on
top of whatever shape lands here. This doc adds exactly one thing: moving
`gitquarry-tracker` out of the same process as `gitquarry-core`/`gitquarry-webui`
into its own binary, talking back only over loopback HTTP — the same posture
`gitquarry-ci` (`docs/ci-cd-concept.md`) and `gitquarry-federation`
(`docs/federation-binary-split-concept.md`) already have. Core stays fully usable
without it: a deploy that doesn't configure `[tracker]` just loses the tracker tab,
the same "`None` = feature fully off" pattern `federation`/`ci`/`artifacts` already
establish.

**This is not a latency win — today, in-process, is strictly faster.** The
justification is deploy-shape: independent versioning/scaling/restart of an
optional feature, generalizing the compile-time-optional posture
`gitquarry-tracker` already has (`feature = "tracker"`) to a runtime/deploy-time
one. Every repo page that shows an issue/PR count, and the issues/board/milestones
tabs themselves, now cost a loopback round trip that used to be a plain Rust
function call. Worth it only if the deploy flexibility matters to an operator more
than that latency — a call each deployment makes for itself by configuring
`[tracker]` or not, not one this doc makes for them.

Four things settled by inventorying `gitquarry-tracker`'s actual coupling to
`gitquarry-core` before drafting (not assumed):

1. Tracker leans on core for: `AppContext`/`ResolvedRepo` (storage backend +
   cipher), storage primitives (`Backend`/`Cas`/`Uid`/`REPOS_PREFIX`/thread
   helpers/CAS-retry backoff), `auth::access::get_user` (assignee-exists check),
   `i18n::Locale` + a dozen `i18n::shared::*` error strings, `validate::validate_issue_title`,
   the in-process `RepoEvent` bus (both directions), and `domain::pull_request::get`
   + `closing_issue_numbers` (core's own PR domain layer, read for issue auto-close
   and board PR-card validation).
2. `RepoConfig.labels` is the *only* `RepoConfig` field tracker ever reads — no
   `visibility`/`protected_branches`/anything else.
3. `gitquarry-webui` calls tracker's `domain::*` functions directly today, `ctx: &AppContext`
   passed straight through, zero serialization boundary. Permission checks
   (`require_manage_permission`, `effective_permission_for_user`) already happen
   in `webui` *before* calling into tracker — tracker's own domain functions never
   make an access decision themselves beyond a narrow author-only check on
   comment edit/delete. That division doesn't change here.
4. `gitquarry-federation`/`gitquarry-ci`'s existing `core_client.rs` files are the
   working template for "a split-out binary's outbound calls into core" — same
   `CoreClient { base_url, internal_token, http: reqwest::Client }` shape, every
   call carrying `X-Internal-Token` + `gitquarry_wire::WIRE_VERSION_HEADER`.

## Design decisions (settled, not open)

| Question | Decision | Why |
|---|---|---|
| Where does it live | New binary `gitquarry-tracker`, same optional/separately-deployed posture as `gitquarry-ci`/`gitquarry-federation` — core stays fully usable without it. | Consistent with the project's established third/fourth-process precedent; no new architectural shape invented. |
| Config | `gitquarry-tracker.toml`, same env>cwd>gitroot>userglobal resolution as `gitquarry-ci.toml`/`gitquarry-federation.toml`, never merged with `gitquarry.toml`. | Matches the existing two-and-three-binary precedent exactly. |
| Storage | Own `s3db` bootstrap against the *same* bucket, own prefix (`repos/<uid>/{issues,board,milestones}` — unchanged paths, just written by a different process now). `REPOS_PREFIX` becomes a small locally-duplicated `&str` constant in `gitquarry-tracker` (it must stay byte-identical to core's own, but it's a single fixed literal, never going to drift — not worth a shared crate for one string). | Same "own bucket access, own prefix" pattern `gitquarry-ci`'s `ci/<repo-uid>/...` and `gitquarry-federation`'s peer/token namespaces already established. |
| IPC | Bidirectional loopback, same shared `internal_token` secret. **webui → tracker**: issue/board/milestone CRUD + reads — core/webui is the caller here, never a browser directly, so this is closer in shape to `gitquarry-ci`'s narrow `core_client.rs` than to `federation_proxy.rs`'s browser-facing byte-proxy. **tracker → core**: `repo_labels(uid)`, `user_exists(username)`, `pr_summary(repo, number)` (new/reused `/_internal/*` endpoints). **core → tracker**: a PR-merge notification push, mirroring `gitquarry-ci`'s own "core calls out to CI's loopback to push trigger events" leg (documented there as "not used yet" — this split is the first thing that actually exercises that shape). | Every direction is a variant already proven somewhere in this codebase; no new IPC shape invented, just a fourth user of the existing one. |
| Auth model | Tracker itself has **zero session/CSRF concept** — every call arrives from `webui` (still same-process as core) carrying the already-decided actor's username as plain request data, e.g. `{"actor": "alice", "title": "...", ...}`. `webui` keeps doing its own permission check in-process exactly as today, unchanged; tracker trusts the call because only `internal_token`-holding core ever reaches it, same posture `gitquarry-ci`'s loopback API already has. | Simpler than `federation_proxy.rs`'s cookie/CSRF-header-assertion dance, which exists specifically because federation's *own* routes are ultimately driven by a real browser request forwarded through core. Tracker never renders anything a browser sees directly (see next row) — it's pure backend-to-backend RPC with an already-authorized caller, same shape `domain::issue::create(ctx, resolved, title, body, &me.username)` already has today (`me.username` is already just a plain string argument). |
| Rendering | Stays in `gitquarry-webui`. Tracker is purely a JSON data + domain-logic API; it never produces HTML. `webui`'s existing `maud` render functions (`webui/repo/{issues,board,milestones}`) barely change — they already just read plain struct fields, now populated from a deserialized wire DTO instead of a direct storage read. | Avoids a second rendering/CSS/theme system the way `gitquarry-federation`'s remote-peer-browsing proxy has to have (that case is *literally* rendering another instance's own page) — this is the same instance's own feature, it should keep one visual system, not two. |
| Wire types | New `gitquarry-wire::tracker` module: read DTOs (`IssueSummary`, `BoardStateDto`, `MilestoneSummaryDto`, ...) and write DTOs (`CreateIssueRequest`, `EditIssueRequest`, `TransitionRequest`, ...) — minimal, only what actually crosses the `webui ↔ tracker` boundary. | Same "only types genuinely crossing 2+ boundaries" rule `gitquarry-wire`'s own doc comment already states; matches how `gitquarry-wire::pipeline` was scoped for the CI SDK split. |
| `closing_issue_numbers` | Reimplemented directly inside `gitquarry-tracker` (a small, pure text-parsing function — no I/O, a handful of lines) rather than shared. | Same "cheap to duplicate a few lines, not worth a shared crate" reasoning `gitquarry-graph-validate`'s design explicitly rejected for the *DFS cycle check* (non-trivial enough to justify a crate) — `closing_issue_numbers` is far simpler than that DFS and doesn't clear the same bar. |
| Error message catalog | Tracker gets its own small `i18n` module (same `msg!`-macro shape core's `i18n` already uses, En/Es), covering only the dozen or so strings tracker's domain layer actually emits today (`unknown_label`, `unknown_assignee`, `issue_not_found`, ...) — copied once, not imported from core. | Depending on `gitquarry-core`'s whole `i18n` module just for a dozen strings would reintroduce exactly the dependency this split exists to sever; duplicating a small, stable message list is cheap and matches the `closing_issue_numbers` reasoning above. |
| Events (tracker → core) | Tracker's own domain events (issue opened/closed) get forwarded to core via a new inbound `POST /_internal/tracker/event`, which core's own `EventBus` re-emits — core's bus stays the *single* source of truth for "one event source, many consumers" (webhooks, future automation), exactly the principle `docs/ci-cd-concept.md`'s own Phase 0 doc comment states for push events. | Keeps the "one event bus" invariant intact across the split instead of tracker growing its own parallel event source that webhooks/automation would then have to special-case. |
| Events (core → tracker) | Core's PR-merge code path gains a call through `AppContext.tracker` (if configured), POSTing `{repo_uid, pr_number, title, description}` to tracker's loopback — tracker's issue-auto-close logic still fires, just via a pushed notification instead of an in-process bus subscription. | Same shape as `gitquarry-ci`'s own (currently unused) core→ci trigger-push leg — the split is what finally gives that shape a real second user. |
| `tracker-concept.md` Phase 4 interaction | Its "automation" phase still works exactly as designed (fixed trigger/action pairs), but its *mechanism* updates from "register directly on `ctx.events`" to "receive events forwarded over the loopback push above" once implemented after this split lands. Not rewritten here — just flagged as the one adjustment Phase 4's implementer needs to make. | Keeps this doc scoped to the split itself, per the explicit choice not to revisit `tracker-concept.md`'s own content. |

## Structure once split

```
crates/gitquarry-tracker/
├── Cargo.toml            (drops gitquarry-core dependency entirely; gains gitquarry-wire)
├── src/
│   ├── main.rs
│   ├── config.rs         (TrackerToml: passphrase, listen, core_internal_url, internal_token)
│   ├── state.rs          (TrackerState: backend, cipher, core: CoreClient — mirrors gitquarry-ci::state)
│   ├── core_client.rs     (repo_labels, user_exists, pr_summary — mirrors gitquarry-ci/federation's own)
│   ├── i18n.rs            (tracker's own small En/Es message catalog)
│   ├── web/mod.rs         (loopback routes: issues/board/milestones CRUD + the /_internal/tracker/event push-out and PR-merge push-in)
│   ├── domain/{issue,board,milestone}.rs   (moved from core-coupled to backend/cipher-direct + core_client-backed)
│   └── storage/{issues,board,milestones}.rs (unchanged storage shape, local REPOS_PREFIX const)

crates/gitquarry-wire/src/tracker.rs   (new: IssueSummary/BoardStateDto/MilestoneSummaryDto + request DTOs)

crates/gitquarry-core/src/
├── app.rs                (new AppContext.tracker: Option<TrackerLink>, mirrors FederationLink/CiLink)
├── config/mod.rs          (new TrackerSection)
└── internal_api.rs        (new: /_internal/repos/labels, /_internal/users/exists, /_internal/tracker/event;
                             reused as-is: /_internal/pull-requests?repo=&number=)

crates/gitquarry-webui/src/webui/repo/{issues,board,milestones}/
└── handlers.rs            (gitquarry_tracker::domain::* calls replaced by ctx.tracker HTTP calls;
                             render functions (the html! {} bodies) untouched)
```

## Implementation

### Phase 1 — `gitquarry-tracker` skeleton (binary)

- New crate becomes bin+lib, mirroring `gitquarry-ci`'s exact shape: `main.rs`/`config.rs`/`state.rs`/`core_client.rs`/`web/mod.rs`, guarded loopback server, `internal_token`-checked.
- Own `s3db` bootstrap against the same bucket as core, using the new local `REPOS_PREFIX` const.
- `core_client.rs`: `repo_labels(uid)`, `user_exists(username)`, `pr_summary(repo, number)`.
- Core side: `config::TrackerSection` (mirrors `CiSection`/`FederationSection`), `AppContext.tracker: Option<TrackerLink>`, wired the same way `federation`/`ci` already are in `app.rs`.

### Phase 2 — new/reused core `/_internal/*` endpoints

- `GET /_internal/repos/labels?uid=` → `RepoConfig.labels` (new; `RepoSummary` deliberately not extended with this — federation's public listing doesn't need it).
- `GET /_internal/users/exists?username=` → `{ "exists": bool }` (new; narrower than any existing route, no `access::effective_permission_for` call needed).
- `GET /_internal/pull-requests?repo=&number=` → reused as-is (already returns `PrDetail`, sufficient input for both the auto-close text parse and the board PR-card existence check).
- `POST /_internal/tracker/event`: tracker's own outbound event push, re-emitted onto core's `EventBus`.
- Core's existing PR-merge code path gains an `if let Some(tracker) = &ctx.tracker` push to tracker's loopback.

### Phase 3 — `gitquarry-wire::tracker` DTOs

- Read shapes: `IssueSummary`, `BoardCardDto`/`CardRefDto`, `MilestoneSummaryDto` — same fields `IssueMeta`/`BoardCard`/`MilestoneMeta` already expose to `webui` today, just as a wire-crossing type instead of an in-process one.
- Write shapes: `CreateIssueRequest`/`EditIssueRequest`/`TransitionRequest`/board add-card/move-card/milestone create-edit requests — each carrying the already-decided `actor: String` field per the auth-model decision above.

### Phase 4 — move domain + storage logic into the tracker binary

- `domain/{issue,board,milestone}.rs` and `storage/{issues,board,milestones}.rs` move largely as-is, with these substitutions at every call site:
  - `AppContext`/`ResolvedRepo` params → tracker's own `TrackerState` (direct `backend`/`cipher`, no core dependency).
  - `access::get_user(...)` (assignee-exists check) → `core_client.user_exists(username)`.
  - `target.remote.get_repo_config().await?.labels` (label validation) → `core_client.repo_labels(uid)`.
  - `gitquarry_core::i18n::shared::*` calls → tracker's own new `i18n.rs` catalog.
  - `gitquarry_core::domain::pull_request::get`/`closing_issue_numbers` → `core_client.pr_summary(...)` + the small reimplemented parser.
  - `gitquarry_core::events::emit`/`ctx.events.register` → the `POST /_internal/tracker/event` push-out / the PR-merge push-in handler.
- `web/mod.rs` gains the actual loopback routes: list/get/create/edit/transition/comment for issues, add/move/remove-card for the board, create/edit/close for milestones.

### Phase 5 — `gitquarry-webui` becomes a thin HTTP client of tracker

- `webui/repo/{issues,board,milestones}/handlers.rs`: every `gitquarry_tracker::domain::*` call replaced by a call through `ctx.tracker` using the Phase 3 DTOs — same shape `webui` already uses for `ctx.federation`/`ctx.ci`. Render functions (the `html! {}` bodies) stay untouched; they already only read plain struct fields.
- `webui/repo/pulls/mod.rs`'s existing cross-read (pulling the milestone list into the PR view's dropdown) becomes another `ctx.tracker` call alongside the others.
- The compile-time-optional posture stays: a minimal build can still compile `webui/repo/{issues,board,milestones}` out entirely via `feature = "tracker"`, which now also gates `AppContext.tracker`/`TrackerLink` — mirroring exactly how the `federation`/`ci` features already gate their own `AppContext` fields. The existing `gitquarry-core/tracker` feature (today apparently a near-no-op passthrough, per the coupling inventory — core has no tracker-specific code of its own beyond what the split removes) gets re-examined during implementation for whether it still does anything at all.

### Phase 6 — migration / operational

- No in-process fallback is kept once this lands — same precedent `gitquarry-ci`/`gitquarry-federation` already set: once split, it's split. A deploy not wanting a fourth process simply doesn't configure `[tracker]` and loses the tab entirely.
- `deploy/docker-compose.yml` gains an optional `tracker` service block once the other split binaries (`ci`/`federation`/`artifacts`) have their own — none of the three do yet as of this doc, so there's no existing block to mirror; revisit this phase once one of them does.

## Explicitly out of scope

- Every `tracker-concept.md` feature phase (workflow states, sprints, hierarchy/dependencies, estimation, transition history, reporting, automation) — unaffected, still designed/built against whatever shape lands here, not revisited by this doc.
- Cross-repo/portfolio boards — `tracker-concept.md` already scopes this out explicitly; this doc doesn't reopen that call.
- Any change to `gitquarry-ci`/`gitquarry-federation`/`gitquarry-artifacts`'s own IPC shape — this doc only adds a fourth instance of the already-established pattern, no changes to the other three.

## Critical files (new/touched)

- `crates/gitquarry-tracker/{Cargo.toml,src/{main,config,state,core_client,i18n}.rs}` (rewritten: binary, no `gitquarry-core` dependency)
- `crates/gitquarry-tracker/src/{domain,storage}/{issue,board,milestone}*.rs` (touched: core-coupling substitutions per Phase 4)
- `crates/gitquarry-tracker/src/web/mod.rs` (new: loopback routes)
- `crates/gitquarry-wire/src/tracker.rs` (new: read/write DTOs)
- `crates/gitquarry-core/src/app.rs` (new `AppContext.tracker: Option<TrackerLink>`)
- `crates/gitquarry-core/src/config/mod.rs` (new `TrackerSection`)
- `crates/gitquarry-core/src/internal_api.rs` (new `/_internal/repos/labels`, `/_internal/users/exists`, `/_internal/tracker/event`; PR-merge push-out call site)
- `crates/gitquarry-webui/src/webui/repo/{issues,board,milestones}/handlers.rs` (touched: HTTP calls replacing direct domain calls)
- `crates/gitquarry-webui/src/webui/repo/pulls/mod.rs` (touched: milestone cross-read becomes a `ctx.tracker` call)

## Verification

- `cargo test` at each phase boundary — `gitquarry-tracker`'s own domain/storage tests move with it (now against a local `backend`/`cipher` fixture instead of a `gitquarry-core` `AppContext` one); `webui`'s existing tracker-page tests get a fake-tracker-loopback fixture, same pattern this session's `gitquarry-ci` work used (`tests/graph.rs`'s `fake_core` helper — a tiny `axum` stand-in bound to `127.0.0.1:0`).
- `cargo clippy --all` / `cargo fmt --check`, `unsafe_code = forbid` — no exception for the new crate shape.
- Manual: deploy with `[tracker]` unconfigured → confirm the tracker tab/nav entry disappears entirely, everything else (git, PRs, CI, artifacts) unaffected. Deploy with `[tracker]` configured → confirm issue create/edit/transition, board card move, milestone create, and PR-merge auto-close all still work end to end through the new loopback hop.
- Commit per phase, one logical change per commit — same criterion `docs/ci-cd-concept.md` and the SDK design doc both already follow.
