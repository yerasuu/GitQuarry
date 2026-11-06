# Publish mirror, inbound — a public pull request becomes a private merge request

Scope: the reverse direction of `docs/publish-mirror-concept.md`. A pull
request opened on the **public** repo (the one a publish mirror pushes to)
is translated into a merge request on the **private** repo the mirror
publishes from.

**Status.** Phase 0 (§7) is built; phases 1-3 are not. Built: the inverse
index (`publish/inverse/`, kb #118), the operator command
`publish-inverse-backfill` (kb #118), the `inbound_token` field of
`PublishMirror` (`publish/mod.rs`) and the command
`publish-inbound-dry-run` (`publish/inbound/`, kb #119, #122, #123, #125).
The dry run decides and reports; it writes nothing to the bucket or to the
private repo. Everything in §3 that lands a branch, a merge request or a
ledger entry is still design. Board task: #52. The owner answered the open
questions of §9 on 2026-11-06 (kb #117). Known gaps: §10.

**Notation.** `[V]` marks a statement verified in the code at the time of
writing (file and function cited). `[P]` marks a proposal: a design
decision, owner-approved through kb #117 where §9 says so, but not built.
Where a section describes what the dry run does, the text follows the code;
where the code differs from an earlier version of this document, the code
wins and the section says so.

## 1. Why inbound is not outbound run backwards

Outbound is a function: `public = rewrite(private, filter, epoch)`,
deterministic, total over the private history. Inbound needs the inverse of
a function that is neither injective nor surjective.

- **A public commit has no private counterpart.** `[V]` The mapping is
  `private → (public commit, public tree)` (`publish/mapping.rs`,
  `MapValue = [u8; 40]`); nothing stores the other direction.
- **Many private commits map to one public commit.** `[V]` A private commit
  that touched only excluded paths collapses onto its parent:
  `Rewriter::rewrite_commit` does `mapping.insert(private, parents.first())`
  and pushes nothing to `published`. So the public commit `P` stands for
  the commit `C_o` that produced it *and* every later private commit that
  collapsed onto it.
- **The filter is lossy.** `[V]` `rewrite_tree` drops excluded entries; the
  public tree is a strict subset of the private one. The public author never
  saw the excluded paths, so their change is a change against a tree they
  could only half see.
- **Trust runs the other way.** Outbound, the private side is trusted and
  the gates stop it leaking. Inbound, the public author is untrusted and
  what flows is content that will sit in a repo with secrets, CI and
  private maintainers. Different threat model, different gates (§3.12).
- **Blob ids survive, which is what makes inbound tractable.** `[V]`
  `rewrite.rs` module doc: "No blob is ever re-encoded at all — a kept blob
  keeps its id". A public blob id equals the private blob id at the same
  path, so "is the public author's old version the one we have?" is an
  id comparison, not a content comparison.

Core idea `[P]`: translate each public commit *exactly against the private
commit its parent came from*, where the public side's view and the private
tree agree on every non-excluded path by construction. Everything that has
moved on the private side since is then an ordinary merge-request conflict,
handled by the existing MR machinery, not by new code.

## 2. What exists today and what it gives us

| Piece | Where | Reusable for inbound |
|---|---|---|
| Private-to-public mapping, 256 sealed shards per epoch | `publish/mapping.rs` `Table<N>`, `epoch_prefix` | `Table::store` refuses a second value for a key, so the inverse is not a `Table`; it is its own shard type in `publish/inverse/` (§3.2). `[V]` |
| Forward rewrite, in-memory id computation | `publish/rewrite.rs` `Rewriter` | The round-trip check (§3.2) runs it on translated commits. `[V]` |
| Filter decision per path | `publish/filter.rs` `Filter::decide`, `excludes_published_path` | Decides which public-touched paths are allowed. `[V]` |
| Run log, gate records, `RunOutcome` | `publish/log.rs` `PublishLog`, `PublishRun`, `MAX_RUNS = 20` | Inbound records go beside it (§6). `[V]` |
| Credentialed fetch, redaction | `git_remote.rs` `fetch_ref_from_remote_url`, `redact_url` | Fetch of PR heads. `[V]` |
| Host allowlist | `validate::check_host_allowed`, called from `publish::create/update` | Unchanged. `[V]` |
| Forge clients | `gitquarry-forge-clients` `ForgeApi`, `GitHubForge`/`GitLabForge`/`CodebergForge` | PR listing, comments. `[V]` |
| MR sync pass, cursor, link entity | `mr_sync/mod.rs` `sync_merge_requests_once`, `ExternalPrLink`, `mr_discovery_cursor` | Pattern reused; code mostly not (below). `[V]` |
| MR storage and merge | `core/storage/merge_requests/mod.rs`, `core/domain/merge_request/mod.rs` (`create`, `merge`, `conflicts`, `missing_required_reviewers`) | The landing place. `[V]` |

What does **not** carry over, and is harder than the older doc's one
paragraph (§7 of the outbound concept) suggested:

1. **`mr_sync` cannot be reused as is.** `[V]` It is bound to
   `ExternalMirror` (`build_forge(mirror)` reads `mirror.provider`), but
   `PublishMirror` (`publish/mod.rs`) has `remote_url` and `token` and no
   `provider`. It also requires both branches to exist in the local
   manifest (`mirrored_branch`) and skips every PR whose branches are not
   mirrored, explicitly including fork PRs ("an MR opened from a fork can't
   be"). Most public PRs come from forks. `RemoteMr` carries branch *names*
   only: no head sha, no fork flag. So the forge trait needs head/base
   sha and a fork flag, and the fetch has to go through the forge's PR ref
   (`refs/pull/<n>/head` on GitHub and Gitea/Codeberg,
   `refs/merge-requests/<n>/head` on GitLab) instead of a branch name. `[P]`
2. **The MR entity has no origin field.** `[V]` `MergeRequestMeta`
   (`gitquarry-wire/src/merge_request.rs`) has `author`, `source_repo`,
   `source_branch`, `labels` and no provenance. Only `MrComment.origin`
   exists. The "from the public side" marker (§3.3) is a wire change or a
   label convention.
3. **Reviews were not bound to a commit; they are now.** `[V]` kb #112:
   `MrReview` carries `head_sha` (`gitquarry-wire/src/merge_request.rs`,
   `#[serde(default)]`) and `domain::merge_request::review_summary` counts an
   approval only if `review_is_current` for the head being merged. A review
   without a recorded head is never current. This holds for every MR, so the
   inbound MR needs no review rule of its own for force-pushes (§3.7, §3.8).
4. **Publish refuses a public ref the mirror did not push.** `[V]`
   `docs/publish-mirror-concept.md` §10: "A public ref at a value this
   mirror never pushed is never built on or forced over", and
   `RunOutcome::Diverged`. So *merging the PR on the public forge* with its
   button breaks outbound. Inbound must be closed by landing the change
   privately and letting the next publish carry it out (§3.2).
5. **Identity gate blocks by default.** `[V]` `UnmappedPolicy::Block` is the
   default; a contributor's public email is by definition not in the
   identity map, so the first translated-and-merged commit would block the
   next publish unless the gate learns about inbound commits (§3.6).
6. **`filter_epoch` bumps rewrite all public shas.** `[V]` `epoch.rs`. A PR
   opened against old-epoch history is based on shas the current epoch's
   map does not contain (§3.9).
7. **A branch in the private repo under `publish_branches` would be
   published.** `[V]` Only `refs/merge-requests/`, `refs/notes/`,
   `refs/gitquarry/` are never published
   (`NEVER_PUBLISHED_REF_PREFIXES`); a mirror with `publish_branches =
   ["**"]` would publish an `inbound/...` head branch. And
   `domain::merge_request::create` only accepts a source that is
   `refs/heads/<name>` (`branch_exists`). Inbound branches need a namespace
   the publisher excludes (§3.3).

## 3. The flow

### 3.1 Discovery `[P]`

Poll, not webhook, in phase 1-2; webhook later.

- Poll for the same reason `mr_sync` polls: no inbound port needed on the
  gitquarry side, the sweep (`run_periodic_sync`) already exists, and the
  cursor discipline (`mr_discovery_cursor`, `oldest_failed` so a failed
  import is retried, not skipped) is already worked out. A webhook is an
  optimisation and an authenticated public endpoint on the instance; defer.
- `ForgeApi::list_merge_requests(since)` returns PRs updated since a
  cursor. Extend `RemoteMr` with `head_sha`, `base_sha`, `head_is_fork`,
  `commit_count`, `changed_files` (all present in GitHub's pull payload; the
  GitLab and Gitea equivalents differ and need checking against their docs
  when built).
- Provider: `PublishMirror` gains `provider` (inferred from the host for
  the three `PUBLIC_FORGE_HOSTS`, explicit otherwise; `Other` has no API and
  inbound is off for it, as `build_forge` already bails).
- Only PRs whose **base branch is a published branch** of this mirror
  (`publish_branches` match) are considered; a PR against anything else has
  no private counterpart ref.
- Objects: fetch `refs/pull/<n>/head` from the public remote into a
  **quarantine bare repo**, never into the private repo's object store
  (same reasoning as the publish repo's separation, outbound §3: do not mix
  public-origin objects with the private store). `scratch::init_scratch`
  already creates a fresh `git init --bare --object-format=sha1`. The
  fetch is `transfer.fsckObjects=true`, no checkout, no hooks run (bare
  repo, plumbing only), no submodule recursion. `[V]` for the dry run
  (`git_remote.rs`, `publish/inbound/quarantine.rs`): the pull ref is
  fetched into `refs/inbound/head` of a temporary quarantine repo removed
  when the run ends, with `fetch.fsckObjects` and `transfer.fsckObjects`, no
  tags and no submodules; the quarantine directory is measured while the
  fetch writes and the fetch is stopped over `MAX_QUARANTINE_BYTES` (§3.12),
  with a wall-clock cap `FETCH_TIMEOUT`. The dry run takes the base branch
  from `--base` (the forge API is not used in phase 0) and the pull ref from
  the provider: `refs/pull/<n>/head` for GitHub and Codeberg,
  `refs/merge-requests/<n>/head` for GitLab (`inbound/mod.rs` `pull_ref`).
  `[P]` for the rest: caps from the forge payload (`commit_count`,
  `changed_files`) before fetching.
- Link entity per PR: `repos/<uid>/publish-inbound/<mirror_id>/<pr>`
  (s3db, `Collection::new_owned`, like `publish-mirrors` and
  `publish-log`): head sha seen, translated private head, local MR number,
  state, refusal reasons, previous revisions. Same role as `ExternalPrLink`.

### 3.2 Mapping back: the inverse index `[V]`

Needed: given a public commit `P`, the private commit it came from.

`[V]` Built in `publish/inverse/mod.rs` (module doc), kb #118. Stored at
`repos/<uid>/publish/<mirror_id>/<epoch>/inverse/<xx>`, sharded by the first
byte of the **public** commit id, sealed with the key as AAD like every
other object. A shard is the magic `GQI1` followed by entries in strictly
ascending public order, each `public (20 bytes) | n (u32, big-endian, >= 1) |
n private ids (20 bytes each, strictly ascending)`. Anything else, trailing
bytes included, is refused as corrupt.

It is **not** a `Table<20>`. `Table::store` hard-errors when a key is stored
with a different value (`store_shard`: "already mapped to something else in
this epoch"), which is right for the forward map and wrong here: several
private commits collapse onto one public commit, and two private commits can
in principle produce byte-identical public commits. So a public commit maps
to a **set** of private commits, collapsed ones included, and a store is a
union under CAS (`STORE_ATTEMPTS`): what a shard holds is kept, an id already
listed is not written again, a shard that gains nothing is not rewritten.

Written by every history run (`history/run.rs`), through
`inverse::fresh_entries` over the `Rewriter`'s fresh commits (each under the
public commit it maps to, collapsed ones under the public commit they
collapsed onto), and **before** the forward map, because a commit the map
lists is seeded and never rewritten again, so its inverse entry would
otherwise be lost after a crash between the two writes.

Which private commit is the base? The set holds the commit that produced the
public commit (`C_o`) and every later one that collapsed onto it. The
dry run takes `C_o` as the member of the set none of whose parents is in the
set (`inbound/translate.rs` `originating`); the others are counted as
`collapsed_onto_base` in the verdict. `[V]` Two roots, or a head with more
than one merge base against the public branch, is the refusal
`AmbiguousBase`. A base that several collapsed commits share is therefore
**translatable**, not refused.

Why `C_o` is a valid base even though later private commits collapsed onto
`P`: the translated commit has parent `C_o` and a tree equal to `C_o`'s tree
with the public author's changes overlaid. When the MR is merged into the
private target, the merge base is `C_o` and git three-way-merges the
private-only changes made since (§3.10). Nothing is lost; those changes
simply appear as the other side of the merge.

- **Backfill.** `[V]` An epoch published before the index existed has none.
  `publish-inverse-backfill [--apply]` (`inverse/command.rs`,
  `inverse/backfill.rs`) rebuilds it by **inverting the forward map**, which
  already holds every private commit the epoch rewrote, collapsed ones
  included. This needs no private repo, no rewrite and no objects, and does
  not depend on private commits that were force-pushed away since. It is
  additive like every inverse write, so running it over an epoch whose runs
  already wrote the index changes nothing. This replaces the earlier idea of
  re-running the rewrite from the root.
- **Epochs.** See §3.9.
- **Tags and merge commits** are covered the same way: any public commit the
  run published is in the index.

### 3.3 Translation, tree overlay `[P]`

Input: PR head `H` (public), base branch tip, the commit range
`B..H` where `B` is the merge-base of `H` and the public base branch
(from `git merge-base` in the quarantine repo after also fetching the
base branch tip, or the forge's `base_sha`).

1. Look up `B` in the inverse (§3.2) → `C_b`, the private parent. No hit →
   refuse (§3.5, §3.9).
2. For each commit `X` of `B..H` in topological order (linear only,
   §3.11), with public parent `Xp`:
   - `changes = diff-tree(Xp.tree, X.tree)`, as path entries with old and
     new (mode, blob id), recursive. Computed by a tree walk over parsed
     trees (`objects::parse_tree` exists; `[V]` objects.rs has
     `parse_commit`/`parse_tree`/`encode_tree`), never by a checkout.
   - For every changed path `p`, check **in order**, refusing the whole PR
     on the first failure (§3.14):
     1. path validity: `validate::validate_tree_path`-class checks plus the
        stricter rules of §3.12 (`.git` components, backslash, NUL,
        case-folded `.git`, length);
     2. `Filter::decide(p, kind, ...)` must be `Keep` (§3.4). A change at a
        path the filter excludes is refused;
     3. entry kind: regular file only (§3.12 for symlinks, gitlinks);
     4. precondition on the private side: the private tree at `p` (in the
        working tree `T`, which starts as `tree(C_b)` and accumulates) must
        have the same blob id and mode as the public **old** entry `p` in
        `Xp.tree`, or both absent. Blob ids are shared across sides `[V]`,
        so this is exact.
   - Apply `changes` to `T`: replace, add, delete entries, rebuilding only
     the trees along changed paths and **keeping every sibling the public
     side never saw** (excluded directories, excluded files). This is the
     essential property: the private tree is the base and the public
     change is overlaid; the filtered public tree is never converted back.
   - Write the private commit `X'`: tree `T`, parent the previous
     translated commit (`C_b` for the first), **author and committer
     verbatim from `X`** including dates, message verbatim, no `gpgsig`/
     `mergetag` (the signature would not match; `objects.rs`
     `DROPPED_HEADERS` already drops them on the forward side `[V]`).
3. Round-trip gate `[P]`: run the forward `Rewriter` (seeded with the
   map for `C_b`'s ancestry) on `X'` in memory and require
   `rewrite(X') == X` byte for byte. If it holds, then after the MR lands
   and the publish runs, the public branch contains the **same public
   commit** the PR already has, the publish is a plain fast-forward, and the
   forge shows the PR as merged by itself. If it fails (an identity, a
   `strip_empty_dirs` effect, an overlay bug), the PR is reported as
   not translatable (§3.14), not shipped with a different public id,
   because a differing id would make the next publish a parallel history
   next to the PR's own commits.

Landing as a private MR:

- Private head branch `[P]`: `inbound/<mirror_id>/pr-<n>` written to
  `refs/heads/` (required by `merge_request::create`, §2 item 7). The
  publisher must exclude the `inbound/` prefix regardless of
  `publish_branches`: add it to the never-published check beside
  `NEVER_PUBLISHED_REF_PREFIXES` (as a heads-prefix rule, since it is under
  `refs/heads/`). Alternative considered: a fork-style holding repo as MR
  source (`source_repo != target`, `fetch_source_ref` into
  `refs/quarry/mr/<n>`, `[V]` `shadow_ref`). Cleaner isolation, but the
  fork must itself be a registered fork (`forked_from` check in
  `merge_request::create`) and public-origin objects would need a
  persistent second repo. Rejected for phase 1, listed in §9.
- Author attribution `[V]`/`[P]`: `MergeRequestMeta.author` =
  `<host>/<login>` as `mr_sync::imported_author` does `[V]`; local
  usernames cannot contain `/` (`auth::access::validate_username`), so the
  MR author is never a local user and `missing_required_reviewers` /
  self-approval rules treat it as an outsider. Commits inside carry the
  public identity (§3.6).
- Marker: new optional `MergeRequestMeta.origin` (`#[serde(default)]`,
  same compatibility rule as every field there) carrying
  `{kind: "publish-inbound", mirror_id, pr, public_url, head_sha}` `[P]`.
  Cheaper fallback with no wire change: label `from-public` plus a first
  line in the description. The label alone can be edited off by anyone with
  label rights (`edit_metadata`), so it is not a trust signal; the typed
  field is, and merge policy (below) reads it.
- Title and body: from the PR, passed through the same neutralisation the
  external-MR import already does (nothing extra exists to cite; markdown
  rendering and `@mention` notification of private users by a public author
  must be checked and, if needed, defanged `[P]`; see
  `webui_markdown_mentions` tests).
- Review requirement `[P]`: an MR with `origin.kind == publish-inbound`
  cannot be merged without at least one approval from a user with write
  access who is **not** the MR author, and it can never be fast-forwarded
  or merged by an automation. Hook point: `domain::merge_request::merge`,
  next to `missing_required_reviewers` `[V]`. Approvals must be bound to a
  head sha (§3.8).
- Merge strategy `[P]`: restrict to strategies that keep the translated
  commits (fast-forward or merge commit), so the round trip in step 3
  holds. A squash strategy produces a private commit unrelated to `X`; it
  is allowed but the PR then ends *closed with a comment*, not merged, and
  the next publish emits a new public commit. State this in the MR UI.

### 3.4 A public commit that adds or changes an excluded path

Cases: `deploy/x` when `deploy/**` is excluded; a new top-level `internal/`
in allowlist mode (`ExcludedBy::NotInAllowlist`); `.gitquarry/publish.toml`
(`ExcludedBy::FilterFile`, `[V]` `Filter::decide` excludes it for any
non-directory); a gitlink under `exclude_submodules`.

How can it even happen? The public author sees the public tree only, so
they do not know the rule, or they are probing.

| Option | Verdict |
|---|---|
| **Refuse the whole PR**, name the path and the rule category, never reveal which pattern | Chosen `[P]`. Uniform, cannot be used to probe the filter content beyond "this path is not accepted" |
| Drop the offending hunks and translate the rest | Rejected: the PR no longer matches what the author wrote or tested, and the round trip (`rewrite(X') == X`) cannot hold |
| Relocate the file to an allowed path | Rejected: inventing paths is guessing |
| Accept it into the private tree (private repo is the owner) | Rejected: the next publish would drop it, the public branch would never contain `X`, and the PR could not close as merged |

Report wording must not enumerate the filter: "path `deploy/x` is not
accepted by this repository's publish filter" only. Naming the pattern would
disclose the exclusion list (some patterns, like `fixtures/customers/**`,
are themselves a minor leak; the in-repo file is hidden from the public tree
for exactly this reason `[V]` `ExcludedBy::FilterFile`, "the list of what is
private is not public").

Collision variant `[P]`: the public side adds a path that **already exists
privately but is excluded** (`deploy/x` exists privately; the public tree
had no `deploy/`). Same refusal: step 2.2 fires first. Directory/file
collision (public adds a *file* `docs` where private has an excluded
directory `docs/` only): the overlay would overwrite a private directory.
Refused at the precondition step (private tree at `docs` is a tree, the
public old entry is absent).

### 3.5 Public branch based on a commit that collapsed or is unknown

Two situations that look alike:

1. **Public base `B` was published (inverse hit).** Normal case, including
   a `B` that stands for several collapsed private commits: base is `C_o`
   (§3.2). Fine.
2. **No inverse hit for `B`.** Sub-cases: (a) a public commit the mirror did
   not produce (someone pushed or merged straight on the public forge; the
   mirror would report `Diverged` outbound already); (b) a commit of an
   abandoned epoch without a retained inverse (§3.9); (c) a PR based on
   another unmerged PR's commits.

Resolution `[P]`: refuse and say which commit has no private counterpart.
Alternatives rejected: guess the nearest mapped ancestor and translate the
foreign commits in between too (would import unreviewed public commits as
trusted private history and silently create private commits whose public
form was never published by this mirror); fall back to a squash against the
current private branch tip (the PR author's tree is a *filtered* tree, so a
tree-level three-way merge is possible but cannot detect which private-only
files the author "deleted" by not knowing about them; high risk of
spurious deletions). The maintainer-visible fallback stays the one the
outbound doc already promised: the mirror renders the PR as a patch.

### 3.6 Identities, in reverse

Facts `[V]`: forward `map_ident` rewrites by exact email
(case-insensitive) from `identities`; unmapped emails pass or block
(`UnmappedPolicy`); dates are preserved verbatim.

Rules `[P]`:

- **No reverse identity mapping by default.** The private commit carries the
  public identity string exactly as on the public commit. Reasons: (1)
  authorship on a public commit is a claim, not a proof; anyone can commit
  as `ana@users.noreply.example.org`. Reverse-mapping it to Ana's internal
  identity would forge her authorship in private history and could satisfy
  `CODEOWNERS`/required-reviewer logic (`missing_required_reviewers`). (2)
  Mapping would put a private email on a commit that is about to be
  republished; the forward map would turn it back, but a bug in either
  direction is a leak. (3) The round trip `rewrite(X') == X` needs the
  public identity to come back byte-identical.
- **Noreply addresses** (`1234+login@users.noreply.github.com`, GitLab
  `...@users.noreply.gitlab.com`, web-flow committer `noreply@github.com`):
  kept as they are. Nothing private is invented and nothing is leaked,
  because they are already public.
- **Publish identity gate must accept inbound identities.** Without this the
  first merged inbound PR blocks every later publish on
  `UnmappedPolicy::Block`. Rule: the gate exempts an identity string on a
  private commit `C'` when `C'` is in the inbound ledger and the string
  equals the identity recorded from the public commit it was translated
  from. The ledger entry stores the exact identity bytes, not just "accepted
  commit", so that a new epoch (which rewrites old translated commits
  differently) still passes. This is the only place inbound changes the
  outbound gate; it is a phase 1 requirement.
- Optional later: an explicit `reverse_identity` list for maintainers
  (public email, private email), applied only when the PR's forge login is
  in a trusted-login list. Not phase 1 (§9).
- Private-looking identity in a public PR (someone commits with an internal
  email by mistake): it is public by then. No action; the secret scan of
  the PR range (§3.12) may flag it informationally.

### 3.7 Merge commits and force-pushed public branches

**Merge commits in the PR range.** Common source: the forge's "update
branch" button merges the public base into the PR branch.

Do not translate them `[P]`. A public merge `M` with parents `[X_prev,
Y]` where `Y` is a public base commit with private origin `C_y`: a private
merge commit `M'` with parents `[X_prev', C_y]` would tell git that all of
`C_y` is merged, while `M'`'s tree (overlay over `X_prev'`) lacks the
private-only changes `C_y` carries. They would be silently reverted at the
next merge. Alternatives: build the tree by a real private three-way merge
(then the public commit's tree and the private tree diverge and the round
trip fails); flatten (squash) the PR (loses commits and the round trip).
Resolution: refuse a PR whose range contains a commit with more than one
parent, with the instruction "rebase onto the base branch". Squash mode
(single synthetic commit) is a phase 3 option and is explicitly a different
promise (no round trip, PR closed instead of merged).

**Force-pushed public PR branch.** The PR's head sha changes; the old head
may not be an ancestor of the new one.

- Every distinct head sha is a **revision**, recorded in the link entity
  with its translated private head (or its refusal).
- If the local MR is open: re-translate; force-move the inbound branch to
  the new translation (allowed only inside the `inbound/` namespace, and
  recorded); post a system comment on the MR "public head changed from
  `<old>` to `<new>`".
- Reviews: `[V]` each review is bound to the head it reviewed (kb #112,
  `review_is_current`), so after a re-translation moves the inbound branch no
  earlier approval counts. The timestamp rule this document first proposed
  is not needed.
- If the local MR is merged: ignore later pushes, post one comment on the
  public PR ("already merged privately as `<sha>`; open a new PR").
- If the PR is closed on the public side: close the MR, leave the branch
  for GC after a retention period.
- The quarantine repo keeps the fetched objects of the old revision until
  the PR is closed (needed to show "what changed since").

### 3.8 Approvals and merge policy

See §3.3 "Review requirement". Summary of what the merge path must check for
`origin.kind == publish-inbound` `[P]`: a non-author approval for the current head (`[V]` already enforced for
every MR by kb #112); no pending `changes_requested`; the round-trip result
stored as passed; the §3.12 content gates passed; the translated head still
equals the head recorded in the ledger (no one pushed to the inbound branch
by hand; that would break the round trip silently).

### 3.9 Epochs

`[V]` The map and the publish repo are per epoch; an old epoch's objects
stay until a GC sweep (outbound §10, "the old ones stay until a GC sweep").
A new epoch rewrites everything and force-pushes with a lease, so every
public sha changes and every open public PR is now based on a sha that is
not on the public branch any more (GitHub shows it as based on an old
commit; contributors have to rebase onto the new history).

Cases `[P]`:

| PR base epoch | Resolution |
|---|---|
| Current epoch | Inverse hit. Normal. |
| Older epoch, old inverse retained | Look the base up in the epoch the PR's base sha belongs to (search the epochs newest to oldest through the retained `inverse/` tables). The private commit `C_o` is epoch-independent (the private history is never rewritten), so translation proceeds against `C_o` exactly as above. The round-trip gate must then run under the **current** filter, and compares to the PR's own commits only if the filter outputs did not change for the touched paths; otherwise the gate result is "translates, but will not merge as the same public commit": allowed, PR ends closed instead of merged, reported. |
| Older epoch, inverse collected | Refuse: "base is from an abandoned epoch and no longer resolvable". |
| Old epoch and the change touches a path the new filter excludes | Refuse per §3.4, under the *current* filter. A path public in the old epoch and private now is exactly the case the epoch bump exists for. |

Consequences for GC `[P]`: inverse tables of past epochs must be retained
as long as any open public PR could be based on them; they are small (about 40
bytes per published commit plus 20 per collapsed one). Retain them past the GC of the pack data and
the `map/`. Record in the run log when an epoch is bumped how many open
inbound PRs it orphaned. The `Fingerprint`/`config_fingerprint` check
`[V]` is unaffected.

### 3.10 Conflicts with private-only changes since the base

Not a translation problem. The translated commits have parent `C_b`; the
MR target is the private branch tip, which may be ahead by any number of
private commits (collapsed ones included).

- A file the public author edited that the private side **moved or
  deleted** since: the translation succeeds against `C_b`, the MR is
  created, and `domain::merge_request::conflicts` reports it `[V]` (the
  function exists; ort-style rename detection is whatever git does). The
  private maintainer resolves it in the usual MR flow, either by rebasing
  the inbound branch (which would break the round trip, see below) or by a
  merge commit.
- Rejected alternatives: auto-detecting renames and rewriting the PR's
  paths (guessing); refusing the PR if the file moved (the maintainer can
  fix it in seconds and losing the contribution is worse); translating onto
  the current private tip instead of `C_b` (the public precondition at
  step 2.4 would fail whenever the private side touched the file, and
  success would depend on private activity the author cannot see).
- Round trip after conflict resolution: a **merge commit** on the private
  side keeps the translated commits as ancestors; the published form of the
  translated commits is unchanged; a merge commit that touches only private
  paths collapses (`rewrite_commit` merge rule `[V]`: "a merge whose mapped
  parents collapse to the same public commit is itself collapsed"). A merge
  that also changed public-visible files while resolving produces a public
  merge commit: valid, deterministic. If the maintainer **rebases** the
  inbound branch, the translated commits are replaced; the PR ends
  closed-with-comment, not merged. The MR UI should say which will happen.
- "Private-only changes made since" that affect the same blob ids:
  nothing special; they are ordinary overlapping edits.

### 3.11 Binary files, renames, modes

- **Binary files**: bytes are moved by blob id, never decoded or
  re-encoded `[V]` (rewrite principle), so a binary change translates
  exactly. Gates `[P]`: size cap per blob and per PR (§3.12), and the
  secret scan reports "not scanned" for anything over `MAX_SCAN_BYTES`
  (8 MiB) or binary `[V]` (`secrets.rs`: "an unscanned file is not a clean
  one"); the MR shows binary entries as "unscanned" for the reviewer. The
  diff view for reviewers is the existing MR diff, so binary changes show
  as binary there.
- **LFS pointers** added by a public PR point to objects that are not in
  the private LFS store: translate (it is just a small text blob), flag
  "pointer without object" for the reviewer, never fetch LFS objects from
  the public side automatically.
- **Renames**: the tree diff is by path (delete + add); no rename
  detection is needed for translation because the precondition is checked
  per path by blob id, and git redetects renames when the MR merges.
  A rename whose source path is excluded is a change at an excluded path
  (refused). A rename whose destination is excluded is the same.
- **Mode changes**: executable-bit changes are allowed and flagged in the
  report (a new executable file from an outsider is relevant to a reviewer).
  Mode `120000` (symlink) and `160000` (gitlink) are handled in §3.12.
- **Case-insensitive collisions** (two names in one directory differing only
  by case or by Unicode normalisation): refuse (`Reason::CaseCollision`);
  they are a known vector against checkouts on macOS and Windows. `[V]`
  `inbound/translate.rs` `collision_key`: a valid UTF-8 name goes through
  `to_uppercase`, `to_lowercase` and NFKC; a name that is not UTF-8 is
  compared by its ASCII lowercase bytes. Limit: this is Rust's own case
  mapping, not full Unicode case folding (that needs `icu_casemap`, a new
  dependency), so locale-specific folds such as the Turkish dotless `i` are
  not seen. NFKC is wider than canonical equivalence and only errs towards
  refusing.

### 3.12 Threat model: untrusted content flowing in

Attacker: any GitHub/GitLab/Codeberg account that can open a PR on the
public repo. Assets: the private repo's content and secrets, the instance
host, private maintainers' machines and CI, and the integrity of the
publish pipeline. The mirror itself must be safe to run on hostile input
with no human in the loop (it runs in the sweep).

| Threat | Mechanism | Resolution `[P]` |
|---|---|---|
| Git hooks | Hooks live in `.git/hooks`, not in tree content; a PR cannot add them. The risk is a *checkout* with a `core.hooksPath` or a template dir pointing into the tree. | The quarantine repo is bare, no checkout ever; every git call is plumbing (`cat-file`, `ls-tree`, `hash-object`, `commit-tree`, `update-ref`, `pack-objects`), with `core.hooksPath=/dev/null`. `[V]` Every git call on untrusted objects goes through `git_remote.rs` `isolate`: environment cleared except `PATH` and the `PASSTHROUGH_ENV` allowlist (`HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`, their lowercase forms, `SSL_CERT_FILE`, `SSL_CERT_DIR`), an empty `HOME`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`; so the server's credentials, `GIT_*` variables and user or system git config never reach it. The translated objects reach the private repo through the same internal pack/manifest path as a normal push, not through a `receive-pack` that would run hooks on an outsider's content. |
| Symlinks (mode 120000) | A link to `../../etc/...` or to an excluded private path becomes a traversal when anyone checks out the repo (maintainers, CI, release tooling), or on the next publish becomes a public pointer into private structure. | Refuse any change that adds or modifies mode `120000` in phase 1. Later, allow only relative targets that stay inside the repo and whose resolved path passes `Filter::decide` (never into an excluded path). |
| Submodules and `.gitmodules` | Gitlink with attacker URL; `.gitmodules` URL used by a later `submodule update` on a maintainer machine. | Refuse gitlinks (mode `160000`) and any change to `.gitmodules`. Outbound already excludes gitlinks by default (`exclude_submodules`, `ExcludedBy::Submodule` `[V]`). |
| Path traversal and `.git` tricks | Entries named `..`, `.git`, `.GIT`, `git~1`, trailing dots/spaces, backslashes, drive letters, NUL, control characters, overlong paths. | `fsckObjects` on fetch catches git's own list; on top, every changed path must pass `validate::validate_tree_path` (control chars, leading `/`, `..` segment, length 4096 `[V]`) plus: no segment equal (case-folded, NFC and HFS-ignorable-stripped) to `.git`; no `\`; no `:` segment (NTFS streams); no segment ending in `.` or space. |
| Huge blobs and decompression bombs | Disk or memory exhaustion in the sweep. | Check size from `cat-file --batch-check` before reading any blob content. `[V]` caps (`inbound/mod.rs`, fixed per instance): `MAX_PR_COMMITS` = 250, `MAX_CHANGED_FILES` = 3000, `MAX_BLOB_BYTES` = 16 MiB, `MAX_PR_BYTES` = 64 MiB (all added blobs together), `MAX_QUARANTINE_BYTES` = 1 GiB (measured during the fetch, which is stopped over it), `FETCH_TIMEOUT` = 120 s, and `MAX_INBOUND_PATH_DEPTH` = 64 (`validate.rs`). They sit at or above what the big forges show in one PR and are not measured on real traffic. Over any cap: refuse (`Reason::TooLarge(Cap)`), do not truncate (truncating a diff is a narrower, misleading one, the same principle `MAX_REPO_FILTER_BYTES` applies `[V]`). |
| Many objects / deep trees | CPU, stack. | `[V]` tree depth is capped by `MAX_INBOUND_PATH_DEPTH` (`Cap::TreeDepth`); a cap on entries per tree is `[P]`. |
| `.gitattributes` | `filter=`/`diff=`/`merge=` name drivers; the drivers are defined in config (not in the tree), so an attribute alone runs nothing, but attributes change what reviewers see (`-diff`, `linguist-generated`, `-text`, `merge=union`, `export-ignore`) and how merges resolve. | Refuse a PR changing any `.gitattributes` or `.gitmodules` at any depth, compared without case (`[V]` `SENSITIVE_NAMES`), in phase 1; allow with flagged review later. `.gitignore`, `.mailmap` and `.git-blame-ignore-revs` are `[P]` candidates and are not refused today. The private repo must never configure filter drivers from repository content. |
| CI triggers | A new `inbound/...` branch push, or a changed workflow file, makes private CI run attacker-controlled code with private secrets (the classic pull-request-target hole). The CI module is parked on `feature/cicd`, but any external CI on the private repo has the same exposure. | `inbound/` branches are excluded from every push-triggered automation by name (stated as a precondition when CI returns); refuse PRs touching CI definition paths in phase 1 (`[V]` in the dry run, `inbound/translate.rs` `SENSITIVE_TOP_LEVEL`, matched on the top-level entry without regard to case: `.gitquarry`, `.github`, `.gitlab-ci.yml`, `.gitea`, `.forgejo`, `.circleci`, `.buildkite`, `.travis.yml`, `Jenkinsfile`, `azure-pipelines.yml`, `bitbucket-pipelines.yml`, plus any top-level name starting with `.woodpecker`); an operator-extendable list stays `[P]`; `events::emit` for the branch write must not fire a push event that triggers anything `[V]` events exist (`events/mod.rs`; what listens must be checked when built). |
| `.gitquarry/` namespace | The in-repo filter file `[V]` is writable by anyone who can push privately; an outsider's change must not reach it. | Refuse any path under `.gitquarry/`. (It is already unpublishable outbound, so a public PR touching it has no public base either.) |
| Secrets in the PR | Not a leak of private data (the content is already public) but a private repo that now stores someone's pasted token, and a later republish. | `[V]` The dry run runs the outbound secret scan over added blobs and commit messages and reports `SecretFinding`s (`Blob`, `Message`, `Unscanned`) naming the rule and the line, never the matched text. Informational: findings never change the verdict. |
| Malicious markdown/@mentions in the PR title and body | Notification spam to private users; script injection if rendering is lax. | Same rendering path as any imported MR body; add mention neutralisation for `origin.kind == publish-inbound` (check `webui_markdown_mentions`). Title through `validate_issue_title` as `create` does `[V]`. |
| Commit message content | Trailers (`Co-authored-by`, `Signed-off-by`) forging attribution; terminal control sequences. | Messages pass through verbatim (consistency with outbound: "a rewritten message ... is worse than a flagged one" `[V]`), but control characters other than newline/tab refuse the PR. |
| Identity forging | See §3.6. | No reverse mapping. |
| Round-trip confusion / replay | Attacker publishes a public commit that translates to a private tree which then re-publishes differently. | The round-trip gate (§3.3). |
| Forge API data | PR titles, logins, branch names are attacker strings. | Validate as `check_path_segment`-style alphabet where they enter URLs or ref names (`[V]` `forge-clients` does for owner/repo); the inbound branch name is built only from `mirror_id` and the numeric PR number. |

### 3.13 Secret or private content leaving with the next publish

A translated PR is private content in the private tree; the next publish
runs the full outbound gates on it (`audit.rs`: filter audit, blob-set
audit, secret scan, identity gate, license gate `[V]`). They are the
backstop; the inbound gates above are the first line. One interaction:
the **blob-set audit** compares published blobs with blobs seen only at
excluded paths. A public author who adds a file whose blob id equals an
excluded private blob (a copy of a private file they somehow obtained) is
detected by that audit and blocks the publish. Inbound runs the same
check at translation time against the epoch's `excluded/` table `[V]`
(`<epoch>/excluded/<xx>`) and refuses early (`Reason::PrivateBlob`), otherwise the failure shows up
later as a blocked publish unrelated to the PR.

### 3.14 What the mirror does with a PR it cannot translate

Report, never guess. `[P]`

- `[V]` The dry run's verdict (`inbound/verdict.rs`) is
  `Verdict::Translatable(Translation)` (private base, `collapsed_onto_base`,
  the translated commits, the `SecretFinding`s) or `Verdict::Refused(Refusal)`
  with the reason, and the public commit and path when the rule is about
  one. A fetch that fails is an error of the command, not a verdict.
  `[P]` The persisted per-PR outcomes: `Translated`, `Refused(reason...)`,
  `Failed(error)` (transient: forge down, fetch failure, retried with the
  cursor discipline of `mr_sync`), `Superseded` (new head), `Closed`.
- `[V]` Reasons are a closed enum, `Reason`, not free text (so the UI can
  group them): `NoPrivateBase`, `AmbiguousBase`, `OldBaseEpoch(epoch)`,
  `FilterChanged` (the filter changed since the public side was published),
  `NoChanges`, `MergeCommit`, `UnsafePath`, `ExcludedPath`, `SensitivePath`,
  `Symlink`, `Gitlink`, `UnsupportedMode`, `CaseCollision`,
  `PreconditionFailed` (public old blob differs from private: signals
  out-of-band public edits), `PrivateBlob` (a blob the publish only saw at
  an excluded path, §3.13), `ControlChars`, `TooLarge(Cap)` with `Cap` one of
  `Commits`, `ChangedFiles`, `BlobBytes`, `PrBytes`, `FetchBytes`,
  `FetchTime`, `TreeDepth`, and `RoundTripMismatch`.
- Nothing is written to the private repo for a refused PR: no branch, no
  MR, no objects (the quarantine repo is outside the bucket's private
  repo). The record lives in the inbound log.
- Public feedback is **off by default** and, when on, is a single generic
  comment on the PR ("This change could not be imported into the
  maintainers' repository: it touches a path that is not accepted"); never
  the private path names, filter patterns, or private shas.
- A maintainer can always fall back to the manual path the outbound doc
  describes: the mirror renders the public commit range as a patch.
- A PR is re-evaluated when its head sha changes, when the mirror's epoch
  or filter changes (`Fingerprint` differs), or on an explicit "retry".

## 4. Security model for the credential that reads the public forge

- `[V]` today: `PublishMirror.token` is a `MirrorToken` (redacted `Debug`,
  `expose()` only at use, sealed at rest by s3db), validated by
  `validate_mirror_token`; the URL has no userinfo and the credential
  reaches `git` through `git_remote::TOKEN_ENV`, not argv; errors go
  through `redact_url` and `stored_error` (`cap_error(&redact_url(...))`);
  the host must pass `check_host_allowed` at save time and at every run.
- `[V]` Inbound adds **read** use of a credential that today pushes. A push
  token is a bigger credential than inbound needs, so `PublishMirror` has a
  separate, optional `inbound_token: MirrorToken` (`publish/mod.rs`,
  `#[serde(default)]`, empty means none, validated like the push token). It
  is documented as read-only scope (GitHub fine-grained: Pull requests read, Contents read;
  GitLab `read_api` + `read_repository`; Codeberg `read:repository`,
  `read:issue`). The push token stands in for it only when the operator says so
  (`--push-token` on the dry run). `[P]` Posting the optional public comment needs write scope on issues/PRs: a
  third, separate decision (§9), off by default.
- Same host allowlist, enforced before every inbound pass, on the forge
  API base as well as the git URL (`api_base_for` resolves a different
  host for GitHub Enterprise-style URLs `[V]` github.rs: the allowlist must
  cover the API host, not only `remote_url`).
- Same redaction rules: no token, no credentialed URL in `last_error`, in
  the inbound log, in MR comments, or in `tracing` fields (named fields, no
  `Debug` of the mirror).
- The forge's responses are untrusted data (§3.12 last row); response size
  and page count capped (the existing client pages with a cap `[V]`
  github.rs comment on "every MR thread past the first page").
- No credential is ever sent to a host named by PR content (a fork's clone
  URL is **not** used: fetch only from the configured `remote_url` through
  its pull ref, so a malicious fork cannot receive the token).
- The inbound pass never has the private repo's secrets in its environment
  other than what the sweep already has; `[V]` the quarantine repo's `git` calls
  run with a minimal env (§3.12, `git_remote.rs` `isolate`).

## 5. UI and run-record surface `[P]`

- **Settings section.** The existing publish mirror settings
  (`gitquarry-webui`, the "publish mirrors" directory module from commit
  `0e037b67`) gain an "Inbound" subsection per mirror: on/off, provider,
  `inbound_token`, allowed base branches (defaults to `publish_branches`),
  caps, comment-back on/off, the sensitive-path list, and the review
  policy. Saving inbound settings does not change what is published, so it
  does **not** touch `config_fingerprint` and never requires an epoch bump
  (`[V]` that fingerprint covers mode, filter, identities only).
- **Inbound list.** A table of public PRs: number, title, author login,
  head sha, state (`Translated`/`Refused`/...), reasons, local MR link.
  Same failure surface as outbound: `access::cap_error`/`last_error`
  `[V]`.
- **Run record.** A second record type beside `PublishRun`, kept in its own
  object under `repos/<uid>/publish-inbound-log/<mirror_id>` for the same
  reason `publish-log` is separate from the entity ("every open edit form
  would lose its CAS race" `[V]` log.rs), capped like `MAX_RUNS`; records
  per PR revision rather than per run, since a poll with nothing to do
  should not produce a record: PR, head sha, base resolution (epoch,
  private base), round-trip result, gate results as `GateRecord`s
  (`Gate` enum gains inbound gates), duration, outcome.
- **MR page.** The MR created shows a banner from the typed origin marker:
  "From public PR #n by `<login>`" with the public link, the translated
  range, the round-trip status, and whether the merge strategy chosen will
  let the PR close as merged. The diff view is the existing one.
- **Preview.** `[V]` Built as `gitquarry publish-inbound-dry-run --repo
  OWNER/NAME --mirror ID --pr N --base BRANCH [--provider NAME]
  [--push-token] [--bucket NAME]` (`inbound/command.rs`): runs everything
  except writing branches or MRs and prints the verdict.
- **Notifications and audit.** Use the existing `events::emit` /
  `RepoEvent::MergeRequest` path for the created MR; log every refusal with
  `tracing` fields `target = "gitquarry::publish_inbound"`, `mirror`, `pr`,
  `reason`.

## 6. Storage additions `[P]`

| Prefix | Content | Retention |
|---|---|---|
| `repos/<uid>/publish/<mirror_id>/<epoch>/inverse/<xx>` `[V]` | public commit to the set of private commits that map to it (magic `GQI1`, §3.2) | Kept across epochs while any open public PR could use it |
| `repos/<uid>/publish-inbound/<mirror_id>/<pr>` | link entity: revisions, translated heads, MR number, state, accepted public identities | With the mirror |
| `repos/<uid>/publish-inbound-log/<mirror_id>` | capped record list | Capped |
| quarantine bare repo | fetched PR objects | Local, per run or per open PR; no bucket copy |

Entities use s3db per the project rule (`Collection::new_owned`, as for
`publish-mirrors`); the inverse is bulk-by-sha and follows the `Remote`
pattern as `map/` does, with its own shard format and a union store `[V]`.

## 7. Phased delivery

| Phase | Delivers | Proves | Done when |
|---|---|---|---|
| 0 `[V]` built | Inverse index written by history runs (+ backfill for existing epochs), `inbound dry-run` for one PR number: fetch into quarantine, resolve base, compute translation in memory, run all gates and the round-trip check, show the verdict. Writes nothing to the private repo. | The inverse is correct, the overlay and the round trip hold on real PRs, the refusal taxonomy is usable. Same role as outbound phase 0. | For a set of fixture PRs (clean, excluded path, collapsed base, merge commit, symlink, force-push, old epoch) the verdicts match the expectations in §3; for every translatable PR `rewrite(X') == X`; a full publish run still produces identical public ids with the inverse writing enabled (the new table changes no existing output); tests live in `tests/` and use the public API only (project rule). |
| 1 | "Import PR #n" button: writes the inbound branch and the MR (typed origin, `host/login` author, review requirement), link entity, linear PRs only, symlink/gitlink/sensitive-path refusals, caps, publish identity-gate exemption for ledgered commits. Manual trigger only. | Landing works end to end: an imported PR can be reviewed, merged by fast-forward or merge commit, published by the next run, and the public PR shows as merged by itself. | A fixture public PR round-trips through import, merge, publish with the public branch containing the PR's own commit ids and no `Diverged`; an inbound branch is never published even under `publish_branches=["**"]`; a refused PR leaves no branch, MR or object in the private repo; an inbound commit with an unmapped identity does not block the next publish; self-approval and non-write approvals do not satisfy the review requirement. |
| 2 | Polling discovery with cursor, revisions and force-push handling, review invalidation on new revision, state sync (closed, merged), generic public comment (opt-in), `inbound_token`, UI list and run record. | The pipeline survives hostile and churning input without an operator. | A force-pushed PR produces a new revision, an old approval no longer counts, a closed PR closes its MR, a poll with nothing new writes nothing, and a forge outage retries without skipping a PR (the `oldest_failed` rule). |
| 3 | Old-epoch bases via retained inverse tables, optional reverse identity list for trusted logins, symlink and `.gitattributes` allowances, squash mode, webhooks, rename hints in the report. | Each relaxation is opt-in and does not weaken the defaults. | Per relaxation, a test shows the default refusal still fires when the option is off. |

Phase 0 is built (kb #118, #119, #122, #123, #125; tests in
`crates/gitquarry-mirror-external/tests/publish_inbound.rs`). Phases 1-3 are
`[P]`. Phase 0 was deliberately the smallest: it is read-only on the private side,
exercises the hardest part (inverse + overlay + round trip) and decides
whether the rest is feasible.

## 8. Things in the existing code that make this harder than the old §7 said

1. The mapping stores private to public with a **hard error on a differing
   value** (`Table::store`); an inverse cannot reuse it unchanged because
   several private commits can collapse onto one public commit and two
   private commits can in principle produce the same public commit. Solved:
   `publish/inverse/` stores sets (§3.2).
2. `mr_sync` cannot import fork PRs by design (branch names only, branches
   must be mirrored), and `PublishMirror` has no provider field. Fork PRs
   are the main case.
3. `MergeRequestMeta` has no origin field, so "this MR came from the public
   side" needs a wire change (federation shares those types:
   `gitquarry-wire`). The review half is solved: `MrReview.head_sha`, kb #112.
4. `merge_request::create` requires the source to be a `refs/heads/` branch
   of the target or of a registered fork, so the translation has to live
   in a private branch that the publisher must be taught never to publish.
5. Outbound `Diverged` makes the forge's own merge button on the public PR
   harmful; the whole design relies on the round trip landing the same
   public commit, which in turn constrains the identity and committer
   handling (§3.6) and the merge strategy (§3.3).
6. The publish identity gate (`UnmappedPolicy::Block` default) breaks on the
   first merged outside contribution; the gate needs an inbound exemption.
7. Epoch bumps invalidate the public shas an open PR is based on; the
   current GC rule ("old ones stay until a GC sweep") does not promise the
   inverse survives.
8. The `ForgeApi` trait lacks head/base shas and a fork flag, and fetching
   a PR's objects needs forge-specific pull refs, so a fetch path other
   than `fetch_ref_from_remote_url(bare_repo, url, token, ref_name)` with a
   branch name is needed (the function itself takes any ref name `[V]`, so
   the git half is small; the trait half is not).

## 9. Decisions of the owner

The owner answered all 15 questions on 2026-11-06, each as recommended;
recorded as decision kb #117. The table keeps the question and the options
as they were put, and states what was decided. Rows marked `[V]` are built
in phase 0; the others bind phases 1-3.

| # | Question | Options | Decision (kb #117) |
|---|---|---|---|
| Q1 | How is a translated PR represented as a private MR source? | (a) `refs/heads/inbound/<mirror>/pr-<n>` in the private repo; (b) a holding fork repo used as `source_repo` | (a); the publisher excludes the prefix. |
| Q2 | Commit by commit with a round trip, or one squashed commit? | (a) per commit, linear only; (b) squash only; (c) both | (a), linear only `[V]` (dry run refuses `MergeCommit`). Squash stays a later opt-in. |
| Q3 | Identity in reverse | (a) never map, keep public identity; (b) map known maintainers via a list; (c) map by the identity map automatically | (a) `[V]` (the dry run keeps the public identity verbatim). (b) is phase 3 for a trusted-login list only. |
| Q4 | Publish identity gate and inbound commits | (a) exempt ledgered identity strings; (b) require every contributor in the identity map; (c) switch the mirror to `passthrough` | (a). |
| Q5 | Which credential reads the public forge? | (a) the push token; (b) a separate read-only `inbound_token`; (c) anonymous | (b) `[V]` (field built); (a) only as an explicit opt-in `[V]` (`--push-token`). |
| Q6 | Comment back on the public PR? | off / generic comment / detailed | Off by default; generic text only when on. |
| Q7 | How strict is the phase 1 path policy? | refuse the sensitive set, or flag and allow with review | Refuse `.gitattributes`, `.gitmodules`, CI files, symlinks, gitlinks and `.gitquarry/**` `[V]`; the built CI list is wider than the one in the question (§3.12). Relax per item in phase 3 with a test each. |
| Q8 | Add `head_sha` to `MrReview`? | (a) yes, for all MRs; (b) ignore older reviews for inbound MRs only | Dropped as a question: covered by kb #112, which binds each review to the head it reviewed, for every MR (`review_is_current` in `gitquarry-core` `domain/merge_request/mod.rs`, `MrReview.head_sha` in `gitquarry-wire`). `[V]` |
| Q9 | Typed `origin` on `MergeRequestMeta` or a label | typed field / label only | Typed field with `#[serde(default)]`. |
| Q10 | Retention of inverse tables across epochs | forever / N epochs / while open PRs exist | Keep while any open PR could reference the epoch and for at least 3 epochs. |
| Q11 | A public PR with merge commits | refuse / squash / true private merge | Refuse `[V]`. |
| Q12 | The forge's merge button on the public PR | document / detect and handle | Detect (`Diverged` already) and document; the supported flow is private merge, then publish. |
| Q13 | Poll or webhook | poll / webhook / both | Poll first; webhook in phase 3. |
| Q14 | What do private maintainers see about the author's untrusted status? | banner only / banner plus a merge-policy gate | Banner plus the merge gate of §3.8. |
| Q15 | Caps | fixed constants / mirror settings | Constants, lowerable per mirror, never above the instance cap; numbers come from measurement. The constants are built `[V]` (§3.12); per-mirror lowering is not. |

## 10. Known gaps

- **Real-forge fetch is untested** (kb #121). The dry run's tests fetch from
  local bare repos that carry the pull ref. No official Forgejo or Gitea
  documentation was found that confirms `refs/pull/<n>/head`, which the
  Codeberg provider relies on; GitHub's and GitLab's refs are the documented
  ones. `[V]` only that the code asks for those refs (`inbound/mod.rs`
  `pull_ref`).
- **A GPG-signed public commit always fails the round trip.** The
  translation drops `gpgsig` and `mergetag` (`objects.rs` `DROPPED_HEADERS`),
  so `rewrite(X') == X` cannot hold for a signed `X` and the verdict is
  `RoundTripMismatch`. Accepting signed commits would need a different
  promise for them.
- **Collision check is Rust case mapping plus NFKC**, not full Unicode case
  folding (§3.11).
- Phase 1-3 items (landing, discovery, epochs older than the current one,
  the identity-gate exemption) are not built; the dry run refuses an
  `OldBaseEpoch` base outright.
