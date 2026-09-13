//! `status_says_good`'s reading of gpg's `--status-fd` output — the part
//! that can be tested without a real keyring.

use super::status_says_good;

#[test]
fn a_plain_good_signature_verifies() {
    let status = "[GNUPG:] NEWSIG\n\
                  [GNUPG:] GOODSIG DEADBEEF Alice <alice@example.com>\n\
                  [GNUPG:] VALIDSIG DEADBEEF 2026-08-20\n\
                  [GNUPG:] TRUST_UNDEFINED\n";
    assert!(status_says_good(status));
}

#[test]
fn an_expired_key_does_not_verify() {
    // gpg exits 0 for this one, which is exactly why the exit status isn't
    // what's read any more.
    let status = "[GNUPG:] NEWSIG\n\
                  [GNUPG:] EXPKEYSIG DEADBEEF Alice <alice@example.com>\n";
    assert!(!status_says_good(status));
}

#[test]
fn a_revoked_key_does_not_verify() {
    let status = "[GNUPG:] NEWSIG\n\
                  [GNUPG:] REVKEYSIG DEADBEEF Alice <alice@example.com>\n";
    assert!(!status_says_good(status));
}

#[test]
fn a_bad_or_unknown_signature_does_not_verify() {
    assert!(!status_says_good("[GNUPG:] BADSIG DEADBEEF Alice\n"));
    assert!(!status_says_good("[GNUPG:] ERRSIG DEADBEEF 1 8 00 1755000000 9\n"));
    assert!(!status_says_good("[GNUPG:] NO_PUBKEY DEADBEEF\n"));
}

#[test]
fn empty_or_unrelated_output_does_not_verify() {
    assert!(!status_says_good(""));
    assert!(!status_says_good("gpg: Signature made Wed 20 Aug 2026\n"));
    // A `GOODSIG` that isn't on a status line is just text gpg printed.
    assert!(!status_says_good("gpg: GOODSIG but not a status line\n"));
}
