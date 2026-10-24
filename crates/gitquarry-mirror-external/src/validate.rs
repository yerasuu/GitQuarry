//! `ExternalMirror` field validation — moved verbatim out of
//! `gitquarry-core::validate` (zero other callers anywhere in the
//! workspace, per `docs/mirror-external-extraction-concept.md`'s audit).

use anyhow::{Result, bail};

const MAX_MIRROR_URL_LEN: usize = 512;
const MAX_MIRROR_TOKEN_LEN: usize = 512;
const MAX_MIRROR_LABEL_LEN: usize = 128;

/// Validates an [`crate::access::ExternalMirror`]'s `remote_url` — a plain
/// `https://host/owner/repo[.git]` (never carries a token, unlike the
/// credentialed URL built at sync time from it), only `http`/`https`
/// accepted since the sync path always builds an `https://` push/fetch URL
/// out of it.
pub fn validate_mirror_url(url: &str) -> Result<()> {
    if url.is_empty() || url.len() > MAX_MIRROR_URL_LEN {
        bail!("empty or too-long mirror URL (max {MAX_MIRROR_URL_LEN})");
    }
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else {
        bail!("invalid mirror URL (must start with http:// or https://): {url}");
    };
    if url.contains(|c: char| c.is_control() || c.is_whitespace()) {
        bail!("invalid mirror URL (control or whitespace characters)");
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        // `https:///path` — a URL with no host at all. Nothing can be
        // reached through it, and it would slip past the host allowlist
        // (`check_host_allowed`) by having no host to match.
        bail!("invalid mirror URL (no host): {url}");
    }
    // No userinfo. The sync path hands `git` the credential through its
    // environment (`git_remote::TOKEN_ENV`) and this URL verbatim, so a
    // `user:pass@` stored here would be a second, attacker-chosen
    // credential travelling to a host of the stored value's choosing.
    if authority.contains('@') {
        bail!("invalid mirror URL (credentials in the URL are not accepted): {url}");
    }
    if is_link_local(authority) {
        // Never a real forge, always the cloud instance-metadata endpoint.
        // Everything else (loopback, RFC1918) stays allowed on purpose:
        // mirroring to a forge on an internal network is a legitimate
        // deployment, so this rejects only the address with no legitimate
        // reading.
        bail!("invalid mirror URL (link-local address): {url}");
    }
    Ok(())
}

/// `true` if `authority` (`host[:port]`, possibly `[v6]:port`) names a
/// link-local address — IPv4 `169.254.0.0/16` or IPv6 `fe80::/10`.
/// Textual, not a DNS lookup: a hostname that *resolves* to one isn't
/// caught here, and can't be without re-resolving at connect time anyway.
fn is_link_local(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        return v4.is_link_local();
    }
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        // `fe80::/10`, hand-rolled because `Ipv6Addr::is_unicast_link_local`
        // is still unstable. The mapped-v4 arm catches `::ffff:169.254.x.x`,
        // which resolves to the same metadata endpoint.
        let link_local_v6 = v6.segments()[0] & 0xffc0 == 0xfe80;
        return link_local_v6 || v6.to_ipv4_mapped().is_some_and(|v| v.is_link_local());
    }
    false
}

/// Validates an [`crate::access::ExternalMirror`]'s stored provider token
/// before it's saved — opaque, minted by the external forge, so there's
/// nothing about its shape to check beyond "not empty, not unbounded, no
/// control characters". Also rejects `@`/`:`/`/`: none of
/// GitHub/GitLab/Codeberg's own token formats contain these, and keeping
/// the value shaped like a bare secret means it stays safe to hand to
/// `git`'s credential helper (and to the forge clients' `Authorization`
/// header) without any escaping question ever arising.
pub fn validate_mirror_token(token: &str) -> Result<()> {
    if token.trim().is_empty() || token.len() > MAX_MIRROR_TOKEN_LEN {
        bail!("empty or too-long mirror token (max {MAX_MIRROR_TOKEN_LEN})");
    }
    if token.contains(|c: char| c.is_control()) {
        bail!("invalid mirror token (control characters)");
    }
    if token.contains(['@', ':', '/']) {
        bail!("invalid mirror token (can't contain '@', ':' or '/')");
    }
    Ok(())
}

/// Validates an [`crate::access::ExternalMirror`]'s display label.
pub fn validate_mirror_label(label: &str) -> Result<()> {
    if label.len() > MAX_MIRROR_LABEL_LEN {
        bail!("too-long mirror label (max {MAX_MIRROR_LABEL_LEN})");
    }
    if label.contains(|c: char| c.is_control()) {
        bail!("invalid mirror label (control characters)");
    }
    Ok(())
}

/// The forges this build has a client for, and so mirrors to with nothing
/// configured — re-exported so the settings page can name them without
/// depending on `gitquarry-forge-clients` itself.
pub const BUILTIN_HOSTS: [&str; 3] = gitquarry_forge_clients::PUBLIC_FORGE_HOSTS;

/// The host allowlist actually in force: every forge this build has a
/// client for, plus whatever the instance added
/// (`ServerSettings::mirror_extra_hosts`).
///
/// A union rather than a replacement, so the providers the code already
/// speaks to are never something an operator has to remember to re-list —
/// the configured list exists to *widen* this to a self-hosted forge, not
/// to narrow the feature's own targets. Turning mirroring off is a matter
/// of not configuring one (and the owner tier that gates doing so), not of
/// emptying this.
pub fn effective_allowed_hosts(settings: &gitquarry_core::storage::entities::ServerSettings) -> Vec<String> {
    let mut hosts: Vec<String> = BUILTIN_HOSTS.iter().map(|host| host.to_string()).collect();
    for extra in &settings.mirror_extra_hosts {
        let extra = extra.trim().to_ascii_lowercase();
        if !extra.is_empty() && !hosts.contains(&extra) {
            hosts.push(extra);
        }
    }
    hosts
}

/// Host part of an already-[`validate_mirror_url`]-checked URL, lowercased
/// and without any port — what [`check_host_allowed`] matches against.
/// `None` when the string carries no `scheme://` at all.
pub fn mirror_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        // `[v6]:port` — the brackets belong to the authority, not the host.
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Refuses a `remote_url` whose host the instance doesn't allow an external
/// mirror to target (`ServerSettings::mirror_allowed_hosts`).
///
/// An empty list allows nothing, deliberately: configuring a mirror makes
/// *the server* open connections to a host of the configurer's choosing,
/// from the server's own network position, so it stays shut until an
/// instance admin opens it host by host.
///
/// Matching is on the *name*, never on a resolved address. The ref sync
/// shells out to `git`, which resolves inside its own process — an
/// address-range check here would be a TOCTOU with DNS rebinding on the far
/// side of it, i.e. the appearance of a control rather than one.
///
/// `*.example.com` matches a subdomain of `example.com` at any depth and
/// not `example.com` itself: a wildcard is a statement about a delegated
/// zone, and an operator who means both writes both.
///
/// A string with no `scheme://` is left alone — [`validate_mirror_url`]
/// already refuses to store one, so the only way to hold one is to write it
/// into the bucket directly, and it names a local path rather than any
/// egress. A `scheme://` with no host is refused like any other host that
/// isn't on the list.
pub fn check_host_allowed(remote_url: &str, allowed: &[String]) -> Result<()> {
    if !remote_url.contains("://") {
        return Ok(());
    }
    let host = mirror_host(remote_url).unwrap_or_default();
    let matched = allowed.iter().any(|pattern| match pattern.strip_prefix("*.") {
        Some(zone) => {
            host.len() > zone.len() + 1 && host.ends_with(zone) && host.as_bytes()[host.len() - zone.len() - 1] == b'.'
        }
        None => host == *pattern,
    });
    if !matched {
        bail!(
            "{}",
            gitquarry_i18n::shared::settings::mirror_host_not_allowed(gitquarry_i18n::Locale::DEFAULT, &host)
        );
    }
    Ok(())
}

const MAX_PUBLISH_PATTERN_LEN: usize = 256;
const MAX_PUBLISH_PATTERNS: usize = 256;
const MAX_REF_PATTERN_LEN: usize = 200;
const MAX_IDENTITY_FIELD_LEN: usize = 254;
const MAX_SPDX_LEN: usize = 64;
const MAX_TREE_PATH_LEN: usize = 4096;

/// Validates one path glob of a publish mirror's filter, from either source
/// (the mirror entity or the in-repo `.gitquarry/publish.toml`).
///
/// `!` negation is refused rather than supported: the in-repo file may only
/// ever add exclusions (`docs/publish-mirror-concept.md` §2), and a negated
/// pattern is exactly a way to remove one. A leading `#` is refused too,
/// since `.gitignore` would read it as a comment and silently drop the rule.
pub fn validate_publish_pattern(pattern: &str) -> Result<()> {
    if pattern.trim().is_empty() || pattern.len() > MAX_PUBLISH_PATTERN_LEN {
        bail!("empty or too-long path pattern (max {MAX_PUBLISH_PATTERN_LEN})");
    }
    if pattern.contains(|c: char| c.is_control()) {
        bail!("invalid path pattern (control characters): {pattern:?}");
    }
    if pattern.starts_with('!') {
        bail!("invalid path pattern (negation can't remove an exclusion): {pattern}");
    }
    if pattern.starts_with('#') {
        bail!("invalid path pattern (a leading '#' is a comment in .gitignore syntax): {pattern}");
    }
    if pattern.trim() != pattern {
        bail!("invalid path pattern (leading or trailing whitespace): {pattern:?}");
    }
    if pattern.split('/').any(|seg| seg == "..") {
        bail!("invalid path pattern ('..' segment): {pattern}");
    }
    Ok(())
}

/// Validates a whole list of [`validate_publish_pattern`]s, bounding its size.
pub fn validate_publish_patterns(patterns: &[String]) -> Result<()> {
    if patterns.len() > MAX_PUBLISH_PATTERNS {
        bail!("too many path patterns (max {MAX_PUBLISH_PATTERNS})");
    }
    for pattern in patterns {
        validate_publish_pattern(pattern)?;
    }
    Ok(())
}

/// Validates a `publish_branches`/`publish_tags` pattern: a short name
/// (`master`, `release/*`, `v*`) matched under `refs/heads/` or `refs/tags/`
/// only. A full ref name is refused outright so that nobody reads
/// `refs/merge-requests/*` in this list as something that could work — the
/// namespaces outside heads and tags are never publishable
/// (`docs/publish-mirror-concept.md` §2, "Refs, not just paths").
pub fn validate_publish_ref_pattern(pattern: &str) -> Result<()> {
    if pattern.is_empty() || pattern.len() > MAX_REF_PATTERN_LEN {
        bail!("empty or too-long ref pattern (max {MAX_REF_PATTERN_LEN})");
    }
    if pattern.starts_with("refs/") {
        bail!("invalid ref pattern (a short branch or tag name, not a full ref): {pattern}");
    }
    if pattern
        .contains(|c: char| c.is_control() || c.is_whitespace() || matches!(c, ':' | '^' | '~' | '?' | '[' | '\\'))
    {
        bail!("invalid ref pattern (characters git refuses in a ref name): {pattern:?}");
    }
    if pattern.starts_with(['/', '-', '.']) || pattern.ends_with(['/', '.']) || pattern.contains("..") {
        bail!("invalid ref pattern: {pattern}");
    }
    if pattern.contains("//") || pattern.contains("@{") || pattern.ends_with(".lock") {
        bail!("invalid ref pattern: {pattern}");
    }
    Ok(())
}

/// Validates an email on either side of a publish mirror's identity map.
/// It ends up between `<`/`>` in a commit header, so anything that could
/// close the bracket or start a new header line is refused.
pub fn validate_identity_email(email: &str) -> Result<()> {
    if email.is_empty() || email.len() > MAX_IDENTITY_FIELD_LEN {
        bail!("empty or too-long identity email (max {MAX_IDENTITY_FIELD_LEN})");
    }
    if email.contains(|c: char| c.is_control() || c.is_whitespace() || matches!(c, '<' | '>')) || !email.contains('@') {
        bail!("invalid identity email: {email:?}");
    }
    Ok(())
}

/// Validates the public name an identity mapping writes into a commit
/// header — same header-breaking characters as [`validate_identity_email`].
pub fn validate_identity_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > MAX_IDENTITY_FIELD_LEN {
        bail!("empty or too-long identity name (max {MAX_IDENTITY_FIELD_LEN})");
    }
    if name.contains(|c: char| c.is_control() || matches!(c, '<' | '>')) {
        bail!("invalid identity name: {name:?}");
    }
    Ok(())
}

/// Validates a declared SPDX license expression (`MIT`, `Apache-2.0`,
/// `GPL-2.0-or-later WITH Classpath-exception-2.0`): the characters the SPDX
/// grammar uses, nothing that could escape the text it is compared against.
pub fn validate_spdx_id(spdx: &str) -> Result<()> {
    if spdx.trim().is_empty() || spdx.len() > MAX_SPDX_LEN {
        bail!("empty or too-long SPDX id (max {MAX_SPDX_LEN})");
    }
    if !spdx
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | ' ' | '(' | ')' | ':'))
    {
        bail!("invalid SPDX id: {spdx:?}");
    }
    Ok(())
}

/// Validates a mirror id as [`crate::access::generate_mirror_id`] mints
/// it: 16 lowercase hex digits. A publish mirror's id ends up in its bucket
/// keys (`repos/<uid>/publish/<id>/…`), so nothing else may pass for one.
pub fn validate_mirror_id(id: &str) -> Result<()> {
    if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        bail!("invalid mirror id (16 lowercase hex characters): {id:?}");
    }
    Ok(())
}

/// Validates a full 40-hex SHA-1 object id.
pub fn validate_sha(sha: &str) -> Result<()> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        bail!("invalid object id (40 lowercase hex characters): {sha:?}");
    }
    Ok(())
}

/// Validates a tree path as typed into a secret-scan allowlist entry. Empty
/// is allowed: it names a commit message rather than a file.
pub fn validate_tree_path(path: &str) -> Result<()> {
    if path.len() > MAX_TREE_PATH_LEN {
        bail!("too-long path (max {MAX_TREE_PATH_LEN})");
    }
    if path.contains(|c: char| c.is_control()) || path.starts_with('/') || path.split('/').any(|seg| seg == "..") {
        bail!("invalid path: {path:?}");
    }
    Ok(())
}
