use gitquarry_core::auth::entities::Permission;
use gitquarry_core::git::ssh::*;

#[test]
fn benign_disconnect_reason_recognizes_early_eof_and_the_other_hangup_kinds() {
    for kind in [
        std::io::ErrorKind::UnexpectedEof, // the literal "early eof" case
        std::io::ErrorKind::ConnectionReset,
        std::io::ErrorKind::ConnectionAborted,
        std::io::ErrorKind::BrokenPipe,
    ] {
        let err = anyhow::Error::from(russh::Error::IO(std::io::Error::new(kind, "boom")));
        assert!(
            benign_disconnect_reason(&err).is_some(),
            "{kind:?} should be treated as a hangup"
        );
    }
}

#[test]
fn benign_disconnect_reason_ignores_a_genuine_protocol_error() {
    let err = anyhow::Error::from(russh::Error::Disconnect);
    assert!(benign_disconnect_reason(&err).is_none());
}

#[test]
fn benign_disconnect_reason_ignores_an_unrelated_io_error_kind() {
    let err = anyhow::Error::from(russh::Error::IO(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "boom",
    )));
    assert!(benign_disconnect_reason(&err).is_none());
}

#[test]
fn benign_disconnect_reason_ignores_a_non_russh_error() {
    let err = anyhow::anyhow!("some other error");
    assert!(benign_disconnect_reason(&err).is_none());
}

#[test]
fn parses_quoted_path() {
    let (cmd, path) = parse_exec_command(b"git-upload-pack '/projectA'").unwrap();
    assert_eq!(cmd, "upload-pack");
    assert_eq!(path, "/projectA");
}

#[test]
fn parses_unquoted_path() {
    let (cmd, path) = parse_exec_command(b"git-receive-pack /projectA").unwrap();
    assert_eq!(cmd, "receive-pack");
    assert_eq!(path, "/projectA");
}

#[test]
fn unquotes_embedded_single_quote() {
    let (_, path) = parse_exec_command(b"git-upload-pack '/a'\\''b'").unwrap();
    assert_eq!(path, "/a'b");
}

#[test]
fn rejects_unknown_service() {
    assert!(parse_exec_command(b"git-upload-archive '/x'").is_err());
    assert!(parse_exec_command(b"just-a-word").is_err());
}

#[test]
fn required_permission_floors_upload_pack_at_reporter_and_receive_pack_at_developer() {
    assert_eq!(required_permission("upload-pack"), Permission::Reporter);
    assert_eq!(required_permission("receive-pack"), Permission::Developer);
}

#[test]
fn a_guest_tier_permission_is_below_upload_packs_own_floor() {
    // The gap this closes: `Guest` grants no code-read rights at all
    // (see `Permission`'s own doc comment), so it must fail
    // `required_permission("upload-pack")`'s floor exactly like it
    // already fails `git::http`'s equivalent check.
    assert!(Permission::Guest < required_permission("upload-pack"));
}
