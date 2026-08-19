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
