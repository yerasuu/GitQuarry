//! A software WebAuthn authenticator, built from the spec: real P-256 and
//! Ed25519 keys, real CBOR, real signatures. Every piece is exposed on its
//! own so a test can hand-build exactly the malformed or forged response it
//! is about.

use ciborium::value::Value;
use sha2::{Digest, Sha256};

pub const UP: u8 = 0x01;
pub const UV: u8 = 0x04;
pub const AT: u8 = 0x40;

/// A key pair derived from a fixed seed, so a test is reproducible.
pub enum Key {
    P256(p256::ecdsa::SigningKey),
    Ed25519(ed25519_dalek::SigningKey),
}

impl Key {
    pub fn p256(seed: u8) -> Key {
        Key::P256(p256::ecdsa::SigningKey::from_slice(&[seed; 32]).expect("a valid P-256 scalar"))
    }

    pub fn ed25519(seed: u8) -> Key {
        Key::Ed25519(ed25519_dalek::SigningKey::from_bytes(&[seed; 32]))
    }

    /// The public key as the COSE_Key an authenticator reports.
    pub fn cose(&self) -> Vec<u8> {
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
        cbor(&Value::Map(map))
    }

    /// The signature WebAuthn carries: DER for ES256, raw for EdDSA.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
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

pub fn cbor(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out).expect("encoding CBOR into a Vec");
    out
}

pub fn client_data(ceremony: &str, challenge: &str, origin: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "type": ceremony, "challenge": challenge, "origin": origin, "crossOrigin": false,
    }))
    .expect("serializing clientDataJSON")
}

/// `authenticatorData`; `attested` is `(credential_id, cose_key)` for a
/// registration.
pub fn auth_data(rp_id: &str, flags: u8, counter: u32, attested: Option<(&[u8], &[u8])>) -> Vec<u8> {
    let mut out = Sha256::digest(rp_id.as_bytes()).to_vec();
    out.push(flags);
    out.extend_from_slice(&counter.to_be_bytes());
    if let Some((id, cose)) = attested {
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(
            &u16::try_from(id.len())
                .expect("credential id under 64 KiB")
                .to_be_bytes(),
        );
        out.extend_from_slice(id);
        out.extend_from_slice(cose);
    }
    out
}

/// A `"none"`-format attestation object around `auth_data`.
pub fn attestation_object(auth_data: Vec<u8>) -> Vec<u8> {
    cbor(&Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text("none".into())),
        (Value::Text("attStmt".into()), Value::Map(vec![])),
        (Value::Text("authData".into()), Value::Bytes(auth_data)),
    ]))
}

/// A whole authenticator: one key, one credential id, one relying party.
pub struct Authenticator {
    pub key: Key,
    pub credential_id: Vec<u8>,
    pub rp_id: String,
    pub origin: String,
}

/// What `navigator.credentials.create()` hands back.
pub struct Registration {
    pub client_data_json: Vec<u8>,
    pub attestation_object: Vec<u8>,
}

/// What `navigator.credentials.get()` hands back.
pub struct AssertionParts {
    pub client_data_json: Vec<u8>,
    pub authenticator_data: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Authenticator {
    pub fn new(key: Key, credential_id: &[u8], rp: &gitquarry_auth_crypto::webauthn::RelyingParty) -> Self {
        Authenticator {
            key,
            credential_id: credential_id.to_vec(),
            rp_id: rp.id.clone(),
            origin: rp.origin.clone(),
        }
    }

    pub fn register(&self, challenge: &str, flags: u8) -> Registration {
        let cose = self.key.cose();
        let auth = auth_data(&self.rp_id, flags | AT, 0, Some((&self.credential_id, &cose)));
        Registration {
            client_data_json: client_data("webauthn.create", challenge, &self.origin),
            attestation_object: attestation_object(auth),
        }
    }

    pub fn assert(&self, challenge: &str, flags: u8, counter: u32) -> AssertionParts {
        let client_data_json = client_data("webauthn.get", challenge, &self.origin);
        let authenticator_data = auth_data(&self.rp_id, flags, counter, None);
        let mut signed = authenticator_data.clone();
        signed.extend_from_slice(&Sha256::digest(&client_data_json));
        AssertionParts {
            signature: self.key.sign(&signed),
            client_data_json,
            authenticator_data,
        }
    }
}
