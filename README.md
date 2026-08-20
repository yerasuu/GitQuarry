# GitQuarry

A self-hosted git forge whose only storage is an object bucket. One binary
serves `ssh://`, git smart-HTTP and a web UI over the same S3-compatible
bucket — no database, no separate git server, and nothing leaves the process
unencrypted.

> **Pre-alpha.** Formats and flags still move between versions. Run it against
> a bucket you can afford to lose.

## What it offers

- **Git hosting on object storage** — multi-repo, one repo per requested path
  under the bucket's `repos/` prefix. Real `git upload-pack`/`receive-pack`
  are exec'd, so protocol negotiation stays git's; concurrent pushes are
  resolved with compare-and-swap on the refs object. Git LFS (batch API,
  locks), instance-signed merge commits, CODEOWNERS.
- **Two transports, no surprises** — `ssh://` with pubkey auth and git
  smart-HTTP with basic auth, both on the same per-repo permission model.
  There is no `git://` daemon and no direct-to-bucket remote helper.
- **Encrypted at rest, always** — XChaCha20Poly1305 under an Argon2id-derived
  per-bucket key. Mandatory, with no opt-out; the storage provider sees object
  names and nothing else.
- **Web UI** — summary, refs, tree, blob with syntax highlighting, log, commit
  diff, compare, blame, stats, activity feed, releases with assets, per-repo
  settings. Markdown with fenced highlighting and GitHub-style alerts; Atom
  feeds and patch/rawdiff/snapshot exports; light/dark themes; English and
  Spanish. Server-rendered, with JavaScript as progressive enhancement only.
- **Pull requests** — reviews, inline comments, required reviewers, merge /
  squash / rebase / fast-forward, and conflict reports that hand you the
  commands to resolve locally.
- **Accounts and permissions** — SSH keys, password and OIDC login, TOTP
  two-factor with recovery codes, personal access tokens, GPG keys, groups
  with owners and teams, a five-tier permission ladder
  (guest/reporter/developer/maintainer/owner), and public/limited/private repo
  visibility.
- **Repo management** — create, fork, star, watch, topics, archive, transfer
  between namespaces, protected branches and tags, signed-commit enforcement,
  default-branch selection, and instance-wide search over filenames and commit
  messages.
- **Ops** — audit log, `/_healthz` and `/_metrics`, server settings editable
  from the UI with conflict detection, idle- and size-based mirror eviction,
  graceful shutdown.

Storage drivers: S3, Cloudflare R2, Azure Blob Storage, Google Cloud Storage,
and a local filesystem directory.

## Quick start

### Docker

```bash
cp deploy/gitquarry.minio-test.toml.example gitquarry.toml   # then edit it
docker compose -f deploy/docker-compose.yml up -d
```

Ports: `8080` for the web UI and smart-HTTP, `2222` for SSH.

### From source

Needs Rust 1.97+ and network access to the `s3db` dependency repo
(`.cargo/config.toml` already sets `net.git-fetch-with-cli`). At runtime the
host also needs `git`, `ssh-keygen` and `gpg` on `PATH`.

```bash
cargo build --release -p gitquarry
./target/release/gitquarry --bucket mybucket
```

With no config file anywhere in the resolution order, the first run serves a
setup wizard on `--web-bind`: pick a driver, enter credentials, and it
verifies them against the bucket, writes the config, and creates the admin
account — no restart needed.

### Cloning

```bash
git clone ssh://git@host:2222/team/project
git clone https://host/team/project
```

## Configuration

`gitquarry.toml` lives at the project root. It is resolved from
`GITQUARRY_CONFIG`, then the working directory, then the git root, then the
user-global path.

```toml
[buckets.myrepo]
driver = "s3"              # s3 | r2 | azure | gcs | local
bucket = "myrepo"
region = "eu-west-1"
endpoint_url = "https://s3.example.com"
access_key_id = "..."
secret_access_key = "..."
path_style = true
passphrase = "..."         # mandatory: encrypts everything uploaded

# allow_push_create = true # off by default: pushing an unknown repo fails
```

Users, groups, per-repo settings and the SSH host key are not local files:
they live encrypted in the bucket under `.conf/`, so a fresh container keeps
its identity.

## Running

```
gitquarry [--bucket NAME] [--web-bind 0.0.0.0:8080] [--ssh-bind 0.0.0.0:2222]
          [--mirrors-dir PATH] [--mirrors-max-age 3d] [--mirrors-max-size 5G]
```

`--bucket` is optional when the config declares exactly one. The bare mirrors
under `--mirrors-dir` are a cache: they are evicted by age and total size, and
rebuilt from the bucket on the next request.

| Command | What it does |
| --- | --- |
| `gitquarry fsck [--apply]` | Report (or remove) objects gitquarry never wrote inside a repo prefix |
| `gitquarry repack <repo> [--apply]` | Consolidate a repo's accumulated packs into one |
| `gitquarry backfill-repo-index` | One-time backfill for repos predating the owner/visibility indexes |

The web UI ships no TLS of its own. Put it behind a reverse proxy for anything
that is not localhost.

## Project layout

| Crate | Role |
| --- | --- |
| `gitquarry-server` | The shipped `gitquarry` binary: CLI, setup wizard, wiring |
| `gitquarry-core` | ssh://, smart-HTTP, git browsing, storage, crypto, auth |
| `gitquarry-domain` | Repo/user/group domain layer and maintenance commands |
| `gitquarry-webui` | Browsing and management UI, login, settings, search |
| `gitquarry-wire` | Shared wire types |
| `gitquarry-auth-crypto` | Password hashing, TOTP, OIDC |
| `gitquarry-gpg` | GPG key handling and signature verification |
| `gitquarry-i18n` | Locale catalog |

The storage engine is [`s3db`](https://git.yerasuu.com/yerasuu/s3db), a
separate project consumed as a tag-pinned git dependency.

`develop` and `master` carry the core only. Optional modules live on their own
branches, each a snapshot of the last state where the module built, to be
reintegrated one at a time: `feature/cicd` (CI/CD runner and SDKs),
`feature/federation` (peer discovery, remote browse/fork, PR proxy),
`feature/artifacts` (package registry), `feature/tracker` (issues, boards,
milestones), `feature/mirror` (GitHub/GitLab/Codeberg mirroring).

## Learn more

- [CHANGELOG.md](CHANGELOG.md) — user-facing feature history, also served at
  `/_changelog`
- [docs/](docs/) — design notes and roadmap
- [CLAUDE.md](CLAUDE.md) — project conventions

## Licence

[AGPL-3.0-or-later](LICENSE).
