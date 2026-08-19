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
    // `engine::credentialed_url` splices the stored token in as userinfo at
    // sync time; a URL that already carries one would decide for itself
    // which host ends up receiving that token.
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
