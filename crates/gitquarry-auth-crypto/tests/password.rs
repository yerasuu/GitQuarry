use gitquarry_auth_crypto::password::*;

#[test]
fn password_roundtrips() {
    let hash = hash_password("correct horse battery staple").unwrap();
    assert!(verify_password("correct horse battery staple", &hash));
    assert!(!verify_password("wrong password", &hash));
}

#[test]
fn password_hash_rejects_garbage() {
    assert!(!verify_password("anything", "not a phc string"));
}

#[test]
fn a_new_hash_carries_the_raised_cost() {
    let hash = hash_password("correct horse battery staple").unwrap();
    assert!(hash.starts_with("$argon2id$"), "{hash}");
    assert!(hash.contains("m=65536,t=3,p=1"), "{hash}");
    assert!(verify_password("correct horse battery staple", &hash));
}

#[test]
fn a_hash_written_at_the_old_cost_still_verifies() {
    // Produced by `Argon2::default()` (m=19456,t=2,p=1) — every account
    // stored before the cost went up looks like this, and must keep
    // working.
    let legacy = {
        use argon2::{PasswordHash, PasswordHasher};
        let hash: PasswordHash = argon2::Argon2::default().hash_password(b"hunter2").unwrap();
        hash.to_string()
    };
    assert!(legacy.contains("m=19456,t=2,p=1"), "{legacy}");
    assert!(verify_password("hunter2", &legacy));
    assert!(!verify_password("wrong", &legacy));
}

#[test]
fn a_hash_stored_by_argon2_0_5_still_verifies() {
    // Written by `hash_password` while the crate was on `argon2` 0.5.3: the
    // 0.6 upgrade rewrote the PHC handling, and every stored account hash
    // predates it.
    let stored = "$argon2id$v=19$m=65536,t=3,p=1$VclFGHiQewA/zqnbnJaXdQ$Okt54dKBqe3H76JQ7NCqg3p4N7pbzbSMgfcHe17+lBg";
    assert!(verify_password("hunter2", stored));
    assert!(!verify_password("wrong", stored));
}
