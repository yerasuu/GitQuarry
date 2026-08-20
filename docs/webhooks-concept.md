# Webhooks

## Context

Roadmap gap paired with CI — `docs/feature-compare.md` names CI+webhooks+notifications together as the top-impact cluster, and `docs/ci-cd-concept.md`'s Phase 0 explicitly depends on this doc landing first: the internal push-event emission point webhooks need is the *same* emission point `gitquarry-ci`'s trigger needs, designed once here rather than twice.

**Lives in `gitquarry-core`, not a separate binary** — same reasoning the package-registry doc used: outbound HTTP delivery to third-party URLs has no genuine "separately-deployed process / peer-trust boundary" rationale the way federation does. It's request/response plumbing bolted onto domain events that already happen in core.

## Design decisions (settled, not open)

| Question | Decision | Why |
|---|---|---|
| Event source | One internal emission point, `events::emit(ctx, RepoEvent)`, called from the places domain state actually changes — not something webhooks or CI each reinvent. | This *is* the Phase 0 `docs/ci-cd-concept.md` deferred; building it twice (once per consumer) would duplicate "what counts as a push" logic exactly the way that doc warned against. |
| v1 event types | `push`, `merge_request` (opened/updated/merged/closed), `issue` (opened/closed), `release` (published). | Covers GitHub-parity basics without shipping every issue/MR sub-event (comment, label, milestone) on day one — those are Phase 5, additive, not blocking. |
| Storage | New `Webhook` entity, own `s3db` collection scoped per-repo (`Collection::new_owned`, same pattern `access::repo_permission_index` already uses for a per-repo-scoped collection) — **not** folded into `RepoConfig` the way `labels: Vec<Label>` is. | A webhook holds a signing secret; embedding it in `RepoConfig` would put that secret in the payload of every ordinary repo-settings read. Same reasoning `docs/ci-cd-concept.md` gives for keeping `Secret` its own entity. |
| Delivery timing | Fire-and-forget background task, spawned the same detached way `ssh.rs:337`'s post-push block already spawns `update_primary_language` — a push must never block on a slow or dead webhook target. | Git push latency is a hard UX constraint; webhook delivery reliability is handled by retry, not by making the pusher wait. |
| Signing | HMAC-SHA256 over the raw JSON body with the webhook's own secret, sent as `X-GitQuarry-Signature-256`; event type as `X-GitQuarry-Event`. GitHub-compatible header shape. | Lets existing receiver-side tooling (already written against GitHub's convention) work against gitquarry with zero changes on the receiving end. |
| Payload DTOs | New `webhooks::payload` module in `gitquarry-core`, **not** `gitquarry-wire`. | `gitquarry-wire` is explicitly scoped to "types genuinely crossing 2+ boundaries" between the two gitquarry binaries (`CLAUDE.md`) — a webhook payload goes to an arbitrary third-party HTTP endpoint, not to `gitquarry-federation` or `gitquarry-ci`. Different contract entirely. |
| Reliability | Bounded retry (3 attempts, exponential backoff), each attempt recorded as a `WebhookDelivery` (status code, duration, truncated response body) for a delivery-history view — capped at the last 50 per webhook, oldest evicted. | Matches what GitHub/GitLab's own webhook settings UIs show; a hard cap keeps this from becoming unbounded per-repo storage growth. |
| Target URL validation | `validate_webhook_url`: same shape checks as `validate_mirror_url` (bounded length, `http(s)://` only, no control/whitespace) **plus** a resolved-IP check done at *delivery* time, not just save time. | See SSRF section below — this is the one genuinely security-critical piece of this design and gets its own explicit treatment rather than inheriting `validate_mirror_url`'s checks by assumption. |
| `gitquarry-ci` wiring | CI does **not** register as a generic `Webhook` row and does **not** go through the SSRF-checked delivery path. It subscribes to the same internal `RepoEvent` stream directly and calls its own loopback (`ci_url`, `internal_token`) — the bidirectional-loopback IPC `docs/ci-cd-concept.md` already specifies. | CI is same-trust-tier as core (operator-configured, fixed address), not a third-party URL a repo owner typed in — running it through SSRF checks meant for untrusted user input would be a category error, not defense in depth. |

## SSRF protection (why this needs its own section)

A webhook target URL is attacker-reachable input in the standard sense: any user with push access to a repo can set it to anything. Unlike `validate_mirror_url` (an external-mirror URL the *same* user is trusting with their own push/fetch credentials — self-directed risk), a webhook fires automatically on every matching event, making it a classic vector for pivoting the server into hitting internal-only services.

Concrete rules for `validate_webhook_url` / the delivery path:

1. **Resolve at delivery time, not just save time.** A save-time-only check is defeated by DNS rebinding (the hostname resolves to a public IP when saved, a private one when actually dialed). Resolve immediately before each delivery attempt.
2. **Reject if the resolved IP is loopback, private, link-local, multicast, or unspecified** — `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `::1`, `fc00::/7`, `fe80::/10`, `0.0.0.0`/`::`. If a hostname resolves to multiple IPs, reject if *any* of them fall in these ranges — don't just check the first.
3. **Never follow redirects automatically.** Disable redirect-following in the HTTP client entirely; treat any `3xx` response as a failed delivery attempt. Re-validating every redirect hop is more code for the same outcome a flat "don't" already gives.
4. **Bounded timeout and bounded response-body read** per attempt (e.g. 10s connect+total, response body capped at a few KB before truncation) — a malicious or broken target shouldn't be able to pin a delivery worker indefinitely.
5. **No operator allowlist escape hatch in v1.** Deliberately not building a bypass for "but I actually want to hit an internal service" — that's a legitimate but rare need that can be added later behind explicit opt-in config if someone asks, not scaffolded speculatively now.

## Implementation

### Phase 1 — internal event bus (the `docs/ci-cd-concept.md` Phase 0 dependency)

- `crates/gitquarry-core/src/events/mod.rs` (new): `RepoEvent` enum (`Push`, `MergeRequest`, `Issue`, `Release` variants carrying just the ids/shas needed, not full domain objects — consumers fetch detail via existing domain functions if they need more), `emit(ctx, event)` fanning out to registered consumers.
- Wire the `Push` variant into the existing post-`receive-pack`-success block (`git/ssh.rs:337`, `git/http.rs`'s equivalent) — today that block only re-syncs the mirror; it gains one more detached call.
- Wire `MergeRequest`/`Issue`/`Release` variants into the relevant `domain/merge_request/`, `domain/issue.rs`, `domain/release.rs` mutation points, right after each successful CAS write.
- v1 consumer registered here: none yet — Phase 2/3 add the webhook dispatcher, Phase 6 of `docs/ci-cd-concept.md` adds CI. Keeps this phase reviewable on its own.

### Phase 2 — `Webhook` entity + validation

- `crates/gitquarry-core/src/storage/webhooks.rs` (new): `Webhook { uid, repo_uid, url, secret, events: BTreeSet<EventKind>, active: bool, created_at }`, `WebhookDelivery { webhook_uid, event, status_code, duration_ms, response_excerpt, attempted_at }` (capped ring, oldest evicted past 50).
- `validate.rs`: `validate_webhook_url` (shape) — the resolved-IP SSRF check lives in the delivery path (Phase 3), not here, since it's inherently a delivery-time check per the DNS-rebinding point above.
- `domain/webhook.rs` (new): `create`, `update`, `delete`, `list_for_repo` — repo-Owner-permission-gated, mirrors `domain/release.rs`'s shape.

### Phase 3 — delivery engine

- `crates/gitquarry-core/src/webhooks/delivery.rs` (new): registers as a Phase-1 event consumer; for each repo's active webhooks matching the event's kind, builds the JSON payload (`webhooks/payload.rs`), HMAC-signs it, and dispatches with the SSRF-checked, non-redirecting, bounded-timeout HTTP client described above.
- Retry: 3 attempts, exponential backoff (e.g. 1s/5s/25s), each attempt appended to that webhook's `WebhookDelivery` ring.
- Unit tests: SSRF IP-range rejection (every listed range plus a couple of public-IP positive cases), HMAC signature correctness against a known vector, redirect-response treated as failure.

### Phase 4 — settings UI

- `crates/gitquarry-webui/src/auth/web/repo_settings/webhooks.rs` (new): list/create/edit/delete per repo (mirrors the existing `labels` settings page pattern), event-kind checkboxes, delivery-history view per webhook, a manual "redeliver" action re-running Phase 3's dispatch for one historical event.
- Secret shown once at creation, never re-displayed after (same treatment `docs/ci-cd-concept.md` gives its own `Secret` entity).

### Phase 5 — event coverage expansion

- Additive `RepoEvent` variants: comment created (issue/MR), label changed, milestone changed, MR review submitted. Each is a small addition to an existing domain mutation point, not new plumbing — Phase 1's bus already carries arbitrary variants.

### Phase 6 — `gitquarry-ci` wiring

- Cross-referenced from `docs/ci-cd-concept.md`'s own Phase 1/IPC design: once this doc's Phase 1 event bus exists, `gitquarry-ci`'s trigger path becomes "register as a `Push`-event consumer, call the loopback client" — no separate design needed here, just confirms the dependency is satisfied.

### Explicitly out of scope

- Per-event payload customization / templating (GitLab-style "select which fields to send") — v1 sends the full fixed payload shape per event kind.
- Webhook signing algorithm choice (only HMAC-SHA256, no SHA-1 legacy option) — nothing in this project needs to interoperate with a receiver that only understands the older, weaker scheme.
- IPv6-specific SSRF edge cases beyond the ranges listed above (e.g. IPv4-mapped IPv6 addresses `::ffff:127.0.0.1`) are **not** an afterthought — the delivery-time resolver must normalize and check the mapped-IPv4 form too, called out explicitly here so Phase 3 doesn't ship without it.

## Critical files

- `crates/gitquarry-core/src/events/mod.rs` (new)
- `crates/gitquarry-core/src/storage/webhooks.rs` (new)
- `crates/gitquarry-core/src/domain/webhook.rs` (new)
- `crates/gitquarry-core/src/webhooks/{mod,delivery,payload}.rs` (new)
- `crates/gitquarry-core/src/validate.rs` (`validate_webhook_url`)
- `crates/gitquarry-core/src/git/{ssh,http}.rs` (Push event emission)
- `crates/gitquarry-core/src/domain/{merge_request/,issue.rs,release.rs}` (event emission)
- `crates/gitquarry-webui/src/auth/web/repo_settings/webhooks.rs` (new)

## Verification

- `cargo test` at each phase boundary — Phase 3's SSRF/HMAC/redirect tests are the load-bearing ones, written before the delivery code that must satisfy them.
- Integration test: local test HTTP server receiving a signed delivery, verifying signature + payload shape end to end.
- Manual: point a webhook at a private-range IP, confirm delivery is rejected and recorded as a failed `WebhookDelivery`, not silently dropped or silently sent.
- `cargo clippy --all` / `cargo fmt --check`, `unsafe_code = forbid`.
- Commit per phase, one logical change per commit.
