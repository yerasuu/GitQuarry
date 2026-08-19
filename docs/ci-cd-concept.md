# CI/CD (`gitquarry-ci`)

## Context

Roadmap gap #1, "máximo impacto" (`docs/feature-compare.md`; `docs/forge-ui-feature-comparison.md:56`). Two lessons pulled from that comparison and folded into the design below, not left as loose commentary:

1. **One execution engine, never two.** Codeberg runs Forgejo Actions and Woodpecker in parallel and documents the confusion as a recurring FAQ (`docs/forge-ui-feature-comparison.md:54`). This design keeps a single executor regardless of which language a pipeline is written in — see the graph-model decision below.
2. **Required status checks and CODEOWNERS ship with CI, not after** (`docs/forge-ui-feature-comparison.md:120`) — the existing PR merge machinery (reviews, merge strategies) already has the shape to gate on this; it's just missing the signal. Phase 5 below wires that in explicitly instead of leaving it a future gap.

Four architecture decisions were settled before drafting (asked directly, not assumed):

| Question | Decision |
|---|---|
| Where does it live | New binary `gitquarry-ci`, same optional/separately-deployed posture as `gitquarry-federation` — core stays fully usable without it. |
| Python pipelines | Run as a `python3` subprocess, never embedded (PyO3 would need `unsafe`, which the workspace forbids). |
| Job isolation | Containers (Docker/Podman), shelled out via subprocess — same idiom `mirror::spawn_git_subcommand` already uses for `git`, not a new Docker-API client dependency. |
| Trigger plumbing | Depends on the webhook/event work (`feature-compare.md`'s own #1/#2 pairing of CI+webhooks) landing first; the two share one internal event-emission point rather than each inventing their own. |

## Design decisions (settled, not open)

| Question | Decision | Why |
|---|---|---|
| Config | `gitquarry-ci.toml`, same env>cwd>gitroot>userglobal resolution as `gitquarry-federation.toml`, never merged with `gitquarry.toml`. | Matches the existing two-binary precedent exactly (`config/mod.rs`'s `FederationSection` vs. `gitquarry-federation/src/config.rs`). |
| Storage | `gitquarry-ci` bootstraps its **own** storage backend + cipher against the same bucket, own prefix (`ci/<repo-uid>/...`) — same pattern federation's `main.rs` uses (`get_or_create_cipher`), not core's storage borrowed through the internal API. | Job logs/artifacts are the kind of large-blob traffic LFS/releases already prove needs direct S3 access, not proxied through core's `/_internal/*` one request at a time. |
| IPC | Bidirectional loopback, same shared `internal_token` secret already used between core and federation, but the two existing legs run in opposite directions and CI needs both: `gitquarry-ci` calls into core's `/_internal/*` (new endpoints: repo clone URL, commit metadata, status write-back) exactly like `federation::core_client::CoreClient`; core calls out to `gitquarry-ci`'s own loopback to push trigger events, exactly like core's existing `AppContext.federation: Option<FederationLink>` → `federation_url` leg. | Both directions already exist in the codebase individually (federation→core via `/_internal`, core→federation via `federation_url` proxying); CI is a new but consistent third user of the same mechanism, not a new IPC shape. |
| Public surface | `gitquarry-ci` never opens a second public port. Anything a browser or external client needs (log download, artifact download) is reached by hitting core's public port, which proxies to `gitquarry-ci`'s loopback listener — identical to how federation's public-facing routes work today. | Project-wide IPC rule (`CLAUDE.md`), no exception carved out for CI. |
| Pipeline definition | `.gitquarry-ci/pipeline.rs` (a small `Cargo.toml` + `main.rs`) **or** `.gitquarry-ci/pipeline.py`, mutually exclusive — both present is a hard error at trigger time, not a silent pick. | Matches "supports Rust or Python," not "supports Rust and Python simultaneously," which would need a merge-precedence rule nobody asked for. |
| Graph model | Neither language's pipeline code executes any step itself. It only *builds and dumps a declarative job graph* (JSON: jobs, `image`, `commands`, `needs`, `env`, declared artifacts) to stdout when invoked with `--dump-graph`. `gitquarry-ci` is the **sole** executor of that graph, regardless of which language produced it. | This is the actual mechanism behind "use Rust or Python" without building two execution engines — both are just graph builders against one shared wire schema, directly avoiding the Actions/Woodpecker duplication Codeberg is cited as a cautionary example of. |
| Graph resolution sandboxing | The resolution step itself (running `pipeline.rs`/`pipeline.py` to get the graph) runs inside its own throwaway container, not on the `gitquarry-ci` host — it's still untrusted repo-controlled code. Rust: a pinned build image, repo mounted, `cargo run -q --bin pipeline -- --dump-graph`. Python: a pinned `python:3-slim`-family image, `python3 pipeline.py --dump-graph`, matching the "subprocess python3" decision. | A malicious or buggy pipeline file must not get arbitrary code execution on the orchestrator itself — same threat model as the job steps it later schedules. |
| Job execution | v1: single-node, `gitquarry-ci` runs `docker run`/`podman run` directly via subprocess, bounded worker pool (`tokio::sync::Semaphore`). Job-queue kept behind a small trait so a distributed poll-based remote-runner agent is a later phase, not a redesign (paper-designed in Phase 8). | Consistent with the project's existing "shell out, don't add a client-library dependency" idiom for `git`; avoids speculatively building distributed scheduling before anyone needs it. |
| Secrets | New namespace-scoped `Secret` entity, same mandatory XChaCha20Poly1305 encryption as everything else in the bucket. Injected as an env var into the job container only, never written to the resolution step. Redacted from logs by substring-scanning raw secret values before a log chunk is uploaded. **Never available at all** to a pipeline run triggered by a PR from a contributor without write access, unless a maintainer explicitly approves that run first. | The fork-PR-secrets-leak class of incident is well documented in every CI product that got this wrong first (GitHub Actions included); an approval gate is the standard fix and belongs in the design, not bolted on after a report. |
| Status write-back | New `POST /_internal/pipeline-status` on core's existing `internal_api.rs`, storing a minimal `CommitStatus` keyed by repo + sha. PR merge gating reads it as a new required-check condition alongside the existing review/merge-strategy checks. | Directly closes the "status checks depend 100% on having CI" gap the comparison doc calls out, using the merge machinery that already exists instead of a parallel gating mechanism. |
| Trigger plumbing | New event-emission point after `ssh.rs:337`'s existing `if git_subcmd == "receive-pack" && status success` block (and the http.rs equivalent) — today that block only re-syncs the mirror; it becomes the one place a "ref updated" event fires, consumed by both webhooks (external HTTP delivery, separate roadmap item) and `gitquarry-ci` (internal trigger call). | One event source, two consumers — avoids webhooks and CI each growing their own copy of "what counts as a push" logic. |
| Logs/artifacts | **Superseded by `docs/artifacts-concept.md`**: logs still use the "buffer to tempfile, hash, then commit" discipline and stay under `ci/<repo-uid>/runs/<run-id>/...` in `gitquarry-ci`'s own bucket namespace (different retention/access model, not a downloadable product). Job-declared build **artifacts** instead upload to `gitquarry-artifacts` via a new direct loopback leg (`gitquarry-ci/src/artifacts_client.rs`, same shared `internal_token`, mirrors `core_client.rs`), falling back to this same `ci/<repo-uid>/runs/<run-id>/...` namespace when artifacts isn't configured. | Avoids two separate content-addressed blob stores (package registry's and CI's) for conceptually the same thing — a downloadable build product. |

## Implementation

### Phase 0 — prerequisite: internal push-event emission (shared with webhooks)

Not detailed here — belongs to the webhooks roadmap item — but scoped precisely: one event fires from the existing post-`receive-pack`-success block in `git/ssh.rs` and `git/http.rs`, carrying repo uid + updated refs + pusher. Webhooks and `gitquarry-ci` are two independent subscribers to it; neither is built until this lands.

### Phase 1 — `gitquarry-ci` skeleton

- New crate/binary, `main.rs`/`config.rs`/`state.rs` mirroring `gitquarry-federation`'s shape exactly: resolve `gitquarry-ci.toml`, bootstrap own backend+cipher against the shared bucket, start a loopback server guarded by `internal_token`.
- `core_client.rs`: outbound calls to core's `/_internal/*` (new endpoints added in this phase: fetch clone URL + latest commit sha for a repo, by uid).
- Core side: `config::CiSection` (mirrors `FederationSection`), `AppContext.ci: Option<CiLink>`, wired the same way `federation` already is in `app.rs`.

### Phase 2 — wire schema (`gitquarry-wire`)

- `PipelineTrigger { repo_uid, sha, refname, pusher }`, `JobGraph { jobs: Vec<JobSpec> }`, `JobSpec { id, image, commands, needs, env, artifacts }`, `PipelineStatus { repo_uid, sha, state, jobs: Vec<JobStatus> }` — kept minimal, same "only types genuinely crossing 2+ boundaries" rule the crate already follows.

### Phase 3 — pipeline discovery + graph resolution

- Detect `.gitquarry-ci/pipeline.{rs,py}` in the pushed tree at the triggering sha; error (surfaced as a failed `CommitStatus`, not a crash) if both exist.
- Containerized resolution per language as described in the design table; parse the dumped `JobGraph`; validate it (cycle detection, job-count/resource bounds, `image`/`commands` pass through `validate.rs`-style checks before ever reaching a `docker run` invocation — same "sanitize before touching any of it" rule as every other network-facing input in this codebase).

### Phase 4 — job execution engine

- Bounded worker pool executing the validated DAG in dependency order; each job is a `docker run`/`podman run` subprocess with the job's `env` (secrets injected only per the approval-gate rule above).
- Log streaming: tempfile-buffered, redacted, then uploaded as a blob under the run's `ci/` prefix; artifacts declared by a job follow the identical path.
- On completion, `gitquarry-ci` calls core's new `POST /_internal/pipeline-status`.

### Phase 5 — required status checks + CODEOWNERS

- Extend the existing PR merge-check path with a new condition: latest `CommitStatus` for the PR's head sha must be `success` (or the check is unconfigured for that repo — opt-in per `RepoConfig`, not forced on every existing repo).
- `CODEOWNERS`-style required-reviewer-by-path, bundled here per the comparison doc's explicit "build together, not after" call — separate design detail, sized similarly to the existing branch-protection rules in `domain/repo.rs`.

### Phase 6 — web UI

- New `gitquarry-webui` module (mirrors `webui/federation_proxy.rs`'s pattern of proxying to a loopback process): pipeline run list per repo, run detail with per-job logs, manual re-run button, the fork-PR approval-gate action for maintainers.

### Phase 7 — secrets management UI

- `Secret` CRUD under repo/namespace settings, write-only after creation (value never re-displayed, matches how the project already treats `passphrase`-class values).

### Phase 8 (paper design) — distributed remote runners

- Self-hosted runner agent binary polling `gitquarry-ci`'s loopback for queued jobs (pull-based, avoids inbound-to-runner network requirements) — same shape as GitHub's self-hosted runners. Not built until single-node execution is proven insufficient; the Phase-4 job-queue trait exists specifically so this doesn't require re-architecting the execution engine.

### Phase 9 (paper design) — SDK ergonomics

- `gitquarry-ci-sdk` Rust crate and a `gitquarry_ci` Python package, both thin builders over the `JobGraph` shape from Phase 2 (`job("test").image("rust:1.80").run("cargo test")` style), so repo authors aren't hand-writing JSON. Deferred past the core engine — the wire schema is the actual contract; ergonomic builders on top of it are additive.

### Explicitly out of scope after all 9 phases

- Multi-project/cross-repo pipeline graphs (GitLab's upstream/downstream trigger graphs) — no current use case, would need its own design pass.
- Self-hosted runner autoscaling/pooling beyond "poll for one job at a time" — first version of Phase 8 only.
- Any language beyond Rust/Python for pipeline definition — the graph-dump contract makes adding a third language a self-contained future adapter, not a redesign, but none is scoped now.

## Critical files (new/touched)

- `crates/gitquarry-ci/src/{main,config,state,core_client,web,executor,graph}.rs` (new crate)
- `crates/gitquarry-wire/src/{pipeline.rs}` (new) — `PipelineTrigger`/`JobGraph`/`JobSpec`/`PipelineStatus`
- `crates/gitquarry-core/src/internal_api.rs` (new `/_internal/pipeline-status`, repo/commit lookup endpoints)
- `crates/gitquarry-core/src/config/mod.rs` (new `CiSection`)
- `crates/gitquarry-core/src/app.rs` (new `AppContext.ci: Option<CiLink>`)
- `crates/gitquarry-core/src/git/{ssh,http}.rs` (Phase 0 event-emission point, shared with webhooks)
- `crates/gitquarry-core/src/domain/repo.rs` (required-status-check merge condition, CODEOWNERS)
- `crates/gitquarry-webui/src/webui/ci/{mod,detail}.rs` (new, Phase 6)
- `crates/gitquarry-webui/src/auth/web/repo_settings/` (Secret CRUD, Phase 7)

## Verification

- `cargo test` at each phase boundary — Phase 3's graph validation and Phase 4's redaction logic get direct unit tests (no container needed for either).
- Integration test with a real local Docker/Podman daemon for Phase 4 (skipped/gated in environments without one, same as any test needing an external service).
- Manual: push a repo containing both a `pipeline.rs` and a `pipeline.py` version of the same trivial graph, confirm identical `JobGraph` shape and identical execution result.
- `cargo clippy --all` / `cargo fmt --check`, `unsafe_code = forbid` — no exception for this crate.
- Commit per phase, one logical change per commit.
