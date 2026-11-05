//! The relying-party side of WebAuthn (W3C Web Authentication, Level 2), as
//! much as passkeys need (PRD §9.2): registering a credential, and checking
//! a login made with one.
//!
//! Gantry asks for no attestation (`"none"`): it is a personal tool and
//! doesn't restrict which authenticators people use, so an attestation
//! statement, if an authenticator sends one anyway, is not checked. What is
//! checked, for both ceremonies: the client data's type, challenge (one
//! Gantry made, unused and under five minutes old) and origin; the
//! authenticator data's RP ID hash and user-presence flag; and, for a
//! login, the signature over the authenticator data and the client data's
//! hash with the credential's stored public key, and that the signature
//! counter moved on (a counter that goes backwards means a cloned
//! authenticator). Signatures are checked with `ring`: ES256 (P-256),
//! EdDSA (Ed25519) and RS256, which between them cover phones, security
//! keys and Windows Hello.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ciborium::Value;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// How long a challenge may be answered.
pub const CHALLENGE_LIFETIME: Duration = Duration::from_secs(300);

/// COSE algorithm ids Gantry accepts, in order of preference.
pub const ES256: i64 = -7;
pub const EDDSA: i64 = -8;
pub const RS256: i64 = -257;

/// Why a passkey wasn't accepted. The text is safe to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct WebauthnError(pub String);

fn fail<T>(message: &str) -> Result<T, WebauthnError> {
    Err(WebauthnError(message.to_owned()))
}

/// Who Gantry is to an authenticator: the RP ID (the host name) and the
/// origin pages are served from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelyingParty {
    pub id: String,
    pub origin: String,
}

impl RelyingParty {
    /// From an origin such as `https://gantry.example.com` or
    /// `http://localhost:8080`.
    pub fn from_origin(origin: &str) -> Option<Self> {
        let url = reqwest::Url::parse(origin).ok()?;
        let host = url.host_str()?.to_owned();
        let mut origin = format!("{}://{}", url.scheme(), host);
        if let Some(port) = url.port() {
            origin.push_str(&format!(":{port}"));
        }
        Some(Self { id: host, origin })
    }
}

/// What a challenge was made for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    Register { user_id: i64 },
    Login,
}

/// Challenges waiting for an answer. Each is used once.
#[derive(Default)]
pub struct Challenges(Mutex<HashMap<String, (Purpose, RelyingParty, Instant)>>);

impl Challenges {
    /// A new challenge, base64url.
    pub fn issue(&self, purpose: Purpose, rp: RelyingParty) -> String {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let challenge = URL_SAFE_NO_PAD.encode(bytes);
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.retain(|_, (_, _, at)| at.elapsed() < CHALLENGE_LIFETIME);
        map.insert(challenge.clone(), (purpose, rp, Instant::now()));
        challenge
    }

    /// Takes the challenge if it is live and was made for `purpose`.
    pub fn take(&self, challenge: &str, purpose: &Purpose) -> Option<RelyingParty> {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (made_for, rp, at) = map.remove(challenge)?;
        (made_for == *purpose && at.elapsed() < CHALLENGE_LIFETIME).then_some(rp)
    }
}

fn b64(value: &str) -> Result<Vec<u8>, WebauthnError> {
    URL_SAFE_NO_PAD
        .decode(value.trim_end_matches('='))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value))
        .map_err(|_| WebauthnError("a field isn't base64url".into()))
}

/// Checks the client data's type and origin, and returns its challenge.
fn client_data(json: &[u8], kind: &str) -> Result<(String, String), WebauthnError> {
    let data: serde_json::Value =
        serde_json::from_slice(json).map_err(|_| WebauthnError("unreadable client data".into()))?;
    if data["type"] != kind {
        return fail("the client data is for another ceremony");
    }
    let challenge = data["challenge"]
        .as_str()
        .ok_or_else(|| WebauthnError("the client data has no challenge".into()))?;
    let origin = data["origin"]
        .as_str()
        .ok_or_else(|| WebauthnError("the client data has no origin".into()))?;
    Ok((challenge.to_owned(), origin.to_owned()))
}

/// Authenticator data (WebAuthn §6.1).
#[derive(Debug, Clone)]
pub struct AuthenticatorData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub sign_count: u32,
    /// With the AT flag: the credential id and its COSE public key.
    pub credential: Option<(Vec<u8>, Vec<u8>)>,
}

const USER_PRESENT: u8 = 0x01;
const ATTESTED: u8 = 0x40;

pub fn authenticator_data(bytes: &[u8]) -> Result<AuthenticatorData, WebauthnError> {
    if bytes.len() < 37 {
        return fail("the authenticator data is too short");
    }
    let mut rp_id_hash = [0u8; 32];
    rp_id_hash.copy_from_slice(&bytes[..32]);
    let flags = bytes[32];
    let sign_count = u32::from_be_bytes([bytes[33], bytes[34], bytes[35], bytes[36]]);
    let credential = if flags & ATTESTED != 0 {
        let rest = &bytes[37..];
        if rest.len() < 18 {
            return fail("the attested credential data is too short");
        }
        let len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
        let id = rest
            .get(18..18 + len)
            .ok_or_else(|| WebauthnError("the credential id is cut short".into()))?
            .to_vec();
        // The COSE key is one CBOR item; extensions may follow it.
        let mut key = &rest[18 + len..];
        let start = key.len();
        let _: Value = ciborium::de::from_reader(&mut key)
            .map_err(|_| WebauthnError("the credential's public key isn't CBOR".into()))?;
        let used = start - key.len();
        Some((id, rest[18 + len..18 + len + used].to_vec()))
    } else {
        None
    };
    Ok(AuthenticatorData {
        rp_id_hash,
        flags,
        sign_count,
        credential,
    })
}

fn check_rp(data: &AuthenticatorData, rp: &RelyingParty) -> Result<(), WebauthnError> {
    if data.rp_id_hash[..] != Sha256::digest(rp.id.as_bytes())[..] {
        return fail("the passkey is for another site");
    }
    if data.flags & USER_PRESENT == 0 {
        return fail("the authenticator didn't confirm anyone was there");
    }
    Ok(())
}

fn cose_int(map: &[(Value, Value)], key: i64) -> Option<&Value> {
    map.iter()
        .find(|(k, _)| k.as_integer().and_then(|i| i64::try_from(i).ok()) == Some(key))
        .map(|(_, v)| v)
}

/// The COSE key's algorithm, if it is one Gantry can check.
pub fn cose_algorithm(cose: &[u8]) -> Result<i64, WebauthnError> {
    let value: Value = ciborium::de::from_reader(cose)
        .map_err(|_| WebauthnError("the public key isn't CBOR".into()))?;
    let map = value
        .as_map()
        .ok_or_else(|| WebauthnError("the public key isn't a COSE key".into()))?;
    let alg = cose_int(map, 3)
        .and_then(Value::as_integer)
        .and_then(|i| i64::try_from(i).ok())
        .ok_or_else(|| WebauthnError("the public key names no algorithm".into()))?;
    if ![ES256, EDDSA, RS256].contains(&alg) {
        return fail(
            "the passkey uses an algorithm Gantry can't check (ES256, EdDSA and RS256 can be)",
        );
    }
    Ok(alg)
}

/// Checks `signature` over `message` with a COSE public key.
pub fn verify_signature(
    cose: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), WebauthnError> {
    use ring::signature::{self, UnparsedPublicKey};
    let value: Value = ciborium::de::from_reader(cose)
        .map_err(|_| WebauthnError("the public key isn't CBOR".into()))?;
    let map = value
        .as_map()
        .ok_or_else(|| WebauthnError("the public key isn't a COSE key".into()))?;
    let bytes = |key| cose_int(map, key).and_then(Value::as_bytes).cloned();
    let bad = || WebauthnError("the signature isn't right".into());
    match cose_algorithm(cose)? {
        ES256 => {
            let (Some(x), Some(y)) = (bytes(-2), bytes(-3)) else {
                return fail("the P-256 key is missing a coordinate");
            };
            let mut point = vec![0x04];
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, point)
                .verify(message, signature)
                .map_err(|_| bad())
        }
        EDDSA => {
            let x = bytes(-2).ok_or_else(|| WebauthnError("the Ed25519 key is empty".into()))?;
            UnparsedPublicKey::new(&signature::ED25519, x)
                .verify(message, signature)
                .map_err(|_| bad())
        }
        _ => {
            let (Some(n), Some(e)) = (bytes(-1), bytes(-2)) else {
                return fail("the RSA key is missing a part");
            };
            signature::RsaPublicKeyComponents { n, e }
                .verify(&signature::RSA_PKCS1_2048_8192_SHA256, message, signature)
                .map_err(|_| bad())
        }
    }
}

/// A credential that passed registration.
#[derive(Debug, Clone)]
pub struct NewCredential {
    pub id: String,
    pub public_key: Vec<u8>,
    pub algorithm: i64,
    pub sign_count: u32,
}

/// The browser's answer to `navigator.credentials.create()`, as JSON with
/// base64url fields (the shape `PublicKeyCredential.toJSON()` gives).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RegistrationResponse {
    pub id: String,
    pub response: AttestationResponse,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestationResponse {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    pub attestation_object: String,
}

/// Checks a registration, taking its challenge from `challenges`.
pub fn finish_registration(
    challenges: &Challenges,
    user_id: i64,
    answer: &RegistrationResponse,
) -> Result<NewCredential, WebauthnError> {
    let client_json = b64(&answer.response.client_data_json)?;
    let (challenge, origin) = client_data(&client_json, "webauthn.create")?;
    let rp = challenges
        .take(&challenge, &Purpose::Register { user_id })
        .ok_or_else(|| WebauthnError("the request has expired; try again".into()))?;
    if origin != rp.origin {
        return fail("the passkey was made on another site");
    }
    let attestation: Value =
        ciborium::de::from_reader(&b64(&answer.response.attestation_object)?[..])
            .map_err(|_| WebauthnError("unreadable attestation".into()))?;
    let auth_data = attestation
        .as_map()
        .and_then(|m| {
            m.iter()
                .find(|(k, _)| k.as_text() == Some("authData"))
                .and_then(|(_, v)| v.as_bytes())
        })
        .ok_or_else(|| WebauthnError("the attestation has no authenticator data".into()))?;
    let data = authenticator_data(auth_data)?;
    check_rp(&data, &rp)?;
    let (id, public_key) = data
        .credential
        .ok_or_else(|| WebauthnError("the authenticator sent no credential".into()))?;
    let id = URL_SAFE_NO_PAD.encode(id);
    if id != answer.id.trim_end_matches('=') {
        return fail("the credential id doesn't match");
    }
    let algorithm = cose_algorithm(&public_key)?;
    Ok(NewCredential {
        id,
        public_key,
        algorithm,
        sign_count: data.sign_count,
    })
}

/// The browser's answer to `navigator.credentials.get()`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AssertionAnswer {
    pub id: String,
    pub response: AssertionResponse,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionResponse {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
}

/// Checks a login against a stored credential. Returns the new signature
/// count to store.
pub fn finish_login(
    challenges: &Challenges,
    answer: &AssertionAnswer,
    public_key: &[u8],
    stored_count: u32,
) -> Result<u32, WebauthnError> {
    let client_json = b64(&answer.response.client_data_json)?;
    let (challenge, origin) = client_data(&client_json, "webauthn.get")?;
    let rp = challenges
        .take(&challenge, &Purpose::Login)
        .ok_or_else(|| WebauthnError("the request has expired; try again".into()))?;
    if origin != rp.origin {
        return fail("the passkey was used on another site");
    }
    let auth_bytes = b64(&answer.response.authenticator_data)?;
    let data = authenticator_data(&auth_bytes)?;
    check_rp(&data, &rp)?;
    let mut message = auth_bytes.clone();
    message.extend_from_slice(&Sha256::digest(&client_json));
    verify_signature(public_key, &message, &b64(&answer.response.signature)?)?;
    // Authenticators that keep no counter always send 0.
    if (data.sign_count != 0 || stored_count != 0) && data.sign_count <= stored_count {
        return fail("this passkey's counter went backwards, so it may have been copied");
    }
    Ok(data.sign_count)
}

/// A software authenticator for tests: what a browser's virtual
/// authenticator (or Chrome DevTools' WebAuthn panel) does, made in Rust.
#[cfg(test)]
pub mod virtual_authenticator {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{self, KeyPair};

    pub enum Key {
        P256(signature::EcdsaKeyPair),
        Ed25519(signature::Ed25519KeyPair),
        Rsa(signature::RsaKeyPair),
    }

    /// One credential, with its counter.
    pub struct Authenticator {
        pub credential_id: Vec<u8>,
        key: Key,
        pub count: u32,
        pub user_handle: Option<String>,
    }

    /// A 2048-bit RSA test key (PKCS#1 DER, base64), made with
    /// `openssl genpkey -algorithm RSA`. Only ever used in tests.
    const RSA_TEST_KEY: &str = include_str!("webauthn_test_rsa.b64");

    impl Authenticator {
        pub fn new(algorithm: i64) -> Self {
            let rng = SystemRandom::new();
            let key = match algorithm {
                ES256 => {
                    let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(
                        &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                        &rng,
                    )
                    .unwrap();
                    Key::P256(
                        signature::EcdsaKeyPair::from_pkcs8(
                            &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                            pkcs8.as_ref(),
                            &rng,
                        )
                        .unwrap(),
                    )
                }
                EDDSA => {
                    let pkcs8 = signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
                    Key::Ed25519(signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap())
                }
                _ => {
                    let der = base64::engine::general_purpose::STANDARD
                        .decode(RSA_TEST_KEY.trim())
                        .unwrap();
                    Key::Rsa(signature::RsaKeyPair::from_der(&der).unwrap())
                }
            };
            let mut credential_id = vec![0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut credential_id);
            Self {
                credential_id,
                key,
                count: 0,
                user_handle: None,
            }
        }

        pub fn id(&self) -> String {
            URL_SAFE_NO_PAD.encode(&self.credential_id)
        }

        fn cose(&self) -> Vec<u8> {
            let int = |i: i64| Value::Integer(i.into());
            let map = match &self.key {
                Key::P256(k) => {
                    let point = k.public_key().as_ref();
                    vec![
                        (int(1), int(2)),
                        (int(3), int(ES256)),
                        (int(-1), int(1)),
                        (int(-2), Value::Bytes(point[1..33].to_vec())),
                        (int(-3), Value::Bytes(point[33..65].to_vec())),
                    ]
                }
                Key::Ed25519(k) => vec![
                    (int(1), int(1)),
                    (int(3), int(EDDSA)),
                    (int(-1), int(6)),
                    (int(-2), Value::Bytes(k.public_key().as_ref().to_vec())),
                ],
                Key::Rsa(k) => {
                    let components: signature::RsaPublicKeyComponents<Vec<u8>> =
                        k.public_key().into();
                    vec![
                        (int(1), int(3)),
                        (int(3), int(RS256)),
                        (int(-1), Value::Bytes(components.n)),
                        (int(-2), Value::Bytes(components.e)),
                    ]
                }
            };
            let mut out = Vec::new();
            ciborium::ser::into_writer(&Value::Map(map), &mut out).unwrap();
            out
        }

        fn auth_data(&self, rp_id: &str, flags: u8, attested: bool) -> Vec<u8> {
            let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
            data.push(flags);
            data.extend_from_slice(&self.count.to_be_bytes());
            if attested {
                data.extend_from_slice(&[0u8; 16]);
                data.extend_from_slice(&(self.credential_id.len() as u16).to_be_bytes());
                data.extend_from_slice(&self.credential_id);
                data.extend_from_slice(&self.cose());
            }
            data
        }

        fn client_data(kind: &str, challenge: &str, origin: &str) -> Vec<u8> {
            serde_json::json!({ "type": kind, "challenge": challenge, "origin": origin, "crossOrigin": false })
                .to_string()
                .into_bytes()
        }

        /// The answer to a creation request, as `toJSON()` gives it.
        pub fn register(&mut self, options: &serde_json::Value, origin: &str) -> serde_json::Value {
            let options = &options["publicKey"];
            let challenge = options["challenge"].as_str().unwrap();
            let rp_id = options["rp"]["id"].as_str().unwrap();
            self.user_handle = options["user"]["id"].as_str().map(str::to_owned);
            let client = Self::client_data("webauthn.create", challenge, origin);
            let attestation = Value::Map(vec![
                (Value::Text("fmt".into()), Value::Text("none".into())),
                (Value::Text("attStmt".into()), Value::Map(vec![])),
                (
                    Value::Text("authData".into()),
                    Value::Bytes(self.auth_data(rp_id, 0x45, true)),
                ),
            ]);
            let mut object = Vec::new();
            ciborium::ser::into_writer(&attestation, &mut object).unwrap();
            serde_json::json!({
                "id": self.id(),
                "rawId": self.id(),
                "type": "public-key",
                "response": {
                    "clientDataJSON": URL_SAFE_NO_PAD.encode(client),
                    "attestationObject": URL_SAFE_NO_PAD.encode(object),
                },
            })
        }

        /// The answer to a login request, signed. Counts up first.
        pub fn login(&mut self, options: &serde_json::Value, origin: &str) -> serde_json::Value {
            self.count += 1;
            self.sign_login(options, origin, self.count)
        }

        /// The same, with a counter chosen by the test.
        pub fn sign_login(
            &self,
            options: &serde_json::Value,
            origin: &str,
            count: u32,
        ) -> serde_json::Value {
            let options = &options["publicKey"];
            let challenge = options["challenge"].as_str().unwrap();
            let rp_id = options["rpId"].as_str().unwrap();
            let client = Self::client_data("webauthn.get", challenge, origin);
            let mut auth = Sha256::digest(rp_id.as_bytes()).to_vec();
            auth.push(0x05);
            auth.extend_from_slice(&count.to_be_bytes());
            let mut message = auth.clone();
            message.extend_from_slice(&Sha256::digest(&client));
            let rng = SystemRandom::new();
            let signature = match &self.key {
                Key::P256(k) => k.sign(&rng, &message).unwrap().as_ref().to_vec(),
                Key::Ed25519(k) => k.sign(&message).as_ref().to_vec(),
                Key::Rsa(k) => {
                    let mut out = vec![0u8; k.public().modulus_len()];
                    k.sign(&signature::RSA_PKCS1_SHA256, &rng, &message, &mut out)
                        .unwrap();
                    out
                }
            };
            serde_json::json!({
                "id": self.id(),
                "rawId": self.id(),
                "type": "public-key",
                "response": {
                    "clientDataJSON": URL_SAFE_NO_PAD.encode(client),
                    "authenticatorData": URL_SAFE_NO_PAD.encode(auth),
                    "signature": URL_SAFE_NO_PAD.encode(signature),
                    "userHandle": self.user_handle,
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::virtual_authenticator::Authenticator;
    use super::*;

    fn rp() -> RelyingParty {
        RelyingParty::from_origin("https://gantry.example.com").unwrap()
    }

    fn options(challenges: &Challenges, purpose: Purpose) -> serde_json::Value {
        let challenge = challenges.issue(purpose, rp());
        serde_json::json!({ "publicKey": {
            "challenge": challenge, "rp": { "id": rp().id }, "rpId": rp().id,
            "user": { "id": "aGFuZGxl" },
        } })
    }

    #[test]
    fn the_relying_party_comes_from_the_origin() {
        assert_eq!(
            RelyingParty::from_origin("http://localhost:8080/x").unwrap(),
            RelyingParty {
                id: "localhost".into(),
                origin: "http://localhost:8080".into()
            }
        );
        assert_eq!(rp().origin, "https://gantry.example.com");
    }

    #[test]
    fn each_algorithm_registers_and_logs_in() {
        for algorithm in [ES256, EDDSA, RS256] {
            let challenges = Challenges::default();
            let mut device = Authenticator::new(algorithm);
            let created = device.register(
                &options(&challenges, Purpose::Register { user_id: 1 }),
                &rp().origin,
            );
            let answer: RegistrationResponse = serde_json::from_value(created).unwrap();
            let credential = finish_registration(&challenges, 1, &answer).unwrap();
            assert_eq!(credential.algorithm, algorithm);
            assert_eq!(credential.id, device.id());

            let got = device.login(&options(&challenges, Purpose::Login), &rp().origin);
            let answer: AssertionAnswer = serde_json::from_value(got).unwrap();
            let count = finish_login(&challenges, &answer, &credential.public_key, 0).unwrap();
            assert_eq!(count, 1, "{algorithm}");
        }
    }

    #[test]
    fn wrong_challenges_origins_sites_signatures_and_counters_are_refused() {
        let challenges = Challenges::default();
        let mut device = Authenticator::new(ES256);
        let created = device.register(
            &options(&challenges, Purpose::Register { user_id: 1 }),
            &rp().origin,
        );
        let answer: RegistrationResponse = serde_json::from_value(created.clone()).unwrap();
        let credential = finish_registration(&challenges, 1, &answer).unwrap();
        // The same answer twice: its challenge is used up.
        assert!(finish_registration(&challenges, 1, &answer).is_err());
        // Made for another user.
        let created = device.register(
            &options(&challenges, Purpose::Register { user_id: 2 }),
            &rp().origin,
        );
        let answer: RegistrationResponse = serde_json::from_value(created).unwrap();
        assert!(finish_registration(&challenges, 1, &answer).is_err());

        let login = |origin: &str, count: u32| {
            let got = device.sign_login(&options(&challenges, Purpose::Login), origin, count);
            let answer: AssertionAnswer = serde_json::from_value(got).unwrap();
            finish_login(&challenges, &answer, &credential.public_key, 5)
        };
        assert_eq!(login(&rp().origin, 6), Ok(6));
        assert!(
            login("https://evil.example.com", 7)
                .unwrap_err()
                .0
                .contains("another site")
        );
        assert!(login(&rp().origin, 5).unwrap_err().0.contains("copied"));

        // Another credential's key doesn't check this one's signature.
        let other = Authenticator::new(ES256);
        let got = device.sign_login(&options(&challenges, Purpose::Login), &rp().origin, 9);
        let answer: AssertionAnswer = serde_json::from_value(got).unwrap();
        let mut other_device = other;
        let other_created = other_device.register(
            &options(&challenges, Purpose::Register { user_id: 1 }),
            &rp().origin,
        );
        let other_credential = finish_registration(
            &challenges,
            1,
            &serde_json::from_value(other_created).unwrap(),
        )
        .unwrap();
        assert!(
            finish_login(&challenges, &answer, &other_credential.public_key, 0)
                .unwrap_err()
                .0
                .contains("signature")
        );

        // A passkey for another RP ID.
        let challenge = challenges.issue(Purpose::Login, rp());
        let elsewhere =
            serde_json::json!({ "publicKey": { "challenge": challenge, "rpId": "example.org" } });
        let got = device.sign_login(&elsewhere, &rp().origin, 20);
        let answer: AssertionAnswer = serde_json::from_value(got).unwrap();
        assert!(
            finish_login(&challenges, &answer, &credential.public_key, 0)
                .unwrap_err()
                .0
                .contains("another site")
        );
    }
}
