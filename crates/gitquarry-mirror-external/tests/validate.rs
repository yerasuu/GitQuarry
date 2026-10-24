use gitquarry_mirror_external::validate::*;

#[test]
fn mirror_url_requires_http_scheme_and_rejects_whitespace() {
    assert!(validate_mirror_url("https://github.com/acme/demo.git").is_ok());
    assert!(validate_mirror_url("http://gitea.internal/acme/demo").is_ok());
    assert!(validate_mirror_url("").is_err());
    assert!(validate_mirror_url("git://github.com/acme/demo").is_err());
    assert!(validate_mirror_url("https://github.com/acme/de mo").is_err());
    assert!(validate_mirror_url(&format!("https://{}", "x".repeat(600))).is_err());
}

#[test]
fn mirror_token_rejects_empty_too_long_and_control_chars() {
    assert!(validate_mirror_token("ghp_abc123").is_ok());
    assert!(validate_mirror_token("").is_err());
    assert!(validate_mirror_token("   ").is_err());
    assert!(validate_mirror_token(&"a".repeat(600)).is_err());
    assert!(validate_mirror_token("tok\nwith\nnewline").is_err());
}

#[test]
fn mirror_token_rejects_url_authority_characters() {
    assert!(validate_mirror_token("evil@host.com").is_err());
    assert!(validate_mirror_token("tok:with:colon").is_err());
    assert!(validate_mirror_token("tok/with/slash").is_err());
}

#[test]
fn mirror_label_rejects_too_long_and_control_chars() {
    assert!(validate_mirror_label("").is_ok());
    assert!(validate_mirror_label("backup mirror").is_ok());
    assert!(validate_mirror_label(&"x".repeat(200)).is_err());
    assert!(validate_mirror_label("bad\nlabel").is_err());
}

#[test]
fn validate_mirror_url_rejects_credentials_in_the_url() {
    // The sync path hands this URL to `git` verbatim; a stored `user:pass@`
    // would be a second credential of the stored value's choosing, sent to
    // a host of its choosing.
    assert!(validate_mirror_url("https://attacker@github.com/o/r.git").is_err());
    assert!(validate_mirror_url("https://user:pass@github.com/o/r.git").is_err());
    // A `@` past the authority is just part of the path, and fine.
    assert!(validate_mirror_url("https://github.com/o/r@v1.git").is_ok());
}

#[test]
fn validate_mirror_url_rejects_link_local_addresses() {
    // The cloud instance-metadata endpoint, in every spelling.
    assert!(validate_mirror_url("http://169.254.169.254/latest/meta-data/").is_err());
    assert!(validate_mirror_url("http://169.254.169.254:80/o/r.git").is_err());
    assert!(validate_mirror_url("http://[fe80::1]/o/r.git").is_err());
    assert!(validate_mirror_url("http://[fe80::1]:8080/o/r.git").is_err());
    assert!(validate_mirror_url("http://[::ffff:169.254.169.254]/o/r.git").is_err());
}

/// Loopback and RFC1918 stay allowed on purpose: mirroring to a forge on an
/// internal network is a legitimate deployment, so this validator rejects
/// only the address range that has no legitimate reading.
#[test]
fn validate_mirror_url_still_allows_internal_forges() {
    assert!(validate_mirror_url("http://127.0.0.1:3000/o/r.git").is_ok());
    assert!(validate_mirror_url("http://192.168.1.10/o/r.git").is_ok());
    assert!(validate_mirror_url("http://git.internal.example/o/r.git").is_ok());
    assert!(validate_mirror_url("https://github.com/o/r.git").is_ok());
}

#[test]
fn a_url_with_no_host_is_refused() {
    // Would otherwise slip past the host allowlist by having no host to
    // match against.
    assert!(validate_mirror_url("https:///acme/demo.git").is_err());
    assert!(validate_mirror_url("https://github.com/acme/demo.git").is_ok());
}

#[test]
fn the_matcher_allows_nothing_on_an_empty_list() {
    // The matcher itself is unconditional — what an instance actually
    // allows comes from `effective_allowed_hosts`, below.
    let nothing_allowed: Vec<String> = Vec::new();
    assert!(check_host_allowed("https://github.com/a/b.git", &nothing_allowed).is_err());
}

#[test]
fn a_stock_instance_already_allows_every_forge_with_a_client() {
    let stock = gitquarry_core::storage::entities::ServerSettings::default();
    let allowed = effective_allowed_hosts(&stock);
    for host in BUILTIN_HOSTS {
        assert!(allowed.iter().any(|h| h == host), "{host} isn't allowed out of the box");
        assert!(check_host_allowed(&format!("https://{host}/a/b.git"), &allowed).is_ok());
    }
    assert!(check_host_allowed("https://gitlab.internal.test/a/b.git", &allowed).is_err());
}

#[test]
fn a_configured_host_widens_the_set_without_replacing_it() {
    // The stored list adds a self-hosted forge; it never costs the instance
    // the providers this build already speaks to.
    let settings = gitquarry_core::storage::entities::ServerSettings {
        mirror_extra_hosts: vec!["gitlab.internal.test".to_string(), "GitHub.com".to_string()],
        ..Default::default()
    };
    let allowed = effective_allowed_hosts(&settings);
    assert!(check_host_allowed("https://gitlab.internal.test/a/b.git", &allowed).is_ok());
    assert!(check_host_allowed("https://codeberg.org/a/b.git", &allowed).is_ok());
    // An entry that repeats a built-in (in any case) doesn't duplicate it.
    assert_eq!(allowed.iter().filter(|h| *h == "github.com").count(), 1);
}

#[test]
fn an_exact_host_matches_regardless_of_port_or_case() {
    let allowed = vec!["github.com".to_string()];
    assert!(check_host_allowed("https://github.com/a/b.git", &allowed).is_ok());
    assert!(check_host_allowed("https://GitHub.com/a/b.git", &allowed).is_ok());
    assert!(check_host_allowed("https://github.com:8443/a/b.git", &allowed).is_ok());
    // A host that merely ends with it isn't it.
    assert!(check_host_allowed("https://evil-github.com/a/b.git", &allowed).is_err());
    assert!(check_host_allowed("https://github.com.evil.test/a/b.git", &allowed).is_err());
}

#[test]
fn a_wildcard_covers_subdomains_but_not_the_zone_itself() {
    let allowed = vec!["*.example.com".to_string()];
    assert!(check_host_allowed("https://git.example.com/a/b.git", &allowed).is_ok());
    assert!(check_host_allowed("https://deep.git.example.com/a/b.git", &allowed).is_ok());
    // The wildcard says "a delegated subdomain", not the zone apex — an
    // operator who means both writes both.
    assert!(check_host_allowed("https://example.com/a/b.git", &allowed).is_err());
    assert!(check_host_allowed("https://notexample.com/a/b.git", &allowed).is_err());
}

#[test]
fn a_plain_local_path_isnt_subject_to_the_allowlist() {
    // No `scheme://` at all: `validate_mirror_url` already refuses to store
    // one, and it names a path rather than any egress.
    assert!(check_host_allowed("/srv/git/repo.git", &[]).is_ok());
}

#[test]
fn a_mirror_id_is_exactly_what_generate_mirror_id_mints() {
    assert!(validate_mirror_id(&gitquarry_mirror_external::access::generate_mirror_id()).is_ok());
    for bad in [
        "",
        "0123456789abcde",
        "0123456789ABCDEF",
        "0123456789abcdef0",
        "../../0123456789",
        "0123456789abcde/",
    ] {
        assert!(validate_mirror_id(bad).is_err(), "{bad:?}");
    }
}
