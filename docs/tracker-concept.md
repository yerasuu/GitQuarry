# Tracker (issues + pull requests + board + milestones)

## Context

The webui's top-level "issues" tab was renamed **Tracker** — issues, pull
requests, board, and milestones already live one level down as
`project_subnav`'s four entries (`webui/page_chrome.rs`), and the group as a
whole is one "project management" concern, not four unrelated sections. This
doc plans closing the gap between that group and Jira/Azure Boards-level
project management, without adopting either's full complexity.

Two structural gaps block everything else:

1. **`IssueMeta.status` is a fixed `Open`/`Closed` enum.** There is no
   per-repo custom workflow (`To Do`/`In Progress`/`Review`/`Done`), so a
   board column can never represent a real status.
2. **The board is a shadow list, not a view.** `webui/repo/board/handlers.rs`
   moves a `BoardCard` between columns without ever touching the issue it
   points at — dragging a card to "Done" doesn't close the issue. Any
   sprint/backlog/automation work built on top of the board today would be
   built on top of state that silently disagrees with the issue itself.

Everything below is sequenced so Phase 0 fixes both before anything else
(sprints, dependencies, automation) is built on top of them.

## Design decisions (settled, not open)

| Question | Decision | Why |
|---|---|---|
| Workflow states | New `RepoConfig.workflow_states: Vec<WorkflowState> { id, name, terminal: bool }`, embedded directly in `RepoConfig` — same pattern `labels: Vec<Label>` already uses, not a new `s3db` entity. | A small, bounded, repo-scoped list with no independent lifecycle of its own — exactly the shape `Label` already established; a dedicated collection would be the wrong tool per `CLAUDE.md`'s own `s3db` guidance. |
| Issue status | `IssueMeta.status` becomes a `String` state id, validated against `RepoConfig.workflow_states` at write time (same "labels must exist in `RepoConfig.labels`" rule `domain::issue::edit` already applies). `IssueStatus::{Open,Closed}` stays alive as a *derived* value from the current state's `terminal` flag. | Every existing consumer of open/closed (milestone `open_issues`/`closed_issues` counters, search filters, PR `closes #N` auto-close) keeps reading a boolean — they don't need to learn the full per-repo state set, only Phase 0's transition code does. |
| `domain::issue::close`/`reopen` | Replaced by one `transition(ctx, target, number, to_state, locale)`, validated against the repo's configured states (illegal transition = rejected, no state graph/edges in v1 — any state to any state is allowed, same "keep it simple until someone needs more" posture the rest of the domain layer follows). `close`/`reopen` become thin callers passing the repo's first/last `terminal` state. | Existing callers (webui issue-detail close/reopen buttons, PR merge auto-close) don't need to change signature at the call site, only what state id gets passed. |
| Board ↔ workflow binding | `BoardColumn` gains `bound_state: Option<String>`. Moving a card whose `CardRef` is `Issue(n)` into a bound column calls `domain::issue::transition` server-side as part of the same move; an unbound column (or a `PullRequest`/`Note` card) behaves exactly as today. | Makes the board a *view* over real issue state instead of a second, driftable copy of it — the core fix Phase 0 exists for. Opt-in per column (not forced) because a board is also used today for pure planning columns that don't map to a status. |
| Sprint | No new entity. `MilestoneMeta` gains `start_date: String` (same free-text tolerance `due_date` already has) and `is_sprint: bool`. | A milestone is already "a bucket of issues with a due date and open/closed counters" — exactly what a sprint is. A parallel `Sprint` entity would need its own numbering counter and its own field on every `IssueMeta`, duplicating what `milestone: Option<u64>` already does. |
| Epic / hierarchy | `IssueMeta` gains `parent: Option<u64>` (an issue number in the same repo). An "epic" is just an issue with children pointing at it — no separate `IssueType` enum in v1. | Mirrors how `milestone: Option<u64>` already resolves by number (`domain::milestone::get_milestone`); avoids a second hierarchy concept (issue type *and* parent link) when one field covers it. |
| Dependencies | `IssueMeta` gains `blocks: Vec<u64>` / `blocked_by: Vec<u64>`, kept symmetric (adding one side writes the other) and validated at edit time the same way `labels`/`assignees`/`milestone` already are. | Closes the gap `feature-compare.md` flags — today the only cross-issue relationship is the PR `closes #N` keyword at merge time. |
| Estimation | `IssueMeta` gains `estimate: Option<u32>` — unitless story points, not hours. | Deliberately *not* time tracking (worklogs, timers) — a separate, larger feature with its own UI, out of scope here (see below). Points are enough to drive velocity/burndown in Phase 3. |
| Transition history | New `IssueTransition { at, from_state, to_state, by }`, appended to a bounded ring on `IssueMeta` (cap 50, oldest evicted — same shape `WebhookDelivery` already uses). | The one thing Phase 3's burndown/cycle-time charts need that nothing today records: *when* an issue moved between states, not just its current one. |
| Automation | New consumer of the internal `RepoEvent` bus `docs/webhooks-concept.md` Phase 1 already designs (`events::emit`), not a new event source. Rules v1: fixed trigger/action pairs (`pull_request.merged → transition issue to <state>`, `label added → move card to <column>`), not a generic rule DSL. | One event bus, three consumers (webhooks, CI, tracker automation) — building a second emission point here would repeat exactly the mistake that doc's Phase 1 comment already calls out. A fixed pair list ships something useful without a rule-engine design pass nobody's asked for yet. |
| Reporting | Computed from `IssueTransition` history + per-state counts (`MilestoneMeta.open_issues`/`closed_issues` generalized to per-state) on read. No separate analytics store/materialized view in v1. | Same "best-effort counter, self-heals" tolerance the codebase already applies to `open_issues`/`pack_bytes`/`fork_count` — a dashboard that's occasionally one issue stale is an acceptable trade against standing up a new storage concern. |

## Implementation

### Phase 0 — workflow states + status model rework (blocking)

- `storage/entities.rs`: `WorkflowState { id, name, terminal: bool }`, `RepoConfig.workflow_states: Vec<WorkflowState>` (default: two states, `open`/`terminal=false` and `closed`/`terminal=true`, so an un-migrated repo behaves exactly as today).
- `storage/issues/mod.rs`: `IssueMeta.status` becomes `String`; `IssueStatus` enum kept only as the derived open/closed helper (`IssueMeta::is_open(&self, states: &[WorkflowState]) -> bool`).
- `domain/issue.rs`: `close`/`reopen` replaced by `transition`; both existing names become thin wrappers so callers in `webui/repo/issues/` don't change shape.
- Migration: an `IssueMeta` read with the old enum-shaped `status` deserializes via a `#[serde(alias)]`/manual fallback into the default two-state ids, one time, on next write — no bulk migration command needed given the default states reproduce old behavior exactly.
- Repo settings: new "workflow" section (mirrors the existing `labels` settings page) to add/rename/reorder/delete states, guarded so a state referenced by an open issue can't be deleted out from under it.

### Phase 1 — board becomes a real view + sprint/backlog

- `storage/board.rs`: `BoardColumn.bound_state: Option<String>`.
- `webui/repo/board/handlers.rs`'s `move_to_column` handler: when the moved card is `CardRef::Issue` and the destination column has `bound_state`, call `domain::issue::transition` in the same request before returning.
- `storage/milestones/mod.rs`: `MilestoneMeta.start_date`, `is_sprint`.
- New backlog view (`webui/repo/issues/`): issues with no `milestone` set, or with a non-sprint milestone — the "not yet planned" pool.
- Board gains swimlanes (group cards by assignee, client-side row grouping over the existing column data — no new storage) and a per-column WIP soft-limit (a count shown, not enforced — enforcing would need a hard reject on `add_card`/`move_to_column`, deliberately left advisory in v1).

### Phase 2 — hierarchy, dependencies, estimation

- `storage/issues/mod.rs`: `parent`, `blocks`, `blocked_by`, `estimate` fields.
- `domain/issue.rs::edit`: validate `parent`/`blocks`/`blocked_by` resolve to real issues in the same repo (same shape the existing `milestone` validation already has); write the symmetric side of `blocks`/`blocked_by` in the same CAS-edit.
- Issue detail view: parent breadcrumb, child-issue list (query by `parent == this.number`, same "resolve at read time" tolerance `milestone`'s back-reference already accepts), blocked-by banner.

### Phase 3 — transition history + reporting

- `storage/issues/mod.rs`: `IssueMeta.transitions: Vec<IssueTransition>` (capped ring), appended inside `domain::issue::transition`.
- `MilestoneMeta`: generalize `open_issues`/`closed_issues` to `counts_by_state: BTreeMap<String, u32>`, maintained the same incremental way the two existing counters already are.
- New sprint/milestone report view: burndown (remaining `estimate` sum over time, derived from transition timestamps crossing into a terminal state), simple cumulative-flow (count per state per day, computed from transition history on read — no background aggregation job in v1).
- Saved filters: a `SavedFilter { name, query }` list on the repo (embedded, `RepoConfig`-adjacent, same reasoning as `workflow_states`), `query` a small structured filter (status/assignee/label/milestone AND-only) — not a full JQL-style parser.

### Phase 4 — automation

- New `tracker::automation` consumer registered against the `RepoEvent` bus (`docs/webhooks-concept.md` Phase 1).
- Fixed rule set v1: `AutomationRule { trigger: PrMerged | LabelAdded(String) | IssueTransitioned(String), action: TransitionIssue(String) | MoveCardToColumn(usize) }`, stored per repo (embedded list, same shape as `SavedFilter`).
- Settings UI: add/remove rules from the fixed trigger/action pairs above (dropdowns, not free text) — a v2 generic rule builder is explicitly deferred, see below.

### Phase 5 — bulk actions, templates, multi-board, per-board permissions

- Bulk issue actions (multi-select on the issues list → close/reopen/label/assign N at once).
- Issue templates (`.gitquarry/ISSUE_TEMPLATE/*.md`, same discovery idea the PR-template gap already names in `feature-compare.md`).
- Multiple named boards per repo (`Board` keyed by name instead of the current single implicit board) with independent column sets.
- Per-board manage permission (today board management is flat `Developer+` repo-wide) — a `board_managers: Vec<String>` override list, same shape `RepoConfig`'s other override lists already use.

## Explicitly out of scope

- **Time tracking** (worklogs, timers, hours-vs-estimate burn) — a separate, larger feature (needs its own UI surface and a hours-logged data model); `estimate` (Phase 2) is points-only on purpose.
- **Cross-repo/org portfolio boards** — needs a cross-repo aggregation layer of its own; no current use case scoped.
- **Generic custom fields** (Jira's arbitrary field-configuration schemes) — `parent`/`blocks`/`estimate` cover the common case without a schema engine nobody's asked for.
- **Per-issue-type workflow schemes** — v1 is one workflow per repo, not "bugs use workflow A, stories use workflow B" (Jira's workflow scheme). Revisit only if a real repo hits the limitation.
- **Free-text/generic automation rule DSL** — Phase 4 ships a fixed trigger/action pair list; a full rule builder is a separate, larger design pass.

## Critical files (new/touched)

- `crates/gitquarry-core/src/storage/entities.rs` (`WorkflowState`, `RepoConfig.workflow_states`, `SavedFilter`, `AutomationRule`)
- `crates/gitquarry-core/src/storage/issues/mod.rs` (`status: String`, `parent`, `blocks`, `blocked_by`, `estimate`, `transitions`)
- `crates/gitquarry-core/src/storage/milestones/mod.rs` (`start_date`, `is_sprint`, `counts_by_state`)
- `crates/gitquarry-core/src/storage/board.rs` (`BoardColumn.bound_state`)
- `crates/gitquarry-core/src/domain/issue.rs` (`transition` replacing `close`/`reopen`, hierarchy/dependency validation)
- `crates/gitquarry-core/src/domain/milestone.rs` (`counts_by_state` bump logic)
- `crates/gitquarry-core/src/tracker/automation.rs` (new, Phase 4 — `RepoEvent` consumer)
- `crates/gitquarry-webui/src/webui/repo/board/handlers.rs` (`move_to_column` → `domain::issue::transition` call)
- `crates/gitquarry-webui/src/webui/repo/issues/` (backlog view, bulk actions, templates, workflow settings page)
- `crates/gitquarry-webui/src/webui/repo/milestones/` (sprint report view)

## Verification

- `cargo test` at each phase boundary — Phase 0's migration fallback (old enum-shaped `status` deserializing into the default two states) gets a direct unit test before anything else builds on it.
- Phase 1: integration test asserting a bound-column move actually flips the issue's `status`, and that an unbound column move doesn't.
- Phase 2: dependency validation tests (self-reference rejected, cross-repo reference rejected, symmetric `blocks`/`blocked_by` write).
- Phase 3: burndown/cumulative-flow computed from a scripted transition history, checked against hand-computed expected output.
- `cargo clippy --all` / `cargo fmt --check`, `unsafe_code = forbid` — no exception.
- Commit per phase, one logical change per commit.
