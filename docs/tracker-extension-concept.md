# Tracker extension (feature-flagged, not separate binary)

## Context

`docs/tracker-concept.md` designs Tracker (issues + pull requests + board +
milestones) in full, assuming it's permanently part of `gitquarry-core`. This
doc amends only the **placement** decision: Tracker becomes an optional cargo
feature, so a minimal deploy (bare git host + read-only web browsing, no
issue tracker) can compile it out entirely — the same "deploy minimal" goal
`gitquarry-artifacts`/`gitquarry-ci` serve, but reached differently here: no
new process, because Tracker has neither a peer-trust boundary (federation's
reason) nor large-blob/isolated-execution traffic (artifacts'/CI's reason).
Its storage is plain `s3db` entities in the same bucket core already owns —
a second process would add IPC overhead for zero isolation benefit.

Everything in `tracker-concept.md`'s phase plan (workflow states, board as a
real view, sprints, dependencies, automation, reporting) is unchanged by this
doc. Only where the code lives, and whether it's compiled, changes.

## Design decisions (settled, not open)

| Question | Decision | Why |
|---|---|---|
| Where does it live | New crate `gitquarry-tracker`, holding `storage::issues`, `storage::board`, `storage::milestones`, `domain::issue`, `domain::milestone`, `tracker::automation` (Phase 4 of `tracker-concept.md`) — pulled out of `gitquarry-core`'s own tree. | Same-binary boundary, but a real crate boundary so it can be feature-gated cleanly instead of `#[cfg(feature)]`-sprinkled through core's existing modules. |
| Webui view modules | **Stay in `gitquarry-webui`** (`webui/repo/{issues,board,milestones}/`), `#[cfg]`-gated in place — they do **not** move into `gitquarry-tracker`. | `gitquarry-tracker` must depend on `gitquarry-core` (needs `AppContext`/`Remote`/i18n); if the webui view modules moved there too, `gitquarry-tracker` would also need `AppError`/`page_chrome`/`components::label_pill` from `gitquarry-webui` — a dependency cycle. Moving only storage+domain avoids a third crate. |
| Compilation | Two feature flags, not one. `gitquarry-core`: `tracker = []`, a **dependency-free behavior flag** (core never links the tracker crate, even behind `cfg`). `gitquarry-webui`/`gitquarry-server`: `tracker = ["dep:gitquarry-tracker", ...]`, default-on — webui *may* link tracker, since that's where synchronous cross-boundary calls legally live. `--no-default-features --features <rest>` on `gitquarry-server` is how a minimal deploy opts out. | Core cannot depend on tracker without a cycle (tracker depends on core); splitting the flag in two is what makes "PR still needs *something* tracker-shaped sometimes" resolvable without core ever calling into the optional crate. |
| Runtime absence | Feature off ⇒ routes don't exist at all — no 503, because there's no separate process to be absent (unlike federation-shaped routes, which are always compiled in and 503 only at runtime for missing config). | Compile-time gate, not a runtime one; matches how the feature is actually implemented. |
| Nav | `page_chrome.rs`'s top-level `tab_defs` becomes a `Vec`: tracker-on pushes the existing merged Tracker tab; tracker-off pushes a `pulls` tab instead, so PR gets its own top-level entry. `project_subnav` (the issues/pulls/board/milestones sub-nav) is gated whole-function. | PR must have *somewhere* to live in the nav when tracker is absent — the existing merged-tab design (`tracker-concept.md`'s own doc comment) assumed tracker always present. |
| Search index | `storage::search_index` gains a build-time capability flag; a tracker-disabled instance never emits issue/PR documents into the index — no new runtime branch beyond what already decides what gets indexed. | Consistent with the rest of the index already being fed by whichever domain mutations exist in the build. |
| Event bus | Issue `RepoEvent` emission points move with `domain::issue`/`domain::milestone` into the gated crate. PR's own emission (`RepoEvent::PullRequest`) stays in core — auto-close-on-merge is now a tracker-registered *consumer* of that event, not a direct call from PR into issue code (see PR decoupling below). A tracker-disabled instance's event bus only ever emits `Push`/`PullRequest`/`Release`. | Removes the direct `domain::pull_request::merge → domain::issue::close` call, which was the actual hard coupling — not just "which crate owns the enum variant." |

## PR decoupling

Confirmed real coupling before this redesign (not incidental): `PullRequestMeta.milestone` crosses the `gitquarry-wire` IPC boundary; PR merge called `domain::issue::close` directly for "closes #N"; PR rendering called into milestone listing and the issues module's `label_pill` helper; PR's own top-level nav tab was merged into the same one as issues/board/milestones. Concrete mechanism per point:

1. **Auto-close-on-merge**: the direct `domain::issue::close` loop is deleted from `domain::pull_request::merge`. `gitquarry-tracker` registers a consumer on the existing `RepoEvent` bus (`events/mod.rs`, real code today, no consumers registered yet) reacting to `RepoEvent::PullRequest{action: Merged}` — it re-fetches the PR via `domain::pull_request::get` (public, stays in core) and re-parses title+description with `closing_issue_numbers` (made `pub`) to find issue numbers to close. Registered once in `gitquarry-server`'s startup, right after `Arc::new(AppContext{..})`, under `#[cfg(feature = "tracker")]`. The consumer closure captures a `Weak<AppContext>`, not `Arc` (an `Arc` capture through `ctx.events` would be a reference cycle).
2. **Milestone-on-PR**: `PullRequestMeta.milestone: Option<u64>` stays an unconditional wire field — only its doc comment changes (no longer names a `gitquarry-core` module it can dangle-reference). Validation moves out of `domain::pull_request::edit_metadata` and into `webui/repo/pulls/handlers.rs` at the edge, where `#[cfg(feature = "tracker")]` can legally call `gitquarry_tracker::domain::milestone::get`. Core's own field write-through becomes cfg-gated: tracker-on writes it normally, tracker-off leaves an existing value untouched (never rejects, never silently clears real data). Milestone title/dropdown rendering is computed in a cfg'd `let` before the `html!` macro, empty when tracker is off — this also removes an unconditional `list_milestones` round-trip from every PR page render in a tracker-off build.
3. **`label_pill`**: moved (not gated) from `webui/repo/issues/mod.rs` to `webui/repo/components.rs` — a neutral shared-helpers home, since PR labels are `RepoConfig.labels` (core-owned, unrelated to tracker). No feature flag needed here at all.
4. **`link_number_mentions`** (`#123`-linking, shared by issue and PR text): stays in `components.rs`; only the issue-lookup half of its resolution loop is `#[cfg(feature = "tracker")]`. Fallback when tracker is off: still tries to resolve `#N` as a PR (not plain text) — PRs exist in every build, and a working link beats a deliberately broken one.
5. **Dispatch**: `webui/repo/mod.rs`'s single `match view` extracts its five tracker arms (`issues`/`issue`/`new_issue`/`milestones`/`board`) into `render_tracker_view(view, args) -> Option<Result<...>>` — `None` when not a tracker view (always `None` in a tracker-off build). The fallthrough arm calls it and falls back to the existing `unknown_view` 400 when `None`. PR's own arms stay untouched in the main match. Mutation routes (`/_issues*`/`/_milestones*`/`/_board*` in `webui/mod.rs`) are extracted into a `tracker_routes(router) -> Router` builder, no-op when the feature is off.

## Implementation

1. Move `label_pill` (`webui/repo/issues/mod.rs`) to `webui/repo/components.rs`;
   reword `repo_settings::labels_heading` ("issue labels" → "labels"). Pure
   refactor, no flag yet.
2. Extract `render_tracker_view`/`RepoViewArgs` from `webui/repo/mod.rs`'s
   `match view`, extract a `tracker_routes(router) -> Router` builder from
   `webui/mod.rs`'s route table, convert `page_chrome.rs`'s `tab_defs` to a
   `Vec`. Still no flag — mechanical, independently testable no-ops.
3. Extract `close_issues_referenced_by` out of `domain::pull_request::merge`,
   register it as a `RepoEvent` consumer (temporarily still calling core's
   own `domain::issue`, before the crate split exists) — verify auto-close
   end to end before moving anything.
4. Move milestone validation from `domain::pull_request::edit_metadata` to
   `webui/repo/pulls/handlers.rs`; add the cfg'd write-through in
   `edit_metadata`.
5. Create `gitquarry-tracker`; move `storage::{issues,board,milestones}`,
   `domain::{issue,milestone}` (**not** `webui/repo/{issues,board,milestones}/`,
   which stay in `gitquarry-webui`, `#[cfg]`-gated in place — see the
   "webui view modules" decision above) + the close-helper + consumer
   registration + the auto-close tests into it.
6. Add both feature flags and every `#[cfg]` from the PR-decoupling section.
   CI: `cargo check -p gitquarry --no-default-features` and
   `cargo test --workspace`, both flag states.

## Critical files (new/moved)

- `crates/gitquarry-tracker/src/{issue,board,milestone,automation,events}.rs` (new crate — storage+domain only)
- `crates/gitquarry-core/Cargo.toml` (`tracker = []`, dependency-free)
- `crates/gitquarry-webui/Cargo.toml`, `crates/gitquarry-server/Cargo.toml` (`tracker = ["dep:gitquarry-tracker", ...]`, default-on)
- `crates/gitquarry-core/src/domain/pull_request/mod.rs` (auto-close removal, milestone validation removal)
- `crates/gitquarry-core/src/events/mod.rs` (consumer `Weak` invariant doc)
- `crates/gitquarry-webui/src/webui/repo/mod.rs` (`render_tracker_view` dispatch split, `#[cfg]`-gated module declarations in place)
- `crates/gitquarry-webui/src/webui/repo/pulls/{mod,handlers}.rs` (milestone edge-validation/rendering)
- `crates/gitquarry-webui/src/webui/repo/components.rs` (`label_pill` destination, `link_number_mentions` gate)
- `crates/gitquarry-webui/src/webui/mod.rs` (`tracker_routes` builder)
- `crates/gitquarry-webui/src/webui/page_chrome.rs` (nav split)
- `crates/gitquarry-wire/src/pull_request.rs` (doc comment only, field unchanged)
- `crates/gitquarry-server/src/serve.rs` (consumer registration)
- `crates/gitquarry-core/src/storage/search_index.rs` (build-time capability flag)

## Verification

- `cargo build --no-default-features --features <everything but tracker>` compiles clean, no dangling references from core/webui into the gated crate.
- `cargo test` both with and without the `tracker` feature at each phase boundary.
- Manual: subnav/routes absent in a `tracker`-off build; identical behavior to today in a `tracker`-on build.
- `cargo clippy --all` / `cargo fmt --check`, `unsafe_code = forbid` — no exception.
- Commit per phase, one logical change per commit.
