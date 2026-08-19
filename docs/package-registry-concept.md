# Package registry (generic multi-format artifact storage)

> **Superseded placement decision:** point 1 below ("lives in
> `gitquarry-core`, not a separate binary") is reversed by
> `docs/artifacts-concept.md` — deploy-minimal is now an explicit goal, and
> package/CI-artifact blob traffic turned out to match federation/CI's
> separate-process rationale after all. Everything else in this doc (the
> storage model, npm/raw adapters, quota, permissions, PAT scopes, Phases
> 1-9) still applies verbatim — only the crate/binary it lands in changed.
> See `artifacts-concept.md` for the current design.

## Context

Roadmap gap (`docs/feature-compare.md`, `docs/forge-ui-feature-comparison.md:76-82`): gitquarry has no package/artifact registry (npm/Maven/Docker-OCI/Helm/NuGet/PyPI-style), a feature all three comparison forges (GitHub, GitLab, Codeberg/Forgejo) ship free and standard. Two decisions were made before drafting this design:

1. **Lives in `gitquarry-core`, not a separate binary/extension** — unlike federation, it has no genuine "separately-deployed process / peer-trust boundary" rationale. It's the same storage/access-control domain LFS and releases already live in.
2. **v1 ecosystems: npm + a generic "raw" format.** Docker-OCI is deferred — its `/v2/` root-path convention can't be namespaced under a `/_packages/` prefix and would collide with the web UI's `/{*repo}` catch-all route; it needs its own route-collision analysis as separate future work, not blocking v1.

Docs (`docs/forge-ui-feature-comparison.md:82`) explicitly call for a **generic multi-format design from the start**, copying the content-addressed approach LFS already proves — not npm bolted on ad hoc. This design follows that: a format-agnostic storage/domain layer, with npm and raw as two thin protocol adapters on top.

## Design decisions (settled, not open)

| Question | Decision | Why |
|---|---|---|
| Config gating | No `Option<...>` toggle — always-on, like LFS/releases | Every write is already permission-gated; unlike federation there's no separate process to be "absent." |
| Namespace reservation | Reuse the existing owning user/group's namespace, no new reservation | A package belongs to a user/group that already exists via `NAMESPACE_INDEX`; no second name pool needed. |
| Quota | Fold into existing total: add `package_bytes: u64` next to `pack_bytes`/`lfs_bytes` on `User`/`Group`, summed in `check_quota` | Avoids a speculative separate quota dimension. |
| Permission model | New **namespace-scoped** permission (`effective_namespace_permission_for_user`), not repo-scoped | Packages belong to a user/group, not any one repo — the existing repo-centric grant model has no slot for "whole namespace." Reuses the `Permission` enum as-is. |
| PAT scopes | Reuse `Pat.scopes: BTreeMap<Uid, Permission>` keyed by the namespace owner's own `Uid` | `Uid` is already kind-agnostic across repos/users/groups; zero schema change. UI to grant such a scope is a fast-follow, not blocking. |
| v1 read visibility | Always public, no private packages yet | Keeps v1 tractable; flagged as the one deliberately deferred piece (Phase 4). |
| URL scheme | `/_packages/npm/{**pkg}` and `/_packages/raw/{namespace}/{name}/{version}/{filename}`, dispatched via a middleware classifier (not plain `.route()`) | Matches the `_`-prefixed reserved-path convention already used (`/_settings`, `/_releases`); avoids `/{*repo}` collision; npm/raw clients both take a configurable base URL so nesting is protocol-compliant (unlike Docker). |

## Implementation

### Phase 1 — storage + domain foundation (no HTTP)

- **`storage/remote.rs`**: widen `key`/`upload_file`/`download_file` from private to `pub(crate)` (no logic change); add generic primitives `get_json_at`/`put_json_at`/`list_keys_at`/`delete_keys_at`/`blob_exists_at` — reusable building blocks, this file never learns the word "package."
- **`app.rs`**: add `remote_for_namespace(owner: &OwnerRef) -> Remote`, prefix `packages/<user|group>/<uid>`, sibling of `avatar_remote()`/`remote_for_uid()`.
- **`storage/package.rs`** (new): package-specific key scheme built only on the Phase-1 primitives, mirroring `storage/lfs_locks.rs`'s split from `remote.rs`.
  - Content-addressed blobs, shared across every package/version in the namespace: `blobs/<sha256>`.
  - `PackageMeta` at `pkg/<format>/<name>/meta.json`; one `PackageVersionMeta` **per version** at `pkg/<format>/<name>/<version>/meta.json` (not one growing array — avoids CAS-rewriting an unbounded list on every publish); `PackageTags` (npm `dist-tags`) at `pkg/<format>/<name>/tags.json`.
  - `PackageVersionMeta.format_metadata: serde_json::Value` — round-tripped verbatim, so npm's manifest fields never leak into this generic layer.
  - `Remote` methods: `get_package`/`put_package`, `get_package_version`/`put_package_version`, `list_package_versions`/`list_packages`, `get_tags`/`put_tags`, `package_blob_exists`/`upload_package_blob`/`download_package_blob`, `delete_package` (deletes only this package's metadata subtree — blobs are namespace-shared and never GC'd, same tradeoff LFS already has).
- **`auth/access/permissions.rs`**: add `effective_namespace_permission_for_user(backend, cipher, user, owner) -> Result<Option<Permission>>` — Owner if `owner` is the user themself or a group they own, else max over org-wide teams (`Team::repos: None`) they belong to. Deliberately **no** `user.default_permission` fallback (that would let any user publish under any namespace by default).
- **`auth/access/quota.rs`** + `User`/`Group`: add `package_bytes: u64`, fold into `check_quota`'s sum; add `bump_package_bytes` next to the existing `bump_pack_bytes`-style helper.
- **`domain/package.rs`** (new): `publish_version(ctx, remote, owner, NewPackageVersion)` — mirrors `domain/release.rs::add_asset`'s CAS shape: quota check → reject if version already exists (versions immutable, enforced by CAS) → dedup-upload each file's blob (only bump quota for genuinely-new blobs) → CAS-create `PackageMeta` (tolerate conflict = someone else's concurrent first publish) → CAS-create `PackageVersionMeta` → best-effort update `PackageTags`. Plus `fetch_version`, `delete_package` (namespace-Owner-only).
- Unit tests directly against the domain layer, no HTTP — same style as other `domain/` test files.

### Phase 2 — raw format over HTTP

Proves the generic storage/domain design end-to-end with the simplest possible protocol before npm's added JSON-manifest complexity.

- **`registry/` module** (new, sibling of `git/`): `mod.rs`, `http.rs` (shared classifier + axum middleware, mirrors `git::http::classify`/`middleware`), `auth.rs`, `raw.rs`.
- **`registry/auth.rs`**: `authorize_namespace(ctx, headers, owner, required) -> Result<Authz>` — reuses `Authz`/`Denial` from `git::http`. Order: `required <= Reporter` → always allowed (v1, no private packages) → Basic auth via `git::http::basic_credentials` (promote to `pub(crate)`) → `gqpat_`-prefixed username → PAT scope lookup keyed by namespace owner's `Uid` → plain username/password via `password::verify_password` (throttled through `ctx.login_attempts`, same as git auth) → `effective_namespace_permission_for_user`.
- **`registry/raw.rs`** routes under `/_packages/raw/{namespace}/{name}/{version}/{filename}`:
  - `PUT` — streaming upload, hash-verify-while-streaming-to-tempfile (same pattern as `git::lfs::upload_object_impl`), then `domain::package::publish_version`.
  - `GET` — streaming download, same tempfile-then-chunked-stream shape as `git::lfs::download_object_impl`; `Content-Disposition`/content-type-guessing borrowed from `webui/export.rs::release_asset`.
  - `GET {namespace}/{name}/{version}` — JSON listing of that version's files.
- **`validate.rs`** additions: `validate_package_segment` (flat segment, no `/`/`\`/`"`/control chars, bounded — same shape as `validate_asset_filename`), `validate_package_version` (opaque bounded string, no slash — format handlers may layer stricter semver on top). `filename` reuses `validate_asset_filename` verbatim.
- Wire the middleware into `gitquarry-webui/src/webui/mod.rs::run_server` as a sibling `.layer(...)` next to the existing `git::http::middleware` layer.
- Integration test: publish → re-publish identical bytes doesn't re-upload (dedup) → download → quota enforcement → permission denial for a non-member.

### Phase 3 — npm format over HTTP

- **`registry/npm.rs`** routes under `/_packages/npm/`:
  - `GET {**pkg}` — packument: `list_package_versions` + per-version `PackageVersionMeta`, inject `dist.tarball`/`dist.shasum`, merge `format_metadata`, read `PackageTags` for `dist-tags`.
  - `PUT {**pkg}` — classic npm publish (whole-document JSON with base64 `_attachments`, not a streaming upload — same limitation real registries like Verdaccio have); bounded body size, decode each attachment to a tempfile, sha256 it, call `domain::package::publish_version`.
  - `GET {pkg}/-/{filename}` — tarball download, same streaming shape as raw's GET.
  - `classify()` splits on the last `/-/` infix (same technique as `git::http::extract_variable_segment`) to tolerate scoped names (`@scope/name`) containing their own `/`.
- Manual verification: `npm publish`/`npm install` against a locally-running gitquarry pointed at the new registry URL — confirms `format_metadata` is actually sufficient for a real client.

### Phase 4 — web UI browsing

Package managers never need this (they talk the wire protocol directly), but a forge's human users expect to *see* what's published — GitLab's comparison explicitly has "UI de actividad por paquete" (`docs/forge-ui-feature-comparison.md:79`).

- **`auth/web/profile.rs`** (`/_profile/<name>`, the existing namespace page that already lists a user/group's repos via `list_owned_repos`/`OwnerRef` resolution, `profile.rs:28-61`): add a "Packages" section below the repo list — for each of the v1 formats (`npm`, `raw`), call `storage/package.rs::list_packages(format)` on that namespace's `Remote` and render name links. No privacy branching needed here (v1 packages are always public, per the read-visibility decision above) — simpler than the repo list's `can_see_private` logic.
- **New `webui/packages/` directory module** (mirrors the existing `webui/repo/`, `webui/repo_settings/` directory-module convention — keeps this off a single growing file per the project's no-oversized-files rule):
  - `webui/packages/mod.rs` + `detail.rs`: `GET /_profile/{name}/-/packages/{format}/{*pkg_name}` — lists versions via `list_package_versions`, shows each version's files with download links pointing straight at the existing Phase 2/3 endpoints (`/_packages/raw/...` or `/_packages/npm/.../-/...`) — the UI never re-implements blob serving, only links to it.
  - Registered as an ordinary `.route()` in `webui/mod.rs` (not the classifier middleware — these are plain browsing pages, not wire-protocol endpoints), same style as the existing `/_profile/{name}` route.
- Any new user-facing string goes through the project's existing locale mechanism — English by default, Spanish only inside `Locale::Es` literals, per the completed i18n migration.

### Phase 5 — Docker-OCI (fully designed here, implemented later)

Deferred out of v1 because it is roughly as much new work as Phases 2+3 combined — not a trivial third protocol adapter. Designed now so the deferral is a scoping choice, not a hand-wave:

- **Required routes** (OCI Distribution Spec v2): `GET /v2/` (ping), `GET/HEAD /v2/<name>/blobs/<digest>`, `POST /v2/<name>/blobs/uploads/` + `PATCH`/`PUT` (chunked upload), `GET/PUT/HEAD /v2/<name>/manifests/<reference>`, `GET /v2/<name>/tags/list`. `<name>` is itself one or more `/`-separated components — Docker already namespaces images this way (`<namespace>/<image>`), so no extra prefix is needed to express "owner/package," unlike npm/raw which needed the explicit `/_packages/` prefix.
- **Route collision**: `/v2/...` sits at web root, same level as the `/{*repo}` catch-all and git-smart-HTTP's own dispatch. Requires the same classifier-middleware approach as `registry::http`/`git::http`, registered as its own layer that intercepts the `/v2` prefix before the router's catch-all route ever matches. Confirmed during exploration: no existing top-level route currently claims `/v2`.
- **Storage/domain reuse**: full reuse of `storage/package.rs`'s content-addressed blob layer and the `PackageVersionMeta`/`PackageTags` shapes — an OCI manifest is just another digest-addressed blob, and a Docker tag maps directly onto `PackageTags`. The only genuinely new mechanism is chunked-upload session state (`POST .../blobs/uploads/` returns a session id, `PATCH` appends, final `PUT` commits) — modeled as a short-lived `UploadSession { uid, namespace, started_at }` backed by a server-side scratch tempfile (same "buffer to tempfile, hash, then commit" discipline used everywhere else in this codebase, e.g. `git::lfs::upload_object_impl`), not real S3 multipart.
- **Auth**: Docker clients expect the registry bearer-token challenge (`WWW-Authenticate: Bearer realm=...`), not static Basic auth — needs a small token-issuing endpoint wrapping `effective_namespace_permission_for_user`, since `docker login`/`docker push` hard-code this flow unlike npm/raw's plain Basic auth.

### Phase 6 — remaining ecosystems (Maven, PyPI, Helm, NuGet — paper design only)

Same precedent as Phase 5: designed now for completeness since the docs comparison (`docs/forge-ui-feature-comparison.md:78-80`) names all of these, but **not built** until one is actually requested — building all six formats speculatively up front would violate the project's no-speculative-features rule as surely as building none would violate "generic from the start." The generic `storage/package.rs` engine already carries every one of these without change; each is purely a new `registry/<format>.rs` adapter translating that ecosystem's wire protocol onto `publish_version`/`fetch_version`/`list_package_versions`.

- **`registry/maven.rs`** (`/_packages/maven/...`): Maven's own repo-layout convention *is* a path scheme — `<groupId-as-path>/<artifactId>/<version>/<artifactId>-<version>.<ext>` (jar/pom/sources/javadoc) maps directly onto `name = <groupId>:<artifactId>`, `version`, one `PackageFile` per extension. `maven-metadata.xml` (the version-list document Maven clients fetch first) is **generated on request** from `list_package_versions`, never stored — same "derived, not duplicated" principle npm's packument already uses in Phase 3. `.sha1`/`.md5` sibling-file requests served by reading the stored `sha256`... no: Maven wants sha1/md5 specifically, so these get computed at upload time alongside the sha256 already computed for content-addressing, stored as extra `PackageFile` fields (`sha1`, `md5`) — the one piece of real new work in this adapter.
- **`registry/pypi.rs`** (`/_packages/pypi/...`): index protocol per PEP 503/691 — `GET .../simple/<project>/` returns an HTML (or PEP-691 JSON) listing of file links, generated from `list_package_versions`. Publish (`twine upload`) posts `multipart/form-data`, not JSON or a raw stream — the one genuinely new piece is a multipart-form parser (axum's `Multipart` extractor) pulling out the wheel/sdist file plus metadata fields, then handing the file to the same `publish_version` call raw/npm already use.
- **`registry/helm.rs`** (`/_packages/helm/...`): classic chart-repo shape first (not OCI) — `GET index.yaml` generated from `list_packages`+`list_package_versions` (same derived-index principle as Maven/npm), `GET <name>-<version>.tgz` is a plain content-addressed download, identical in shape to raw's GET. OCI-based `helm push` (the modern alternative) piggybacks on Phase 5's Docker-OCI work instead of this adapter, since it speaks the same distribution-spec protocol — not duplicated here.
- **`registry/nuget.rs`** (`/_packages/nuget/...`): NuGet v3 requires a JSON "service index" at `index.json` advertising sub-resource URLs (search/publish/package-content) — a static document per instance, not per-package. Publish auth is the one real divergence: NuGet clients send `X-NuGet-ApiKey: <key>` as a plain header instead of Basic auth — needs a header-reading variant of `registry::auth::authorize_namespace` step 2, but resolves through the exact same PAT-scope lookup afterward.

### Phase 7 — private packages (visibility gate, concrete design)

The one deliberately-deferred v1 gap, now specified: add `packages_visibility: Visibility` to `User`/`Group` (reuses the existing `Visibility` enum from `storage/entities.rs`, default `Public` so v1 behavior is unchanged for anyone who doesn't touch the setting). `registry::auth::authorize_namespace`'s step 1 (`required <= Reporter → always Allowed`) becomes conditional: only short-circuits when `packages_visibility == Public`; otherwise falls through to the same `effective_namespace_permission_for_user` check writes already use. One field, one branch — no new permission tier, no per-package override (visibility is namespace-wide, matching how the namespace itself is the unit everywhere else in this design).

### Phase 8 — PAT UI for namespace scopes (concrete design)

`crates/gitquarry-webui/src/auth/web/pat.rs` currently builds `Pat.scopes` entries keyed by repo uid only (a repo picker in the token-creation form). Add a second picker mode: "namespace" vs "repo" scope, where namespace mode lists the user's own namespaces plus any group they belong to (same source `auth/web/profile.rs` already draws from), and on submit resolves the chosen namespace's owner `Uid` as the scope key — zero change to `Pat`'s schema (`Uid` is already kind-agnostic, per the Phase-2 design decision), purely a form/handler change in this one file.

### Phase 9 — federation read-only package browsing

Mirrors federation's existing browse layer (repo remote-browse via `/_remote/{peer}/{*repo}`) rather than inventing a new mechanism: expose a read-only package-listing/download proxy through the same `/_api/*` federation surface, gated by the same peer access-tier checks `federation_proxy.rs` already applies to repo browsing. Publish/delete never cross this boundary — only `list_packages`/`list_package_versions`/blob download, matching federation's existing "browse, never write" posture for everything else it exposes. No new peer-trust concept needed; this is an additional resource type under an access-control model that already exists.

### Explicitly still out of scope after all 9 phases

- Cross-repo/cross-namespace package search (would need a dedicated index, same caveat `docs/feature-compare.md:243` already flags for repo/code search at scale).
- Any garbage-collection/orphan-sweep for content-addressed blobs (LFS has the identical gap today — deleting a package's metadata never reclaims its now-unreferenced blob bytes; a shared solution for both is future work, not scoped here).

## Critical files

- `crates/gitquarry-core/src/storage/remote.rs` (widen visibility, add generic primitives)
- `crates/gitquarry-core/src/storage/package.rs` (new)
- `crates/gitquarry-core/src/domain/package.rs` (new)
- `crates/gitquarry-core/src/registry/{mod,http,auth,raw,npm}.rs` (new)
- `crates/gitquarry-core/src/auth/access/permissions.rs`
- `crates/gitquarry-core/src/auth/access/quota.rs`
- `crates/gitquarry-core/src/validate.rs`
- `crates/gitquarry-core/src/app.rs`
- `crates/gitquarry-webui/src/webui/mod.rs` (middleware + route wiring)
- `crates/gitquarry-webui/src/auth/web/profile.rs` (Packages section)
- `crates/gitquarry-webui/src/webui/packages/{mod,detail}.rs` (new, Phase 4)
- `crates/gitquarry-core/src/registry/{maven,pypi,helm,nuget}.rs` (new, Phase 6, paper design only)
- `crates/gitquarry-webui/src/auth/web/pat.rs` (Phase 8)
- `crates/gitquarry-webui/src/webui/federation_proxy.rs` (Phase 9)

## Verification

- `cargo test` at each phase boundary — Phase 1 domain-layer unit tests, Phase 2/3 integration tests against a local S3-compatible backend (same harness existing LFS/release tests use).
- `cargo clippy --all` / `cargo fmt --check` (project standard, `unsafe_code = forbid`).
- Manual: `npm publish` + `npm install` against a local gitquarry instance after Phase 3, confirming a real npm client round-trips correctly.
- Commit per phase, one logical change per commit, per project commit conventions.
