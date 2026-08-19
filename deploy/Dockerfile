# Build context must be the repo root, not this directory (COPY below needs
# Cargo.toml/Cargo.lock/crates from there): docker build -f deploy/Dockerfile .
FROM rust:1.97-bookworm AS builder

WORKDIR /usr/src/gitquarry
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY gitquarry-ci-sdk-contract-tests ./gitquarry-ci-sdk-contract-tests

# The shipped bin package is still named `gitquarry`, but it now lives in
# `crates/gitquarry-server` — `gitquarry-core` went lib-only when the webui
# crate split out (docs/webui-crate-split-concept.md), so the bin crate is
# the one place depending on both `gitquarry-core` and `gitquarry-webui`.
RUN cargo install --path crates/gitquarry-server --locked --root /usr/local

FROM debian:trixie-slim

# The refs/summary web views (crates/gitquarry-core/src/git/browse/refs.rs)
# for-each-ref with `%(signature:grade)` -- bookworm's git 2.39 rejects
# that atom outright ("unknown field name"), trixie's 2.47 accepts it.
RUN apt-get update && \
    apt-get install -y --no-install-recommends git ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/gitquarry /usr/local/bin/gitquarry

WORKDIR /app

EXPOSE 8080 2222

ENTRYPOINT ["gitquarry"]
# Defaults bind 127.0.0.1, unreachable from outside the container even with
# published ports. Override these args (docker run / compose `command:`) if
# you need different binds, but never point them at 127.0.0.1 in a container.
# Only ssh:// and git smart-HTTP (over the web port) are ever served — see
# serve.rs, there's no flag to turn on anything else.
CMD ["--web-bind", "0.0.0.0:8080", "--ssh-bind", "0.0.0.0:2222"]