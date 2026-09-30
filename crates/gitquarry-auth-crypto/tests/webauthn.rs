//! `webauthn` against a software authenticator built here from the spec:
//! real P-256 / Ed25519 keys, real CBOR, real signatures — so every
//! rejection below is a byte a genuine browser or authenticator could send.

use ciborium::value::Value;
use gitquarry_auth_crypto::webauthn::*;
use sha2::{Digest, Sha256};

const ORIGIN: &str = "https://git.example.com";

fn rp() -> RelyingParty {
    RelyingParty::from_public_url("https://git.example.com/").unwrap()
}

const UP: u8 = 0x01;
const UV: u8 = 0x04;
const AT: u8 = 0x40;

enum Key {
    P256(p256::ecdsa::SigningKey),
    Ed25519(ed25519_dalek::SigningKey),
}

impl Key {
    fn p256(seed: u8) -> Key {
        Key::P256(p256::ecdsa::SigningKey::from_slice(&[seed; 32]).unwrap())
    }

    fn ed25519(seed: u8) -> Key {
        Key::Ed25519(ed25519_dalek::SigningKey::from_bytes(&[seed; 32]))
    }

    fn cose(&self) -> Vec<u8> {
        let int = |i: i64| Value::Integer(i.into());
        let map = match self {
            Key::P256(k) => {
                let point = k.verifying_key().to_sec1_point(false);
                let bytes = point.as_bytes();
                vec![
                    (int(1), int(2)),
                    (int(3), int(-7)),
                    (int(-1), int(1)),
                    (int(-2), Value::Bytes(bytes[1..33].to_vec())),
                    (int(-3), Value::Bytes(bytes[33..65].to_vec())),
                ]
            }
            Key::Ed25519(k) => vec![
                (int(1), int(1)),
                (int(3), int(-8)),
                (int(-1), int(6)),
                (int(-2), Value::Bytes(k.verifying_key().to_bytes().to_vec())),
            ],
        };
        let mut out = Vec::new();
        ciborium::ser::into_writer(&Value::Map(map), &mut out).unwrap();
        out
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        match self {
            Key::P256(k) => {
                use p256::ecdsa::signature::Signer;
                let sig: p256::ecdsa::Signature = k.sign(message);
                sig.to_der().as_bytes().to_vec()
            }
            Key::Ed25519(k) => {
                use ed25519_dalek::Signer;
                k.sign(message).to_bytes().to_vec()
            }
        }
    }
}

fn client_data(ceremony: &str, challenge: &str, origin: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "type": ceremony, "challenge": challenge, "origin": origin, "crossOrigin": false,
    }))
    .unwrap()
}

fn auth_data(rp_id: &str, flags: u8, counter: u32, attested: Option<(&[u8], &[u8])>) -> Vec<u8> {
    let mut out = Sha256::digest(rp_id.as_bytes()).to_vec();
    out.push(flags);
    out.extend_from_slice(&counter.to_be_bytes());
    if let Some((id, cose)) = attested {
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(&u16::try_from(id.len()).unwrap().to_be_bytes());
        out.extend_from_slice(id);
        out.extend_from_slice(cose);
    }
    out
}

fn attestation_object(auth: Vec<u8>) -> Vec<u8> {
    let object = Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text("none".into())),
        (Value::Text("attStmt".into()), Value::Map(vec![])),
        (Value::Text("authData".into()), Value::Bytes(auth)),
    ]);
    let mut out = Vec::new();
    ciborium::ser::into_writer(&object, &mut out).unwrap();
    out
}

fn register(key: &Key, challenge: &str, flags: u8) -> Result<NewCredential, WebauthnError> {
    let auth = auth_data("git.example.com", flags | AT, 0, Some((b"cred-1", &key.cose())));
    verify_registration(
        &rp(),
        challenge,
        &client_data("webauthn.create", challenge, ORIGIN),
        &attestation_object(auth),
        true,
    )
}

/// One `get()` response, signed by `key`, with every field overridable.
struct Assert<'a> {
    key: &'a Key,
    ceremony: &'a str,
    challenge: &'a str,
    origin: &'a str,
    rp_id: &'a str,
    flags: u8,
    counter: u32,
}

impl<'a> Assert<'a> {
    fn new(key: &'a Key, challenge: &'a str) -> Self {
        Assert {
            key,
            ceremony: "webauthn.get",
            challenge,
            origin: ORIGIN,
            rp_id: "git.example.com",
            flags: UP | UV,
            counter: 0,
        }
    }

    fn parts(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let client = client_data(self.ceremony, self.challenge, self.origin);
        let auth = auth_data(self.rp_id, self.flags, self.counter, None);
        let mut signed = auth.clone();
        signed.extend_from_slice(&Sha256::digest(&client));
        (client, auth, self.key.sign(&signed))
    }

    fn verify(
        &self,
        stored: &NewCredential,
        expected: &str,
        stored_count: u32,
        require_uv: bool,
    ) -> Result<VerifiedAssertion, WebauthnError> {
        let (client, auth, sig) = self.parts();
        verify_assertion(
            &rp(),
            expected,
            &stored.public_key,
            stored_count,
            &client,
            &auth,
            &sig,
            require_uv,
        )
    }
}

#[test]
fn the_relying_party_comes_from_the_public_url() {
    let rp = RelyingParty::from_public_url("https://git.example.com:8443/forge/").unwrap();
    assert_eq!(rp.id, "git.example.com");
    assert_eq!(rp.origin, "https://git.example.com:8443");
    let local = RelyingParty::from_public_url("http://localhost:8080").unwrap();
    assert_eq!(
        (local.id.as_str(), local.origin.as_str()),
        ("localhost", "http://localhost:8080")
    );

    for refused in [
        "http://git.example.com",
        "https://203.0.113.9",
        "https://user:pw@git.example.com",
        "ftp://git.example.com",
        "not a url",
    ] {
        assert!(RelyingParty::from_public_url(refused).is_err(), "{refused} accepted");
    }
}

#[test]
fn challenges_are_fresh_and_32_bytes() {
    let (a, b) = (new_challenge(), new_challenge());
    assert_ne!(a, b);
    assert_eq!(decode_b64url(&a).unwrap().len(), 32);
}

#[test]
fn both_key_types_register_and_then_sign_in() {
    for key in [Key::p256(1), Key::ed25519(2)] {
        let challenge = new_challenge();
        let cred = register(&key, &challenge, UP | UV).unwrap();
        assert_eq!(cred.credential_id, b"cred-1");
        assert!(cred.user_verified);
        assert_eq!(cred.algorithm, if matches!(key, Key::P256(_)) { ES256 } else { EDDSA });

        let login = new_challenge();
        let ok = Assert::new(&key, &login).verify(&cred, &login, 0, true).unwrap();
        assert!(ok.user_verified);
    }
}

#[test]
fn registration_refuses_a_response_made_for_something_else() {
    let key = Key::p256(1);
    let challenge = new_challenge();
    let cose = key.cose();
    let attested = |rp_id: &str, flags: u8| attestation_object(auth_data(rp_id, flags | AT, 0, Some((b"id", &cose))));
    let ok_auth = attested("git.example.com", UP | UV);

    let cases: [(Vec<u8>, Vec<u8>, bool, WebauthnError); 6] = [
        (
            client_data("webauthn.get", &challenge, ORIGIN),
            ok_auth.clone(),
            true,
            WebauthnError::WrongCeremony,
        ),
        (
            client_data("webauthn.create", &new_challenge(), ORIGIN),
            ok_auth.clone(),
            true,
            WebauthnError::ChallengeMismatch,
        ),
        (
            client_data("webauthn.create", &challenge, "https://evil.example"),
            ok_auth.clone(),
            true,
            WebauthnError::OriginMismatch,
        ),
        (
            client_data("webauthn.create", &challenge, ORIGIN),
            attested("evil.example", UP | UV),
            true,
            WebauthnError::RpIdMismatch,
        ),
        (
            client_data("webauthn.create", &challenge, ORIGIN),
            attested("git.example.com", UV),
            true,
            WebauthnError::UserNotPresent,
        ),
        (
            client_data("webauthn.create", &challenge, ORIGIN),
            attested("git.example.com", UP),
            true,
            WebauthnError::UserNotVerified,
        ),
    ];
    for (client, attestation, require_uv, expected) in cases {
        assert_eq!(
            verify_registration(&rp(), &challenge, &client, &attestation, require_uv),
            Err(expected)
        );
    }
    // Presence alone is enough when the caller doesn't need verification.
    let client = client_data("webauthn.create", &challenge, ORIGIN);
    assert!(verify_registration(&rp(), &challenge, &client, &attested("git.example.com", UP), false).is_ok());
}

#[test]
fn a_cross_origin_iframe_can_not_register() {
    let key = Key::p256(1);
    let challenge = new_challenge();
    let client = serde_json::to_vec(&serde_json::json!({
        "type": "webauthn.create", "challenge": challenge, "origin": ORIGIN, "crossOrigin": true,
    }))
    .unwrap();
    let auth = attestation_object(auth_data(
        "git.example.com",
        UP | UV | AT,
        0,
        Some((b"id", &key.cose())),
    ));
    assert_eq!(
        verify_registration(&rp(), &challenge, &client, &auth, true),
        Err(WebauthnError::OriginMismatch)
    );
}

#[test]
fn registration_refuses_unsupported_keys_and_garbage() {
    let challenge = new_challenge();
    let client = client_data("webauthn.create", &challenge, ORIGIN);
    let int = |i: i64| Value::Integer(i.into());
    let mut rs256 = Vec::new();
    ciborium::ser::into_writer(&Value::Map(vec![(int(1), int(3)), (int(3), int(-257))]), &mut rs256).unwrap();
    let auth = attestation_object(auth_data("git.example.com", UP | UV | AT, 0, Some((b"id", &rs256))));
    assert_eq!(
        verify_registration(&rp(), &challenge, &client, &auth, true),
        Err(WebauthnError::UnsupportedAlgorithm)
    );

    for garbage in [&b""[..], b"\xa0", b"not cbor at all"] {
        assert!(matches!(
            verify_registration(&rp(), &challenge, &client, garbage, true),
            Err(WebauthnError::Malformed(_))
        ));
    }
    // An attested credential whose key is followed by bytes it didn't flag.
    let mut trailing = auth_data("git.example.com", UP | UV | AT, 0, Some((b"id", &Key::p256(1).cose())));
    trailing.push(0);
    assert!(matches!(
        verify_registration(&rp(), &challenge, &client, &attestation_object(trailing), true),
        Err(WebauthnError::Malformed(_))
    ));
}

#[test]
fn sign_in_refuses_a_response_made_for_something_else() {
    let key = Key::p256(1);
    let cred = register(&key, &new_challenge(), UP | UV).unwrap();
    let challenge = new_challenge();
    let other = new_challenge();

    let mut a = Assert::new(&key, &challenge);
    a.ceremony = "webauthn.create";
    assert_eq!(a.verify(&cred, &challenge, 0, true), Err(WebauthnError::WrongCeremony));

    let a = Assert::new(&key, &other);
    assert_eq!(
        a.verify(&cred, &challenge, 0, true),
        Err(WebauthnError::ChallengeMismatch)
    );

    let mut a = Assert::new(&key, &challenge);
    a.origin = "https://git.example.com.evil.example";
    assert_eq!(a.verify(&cred, &challenge, 0, true), Err(WebauthnError::OriginMismatch));

    let mut a = Assert::new(&key, &challenge);
    a.rp_id = "example.com";
    assert_eq!(a.verify(&cred, &challenge, 0, true), Err(WebauthnError::RpIdMismatch));

    let mut a = Assert::new(&key, &challenge);
    a.flags = UP;
    assert_eq!(
        a.verify(&cred, &challenge, 0, true),
        Err(WebauthnError::UserNotVerified)
    );
    // As a second factor, presence is enough.
    assert!(a.verify(&cred, &challenge, 0, false).is_ok());
}

#[test]
fn a_signature_by_another_key_or_over_other_data_is_refused() {
    let key = Key::p256(1);
    let challenge = new_challenge();
    let cred = register(&key, &challenge, UP | UV).unwrap();
    let login = new_challenge();

    let impostor = Key::p256(9);
    assert_eq!(
        Assert::new(&impostor, &login).verify(&cred, &login, 0, true),
        Err(WebauthnError::BadSignature)
    );

    let (client, mut auth, sig) = Assert::new(&key, &login).parts();
    auth[36] ^= 1; // the counter byte, after signing
    assert_eq!(
        verify_assertion(&rp(), &login, &cred.public_key, 0, &client, &auth, &sig, true),
        Err(WebauthnError::BadSignature)
    );

    let (client, auth, mut sig) = Assert::new(&key, &login).parts();
    let last = sig.len() - 1;
    sig[last] ^= 1;
    assert_eq!(
        verify_assertion(&rp(), &login, &cred.public_key, 0, &client, &auth, &sig, true),
        Err(WebauthnError::BadSignature)
    );

    let ed = Key::ed25519(2);
    let c = new_challenge();
    let ed_cred = register(&ed, &c, UP | UV).unwrap();
    let (client, mut auth, sig) = Assert::new(&ed, &login).parts();
    auth[33] ^= 1;
    assert_eq!(
        verify_assertion(&rp(), &login, &ed_cred.public_key, 0, &client, &auth, &sig, true),
        Err(WebauthnError::BadSignature)
    );
}

#[test]
fn a_counting_authenticator_must_move_forward() {
    let key = Key::p256(1);
    let challenge = new_challenge();
    let cred = register(&key, &challenge, UP | UV).unwrap();
    let login = new_challenge();

    let mut a = Assert::new(&key, &login);
    a.counter = 5;
    assert_eq!(a.verify(&cred, &login, 4, true).unwrap().sign_count, 5);
    // A repeat or a step back is what a cloned authenticator looks like.
    assert_eq!(a.verify(&cred, &login, 5, true), Err(WebauthnError::CounterRegressed));
    assert_eq!(a.verify(&cred, &login, 9, true), Err(WebauthnError::CounterRegressed));
    // One that stopped counting after it had counted is refused too.
    a.counter = 0;
    assert_eq!(a.verify(&cred, &login, 5, true), Err(WebauthnError::CounterRegressed));
    // One that never counts (most synced passkeys) is fine at 0 every time.
    assert!(a.verify(&cred, &login, 0, true).is_ok());
}

#[test]
fn credential_ids_round_trip_through_base64url() {
    let id = [0u8, 255, 17, 42];
    assert_eq!(decode_b64url(&encode_id(&id)).unwrap(), id);
    assert!(decode_b64url("not*base64").is_err());
}
