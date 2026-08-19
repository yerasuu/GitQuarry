# Artifacts (`gitquarry-artifacts` binary)

## Context

`docs/package-registry-concept.md` designed a package registry (npm/raw,
future ecosystems) on the explicit reasoning that it "has no genuine
separately-deployed-process rationale... unlike federation," and so should
live inside `gitquarry-core`. That placement decision is **superseded** by
this doc: package registry and CI build-artifact storage
(`docs/ci-cd-concept.md` Phase 4, which currently puts job artifacts in
`gitquarry-ci`'s own bucket namespace) are unified into one new binary,
`gitquarry-artifacts`, same optional/separately-deployed posture as
`gitquarry-federation`/`gitquarry-ci`.

Two things changed the calculus from the original doc:

1. **Deploy-minimal is now an explicit project goal** (driving the Tracker
   split too, see `docs/tracker-extension-concept.md`) — an operator who
   doesn't want a package registry shouldn't pay for its code or its S3
   traffic pattern in the core binary.
2. **The traffic shape genuinely matches federation/CI's rationale**, unlike
   Tracker's: package/artifact blobs are large, bursty, and third-party
   (uploaded packages, CI build outputs) — exactly the kind of traffic
   federation and CI already justify a separate process for. Unlike Tracker,
   there is no synchronous, frequent, small-payload interaction with core's
   git/PR machinery that a process boundary would tax.

Confirmed before drafting: **zero code exists for either package registry or
CI/CD today** — no `crates/gitquarry-ci`, no `registry`/`PackageMeta`
anywhere in the workspace. `gitquarry-artifacts` is a genuinely fresh binary
with no naming collisions, and `gitquarry-federation` is the direct,
already-proven template for its shape (`main.rs`, `config.rs`, `state.rs`,
`core_client.rs`, `web/mod.rs`, plus its `Cargo.toml` dependency set).

## Design decisions (settled, not open)

| Question | Decision | Why |
|---|---|---|
| Binary | New `gitquarry-artifacts`, mirrors `gitquarry-federation`'s file set field-for-field. | Proven template already in the codebase; no need to invent a new binary shape. |
| Config | `gitquarry-artifacts.toml`, same env>cwd>gitroot>userglobal resolution as `gitquarry-federation.toml`/`gitquarry-ci.toml`, never merged with any other config file. | Matches existing precedent exactly. |
| Storage | Own storage backend + cipher bootstrap against the shared bucket, own prefix (`artifacts/...`) — not proxied through core one request at a time. | Package/CI-artifact blob traffic is the large-blob case federation/CI precedent already identifies as needing direct S3 access. |
| IPC: core↔artifacts | Loopback, shared `internal_token`, mirrors core↔federation/core↔ci exactly. New `AppContext.artifacts: Option<ArtifactsLink>` (mirrors `federation: Option<FederationLink>`), new `ArtifactsSection` in `config/mod.rs`, new `/_internal/*` endpoints in core's `internal_api.rs` for permission lookups (`effective_namespace_permission_for_user`) — artifacts never re-implements core's permission model. | Same "one shared loopback pattern, N optional binaries" the project already has for federation/CI. |
| Public surface | Core's public port proxies external package-manager/browsing traffic (`/_packages/*`) to artifacts' loopback listener via a new `webui/artifacts_proxy.rs`, mirroring `federation_proxy.rs`'s `send`/`is_hop_by_hop`/`*_unavailable` (→ 503) pattern exactly. Artifacts never opens a second public port. | Project-wide "one public port" rule, no exception. |
| IPC: ci↔artifacts | **New** direct loopback leg between the two optional binaries, same shared `internal_token`. A CI job's declared build artifacts upload straight to `gitquarry-artifacts` via a new `artifacts_client.rs` in `gitquarry-ci` (mirrors its own `core_client.rs`). | Avoids two separate content-addressed blob stores for conceptually the same thing (a downloadable build product); CI logs (not artifacts) stay in `gitquarry-ci`'s own namespace regardless — different retention/access model, not a product a user downloads independently. |
| Fallback when artifacts absent | `gitquarry-ci` job artifacts fall back to its own `ci/<repo-uid>/runs/<run-id>/...` namespace exactly as `ci-cd-concept.md` Phase 4 originally specified, when the artifacts link isn't configured. Package-registry HTTP routes (`/_packages/*`) 503 the same way any federation-shaped route does when unconfigured. | Matches the existing "absent optional binary = graceful fallback, never a lost capability for the binaries present" rule. |
| Scope carried over unchanged | Everything in `package-registry-concept.md`'s Phase 1-9 (storage model, npm/raw/future-ecosystem adapters, quota, permissions, PAT scopes) stays exactly as designed — only the crate/binary boundary and the two IPC legs above are new. | No need to redesign a working phase plan just because the process boundary moved. |

## Implementation

Package-registry-concept.md's phases 1-9 move into the new binary's scope,
with these adjustments:

- **Phase 1** (storage/domain foundation) lands in `gitquarry-artifacts`
  instead of `gitquarry-core`. `storage/package.rs`'s content-addressed blob
  primitives and `domain/package.rs`'s `publish_version`/`fetch_version`
  become this binary's own modules, bootstrapped against its own `Remote`
  instance the same way `gitquarry-federation`/`gitquarry-ci` each bootstrap
  their own.
- **Phase 2/3** (raw + npm HTTP routes) are served by artifacts' own
  loopback listener; core's `/_packages/*` becomes a proxy layer
  (`webui/artifacts_proxy.rs`), not the handler itself.
- **New phase — ci-artifact handoff**: `gitquarry-ci`'s Phase 4 (job
  execution engine, artifact upload) gets a client call to artifacts'
  loopback (`gitquarry-ci/src/artifacts_client.rs`, new), falling back to
  its own namespace when unconfigured, per the design decision above.
- **Auth**: permission checks still need core's user/permission data —
  artifacts calls core's `/_internal/*` for permission lookups the same way
  `gitquarry-ci` already does for repo metadata, rather than duplicating the
  permission model in a second binary.
- **Phases 5 (Docker-OCI), 6 (Maven/PyPI/Helm/NuGet), 7 (private packages),
  8 (PAT UI), 9 (federation read-only browsing)** from the original doc are
  unaffected in substance — they're paper-designed-only phases that simply
  now target `gitquarry-artifacts` instead of `gitquarry-core`.

## Critical files

- `crates/gitquarry-artifacts/src/{main,config,state,core_client,web,storage,domain}.rs` (new crate, mirrors `gitquarry-federation`'s file set)
- `crates/gitquarry-core/src/config/mod.rs` (`ArtifactsSection`)
- `crates/gitquarry-core/src/app.rs` (`AppContext.artifacts: Option<ArtifactsLink>`)
- `crates/gitquarry-core/src/internal_api.rs` (new endpoints: namespace permission lookup)
- `crates/gitquarry-webui/src/webui/artifacts_proxy.rs` (new, mirrors `federation_proxy.rs`)
- `crates/gitquarry-ci/src/artifacts_client.rs` (new leg, once `gitquarry-ci` is scaffolded per `ci-cd-concept.md`)
- `crates/gitquarry-wire/` (any new shared DTO genuinely crossing 2+ boundaries, e.g. an artifact manifest type)

## Verification

- `cargo test` at each phase boundary — Phase 1 domain-layer unit tests
  against artifacts' own storage, same style as `gitquarry-federation`'s own
  domain tests.
- Integration test: publish → re-publish identical bytes doesn't re-upload
  (dedup) → download → quota enforcement → permission denial for a
  non-member (unchanged from the original package-registry doc's Phase 2
  verification, just targeting the new binary).
- New: ci→artifacts handoff integration test, and the fallback-to-own-namespace
  path when artifacts is unconfigured.
- Manual: `npm publish`/`npm install` against a local gitquarry instance with
  `gitquarry-artifacts` running, confirming a real npm client round-trips
  correctly through the core proxy.
- `cargo clippy --all` / `cargo fmt --check`, `unsafe_code = forbid` — no
  exception for this crate.
- Commit per phase, one logical change per commit.
