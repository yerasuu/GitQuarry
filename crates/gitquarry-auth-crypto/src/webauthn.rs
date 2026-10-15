//! Server-side WebAuthn (passkey) verification — the relying-party half of
//! W3C Web Authentication Level 2, reduced to what a self-hosted server
//! needs: registering a credential and checking an assertion made with it.
//!
//! Deliberately not `webauthn-rs`: that crate does its cryptography through
//! openssl, and this binary is rustls-only. What's left once attestation is
//! out of the picture is small and fully specified — CBOR decoding (through
//! `ciborium`), two signature algorithms (`p256` and `ed25519-dalek`, both
//! already in the tree) and a handful of byte-exact comparisons.
//!
//! Attestation is not verified (equivalent to requesting `"none"`): there is
//! no authenticator vendor allow-list to check a certificate chain against,
//! so its statement is parsed past and ignored. What registration proves is
//! only that this browser, on this origin, answered this challenge with a
//! fresh key pair — which is all login needs.

use ciborium::value::Value;
use data_encoding::BASE64URL_NOPAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Cursor;

/// COSE algorithm identifiers (IANA "COSE Algorithms") this verifier
/// accepts — the two every current platform authenticator offers.
pub const ES256: i64 = -7;
pub const EDDSA: i64 = -8;

/// WebAuthn §5.8.3's upper bound; anything longer is not a real credential.
const MAX_CREDENTIAL_ID_LEN: usize = 1023;

const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_BACKUP_ELIGIBLE: u8 = 0x08;
const FLAG_ATTESTED_CREDENTIAL: u8 = 0x40;
const FLAG_EXTENSIONS: u8 = 0x80;

/// Why a ceremony was refused. Each variant is a distinct way a response
/// can fail to prove what it claims, kept apart so a log line (and a test)
/// can say which check tripped.
#[derive(Debug, PartialEq, Eq)]
pub enum WebauthnError {
    /// Not well-formed: bad JSON/CBOR, truncated data, wrong field types.
    Malformed(&'static str),
    /// `clientDataJSON.type` isn't the ceremony being verified.
    WrongCeremony,
    ChallengeMismatch,
    /// Signed for a different web origin, or from a cross-origin iframe.
    OriginMismatch,
    /// The authenticator scoped the credential to a different relying party.
    RpIdMismatch,
    UserNotPresent,
    UserNotVerified,
    UnsupportedAlgorithm,
    BadSignature,
    /// The authenticator's signature counter went backwards or stood still
    /// while claiming to count — the signal WebAuthn gives for a cloned key.
    CounterRegressed,
}

impl std::fmt::Display for WebauthnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebauthnError::Malformed(what) => write!(f, "malformed WebAuthn response: {what}"),
            WebauthnError::WrongCeremony => f.write_str("WebAuthn response is for a different ceremony"),
            WebauthnError::ChallengeMismatch => f.write_str("WebAuthn challenge doesn't match"),
            WebauthnError::OriginMismatch => f.write_str("WebAuthn response comes from another origin"),
            WebauthnError::RpIdMismatch => f.write_str("WebAuthn credential belongs to another site"),
            WebauthnError::UserNotPresent => f.write_str("the authenticator didn't confirm user presence"),
            WebauthnError::UserNotVerified => f.write_str("the authenticator didn't verify the user"),
            WebauthnError::UnsupportedAlgorithm => f.write_str("unsupported passkey algorithm"),
            WebauthnError::BadSignature => f.write_str("WebAuthn signature doesn't verify"),
            WebauthnError::CounterRegressed => f.write_str("passkey signature counter went backwards"),
        }
    }
}

impl std::error::Error for WebauthnError {}

/// This server as a WebAuthn relying party: `id` is what a credential is
/// scoped to (a registrable domain), `origin` what the browser reports it
/// signed for. Both derive from the one configured public URL, so they can
/// never disagree with each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelyingParty {
    pub id: String,
    pub origin: String,
}

impl RelyingParty {
    /// From the instance's public URL. WebAuthn only runs in a secure
    /// context, so this is `https://`, or plain `http://` on `localhost` /
    /// `127.0.0.1` — the one exception browsers make. An IP address other
    /// than loopback can't be an rpId at all.
    pub fn from_public_url(public_url: &str) -> Result<RelyingParty, WebauthnError> {
        let url = url::Url::parse(public_url.trim()).map_err(|_| WebauthnError::Malformed("public URL"))?;
        let host = url
            .host_str()
            .ok_or(WebauthnError::Malformed("public URL has no host"))?;
        let loopback = matches!(host, "localhost" | "127.0.0.1");
        match url.scheme() {
            "https" => {}
            "http" if loopback => {}
            _ => {
                return Err(WebauthnError::Malformed(
                    "public URL must be https (or http on localhost)",
                ));
            }
        }
        if !loopback && matches!(url.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_))) {
            return Err(WebauthnError::Malformed(
                "public URL must name a domain, not an IP address",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(WebauthnError::Malformed("public URL must not carry credentials"));
        }
        Ok(RelyingParty {
            id: host.to_string(),
            origin: url.origin().ascii_serialization(),
        })
    }
}

/// A fresh challenge, base64url without padding — the form the browser
/// echoes back inside `clientDataJSON`. 32 bytes, WebAuthn's recommended
/// minimum is 16.
pub fn new_challenge() -> String {
    let raw: [u8; 32] = rand::random();
    BASE64URL_NOPAD.encode(&raw)
}

/// What a successful registration yields, to store against the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCredential {
    pub credential_id: Vec<u8>,
    /// The COSE_Key exactly as the authenticator sent it; re-parsed on
    /// every assertion rather than stored in some intermediate shape.
    pub public_key: Vec<u8>,
    pub algorithm: i64,
    pub sign_count: u32,
    pub user_verified: bool,
    /// Whether this is a synced (multi-device) passkey rather than one
    /// bound to a single hardware authenticator — shown to the user, never
    /// used to decide anything.
    pub backup_eligible: bool,
}

/// What a successful assertion yields: the counter to store next time.
#[derive(Debug, PartialEq)]
pub struct VerifiedAssertion {
    pub sign_count: u32,
    pub user_verified: bool,
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    ceremony: String,
    challenge: String,
    origin: String,
    #[serde(default, rename = "crossOrigin")]
    cross_origin: bool,
}

fn check_client_data(
    rp: &RelyingParty,
    ceremony: &str,
    expected_challenge: &str,
    client_data_json: &[u8],
) -> Result<(), WebauthnError> {
    let data: ClientData =
        serde_json::from_slice(client_data_json).map_err(|_| WebauthnError::Malformed("clientDataJSON"))?;
    if data.ceremony != ceremony {
        return Err(WebauthnError::WrongCeremony);
    }
    // Compared as the base64url strings both sides hold, in constant time:
    // it's the one value here an attacker is trying to guess.
    if !constant_time_eq(data.challenge.as_bytes(), expected_challenge.as_bytes()) {
        return Err(WebauthnError::ChallengeMismatch);
    }
    if data.origin != rp.origin || data.cross_origin {
        return Err(WebauthnError::OriginMismatch);
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `authenticatorData`'s fixed 37-byte head, plus whatever follows it.
struct AuthData<'a> {
    rp_id_hash: &'a [u8],
    flags: u8,
    sign_count: u32,
    rest: &'a [u8],
}

fn parse_auth_data(bytes: &[u8]) -> Result<AuthData<'_>, WebauthnError> {
    if bytes.len() < 37 {
        return Err(WebauthnError::Malformed("authenticatorData too short"));
    }
    Ok(AuthData {
        rp_id_hash: &bytes[..32],
        flags: bytes[32],
        sign_count: u32::from_be_bytes([bytes[33], bytes[34], bytes[35], bytes[36]]),
        rest: &bytes[37..],
    })
}

fn check_rp_and_flags(rp: &RelyingParty, auth: &AuthData<'_>, require_uv: bool) -> Result<(), WebauthnError> {
    if auth.rp_id_hash != Sha256::digest(rp.id.as_bytes()).as_slice() {
        return Err(WebauthnError::RpIdMismatch);
    }
    if auth.flags & FLAG_USER_PRESENT == 0 {
        return Err(WebauthnError::UserNotPresent);
    }
    if require_uv && auth.flags & FLAG_USER_VERIFIED == 0 {
        return Err(WebauthnError::UserNotVerified);
    }
    Ok(())
}

fn map_get<'a>(map: &'a [(Value, Value)], key: &Value) -> Option<&'a Value> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn int(value: &Value) -> Option<i128> {
    value.as_integer().map(i128::from)
}

/// The parts of a COSE_Key this verifier needs, validated against the two
/// supported key types.
enum PublicKey {
    P256(p256::ecdsa::VerifyingKey),
    Ed25519(ed25519_dalek::VerifyingKey),
}

fn parse_cose_key(value: &Value) -> Result<(PublicKey, i64), WebauthnError> {
    let map = value
        .as_map()
        .ok_or(WebauthnError::Malformed("COSE key is not a map"))?;
    let field = |label: i64| map_get(map, &Value::Integer(label.into()));
    let bytes = |label: i64| {
        field(label)
            .and_then(Value::as_bytes)
            .ok_or(WebauthnError::Malformed("COSE key coordinate"))
    };
    let kty = field(1).and_then(int);
    let alg = field(3).and_then(int);
    let crv = field(-1).and_then(int);
    match (kty, alg, crv) {
        // EC2, ES256, P-256
        (Some(2), Some(-7), Some(1)) => {
            let (x, y) = (bytes(-2)?, bytes(-3)?);
            if x.len() != 32 || y.len() != 32 {
                return Err(WebauthnError::Malformed("P-256 coordinate length"));
            }
            let mut sec1 = Vec::with_capacity(65);
            sec1.push(0x04);
            sec1.extend_from_slice(x);
            sec1.extend_from_slice(y);
            let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(&sec1)
                .map_err(|_| WebauthnError::Malformed("P-256 point not on curve"))?;
            Ok((PublicKey::P256(key), ES256))
        }
        // OKP, EdDSA, Ed25519
        (Some(1), Some(-8), Some(6)) => {
            let x: [u8; 32] = bytes(-2)?
                .as_slice()
                .try_into()
                .map_err(|_| WebauthnError::Malformed("Ed25519 key length"))?;
            let key =
                ed25519_dalek::VerifyingKey::from_bytes(&x).map_err(|_| WebauthnError::Malformed("Ed25519 key"))?;
            Ok((PublicKey::Ed25519(key), EDDSA))
        }
        _ => Err(WebauthnError::UnsupportedAlgorithm),
    }
}

fn decode_cbor(bytes: &[u8]) -> Result<Value, WebauthnError> {
    ciborium::de::from_reader(bytes).map_err(|_| WebauthnError::Malformed("CBOR"))
}

/// Checks a `navigator.credentials.create()` response for `expected_challenge`
/// and returns the credential to store.
///
/// `require_uv` is whether the credential must have verified the user (PIN,
/// biometrics) at creation — `true` for one meant to sign in on its own.
///
/// # Errors
///
/// Any [`WebauthnError`]; nothing is to be stored on `Err`.
pub fn verify_registration(
    rp: &RelyingParty,
    expected_challenge: &str,
    client_data_json: &[u8],
    attestation_object: &[u8],
    require_uv: bool,
) -> Result<NewCredential, WebauthnError> {
    check_client_data(rp, "webauthn.create", expected_challenge, client_data_json)?;

    let attestation = decode_cbor(attestation_object)?;
    let attestation = attestation
        .as_map()
        .ok_or(WebauthnError::Malformed("attestationObject is not a map"))?;
    let auth_bytes = map_get(attestation, &Value::Text("authData".into()))
        .and_then(Value::as_bytes)
        .ok_or(WebauthnError::Malformed("attestationObject.authData"))?;

    let auth = parse_auth_data(auth_bytes)?;
    check_rp_and_flags(rp, &auth, require_uv)?;
    if auth.flags & FLAG_ATTESTED_CREDENTIAL == 0 {
        return Err(WebauthnError::Malformed("no attested credential data"));
    }
    // aaguid(16) | credentialIdLength(2) | credentialId | credentialPublicKey
    let rest = auth.rest;
    if rest.len() < 18 {
        return Err(WebauthnError::Malformed("attested credential data too short"));
    }
    let id_len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
    if id_len == 0 || id_len > MAX_CREDENTIAL_ID_LEN || rest.len() < 18 + id_len {
        return Err(WebauthnError::Malformed("credential id length"));
    }
    let credential_id = rest[18..18 + id_len].to_vec();
    let key_bytes = &rest[18 + id_len..];
    let mut reader = Cursor::new(key_bytes);
    let key_value: Value =
        ciborium::de::from_reader(&mut reader).map_err(|_| WebauthnError::Malformed("credential public key"))?;
    let key_len = usize::try_from(reader.position()).map_err(|_| WebauthnError::Malformed("key length"))?;
    // Only extension outputs may follow the key, and only when flagged.
    if key_len != key_bytes.len() && auth.flags & FLAG_EXTENSIONS == 0 {
        return Err(WebauthnError::Malformed("trailing bytes after the public key"));
    }
    let (_, algorithm) = parse_cose_key(&key_value)?;

    Ok(NewCredential {
        credential_id,
        public_key: key_bytes[..key_len].to_vec(),
        algorithm,
        sign_count: auth.sign_count,
        user_verified: auth.flags & FLAG_USER_VERIFIED != 0,
        backup_eligible: auth.flags & FLAG_BACKUP_ELIGIBLE != 0,
    })
}

/// Checks a `navigator.credentials.get()` response against a credential
/// stored by [`verify_registration`].
///
/// `stored_sign_count` is the counter recorded after the credential's last
/// use; the returned one replaces it. `require_uv`: `true` when the passkey
/// alone signs the user in, `false` when it's the second factor after a
/// password.
///
/// # Errors
///
/// Any [`WebauthnError`]; the user must not be signed in on `Err`.
#[allow(clippy::too_many_arguments)] // each is a distinct field of the ceremony
pub fn verify_assertion(
    rp: &RelyingParty,
    expected_challenge: &str,
    stored_public_key: &[u8],
    stored_sign_count: u32,
    client_data_json: &[u8],
    authenticator_data: &[u8],
    signature: &[u8],
    require_uv: bool,
) -> Result<VerifiedAssertion, WebauthnError> {
    check_client_data(rp, "webauthn.get", expected_challenge, client_data_json)?;
    let auth = parse_auth_data(authenticator_data)?;
    check_rp_and_flags(rp, &auth, require_uv)?;

    let (key, _) = parse_cose_key(&decode_cbor(stored_public_key)?)?;
    let mut signed = Vec::with_capacity(authenticator_data.len() + 32);
    signed.extend_from_slice(authenticator_data);
    signed.extend_from_slice(&Sha256::digest(client_data_json));
    match key {
        PublicKey::P256(key) => {
            use p256::ecdsa::signature::Verifier;
            let sig = p256::ecdsa::Signature::from_der(signature).map_err(|_| WebauthnError::BadSignature)?;
            key.verify(&signed, &sig).map_err(|_| WebauthnError::BadSignature)?;
        }
        PublicKey::Ed25519(key) => {
            let sig = ed25519_dalek::Signature::from_slice(signature).map_err(|_| WebauthnError::BadSignature)?;
            key.verify_strict(&signed, &sig)
                .map_err(|_| WebauthnError::BadSignature)?;
        }
    }

    // Authenticators that don't count report 0 every time (most synced
    // passkeys do); only one that counts can reveal a clone by repeating.
    if (auth.sign_count != 0 || stored_sign_count != 0) && auth.sign_count <= stored_sign_count {
        return Err(WebauthnError::CounterRegressed);
    }
    Ok(VerifiedAssertion {
        sign_count: auth.sign_count,
        user_verified: auth.flags & FLAG_USER_VERIFIED != 0,
    })
}

/// A credential id as it travels in JSON (`allowCredentials`, the browser's
/// `rawId`) and as it's keyed in storage.
pub fn encode_id(credential_id: &[u8]) -> String {
    BASE64URL_NOPAD.encode(credential_id)
}

pub fn decode_b64url(value: &str) -> Result<Vec<u8>, WebauthnError> {
    BASE64URL_NOPAD
        .decode(value.trim_end_matches('=').as_bytes())
        .map_err(|_| WebauthnError::Malformed("base64url"))
}
