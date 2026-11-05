//! Time-based one-time passwords (RFC 6238) for two-factor authentication
//! (PRD §9.2): HMAC-SHA1, 6 digits, a 30-second step, as every authenticator
//! app expects. A code from the step before or after is accepted too, for
//! clocks that drift, but never a step already used.
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;
use sha2::{Digest, Sha256};

/// Seconds per step.
pub const STEP: i64 = 30;

/// Digits per code.
pub const DIGITS: u32 = 6;

/// How many recovery codes a user gets.
pub const RECOVERY_CODES: usize = 10;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, without padding, as authenticator apps take it.
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Reads base32, ignoring case, spaces and padding.
pub fn from_base32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let value = BASE32
            .iter()
            .position(|&b| b as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// A new secret: 160 random bits, base32.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base32(&bytes)
}

/// The code for `step` (HOTP, RFC 4226, with the step as the counter).
pub fn code_at_step(secret: &[u8], step: i64, digits: u32) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(&step.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = usize::from(hash[19] & 0x0f);
    let value = (u32::from(hash[offset] & 0x7f) << 24)
        | (u32::from(hash[offset + 1]) << 16)
        | (u32::from(hash[offset + 2]) << 8)
        | u32::from(hash[offset + 3]);
    format!(
        "{:0width$}",
        value % 10u32.pow(digits),
        width = digits as usize
    )
}

/// The step `unix` seconds fall in.
pub fn step_at(unix: i64) -> i64 {
    unix.div_euclid(STEP)
}

/// The step `code` is valid for, within one step of `unix`, when it is
/// later than `last_used`.
pub fn check(secret_b32: &str, code: &str, unix: i64, last_used: Option<i64>) -> Option<i64> {
    let secret = from_base32(secret_b32)?;
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let now = step_at(unix);
    [now - 1, now, now + 1].into_iter().find(|&step| {
        last_used.is_none_or(|used| step > used)
            && constant_eq(&code_at_step(&secret, step, DIGITS), &code)
    })
}

fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// The `otpauth://` URI an authenticator app's QR scanner reads.
pub fn uri(secret_b32: &str, username: &str, issuer: &str) -> String {
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    format!(
        "otpauth://totp/{}:{}?secret={secret_b32}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP}",
        enc(issuer),
        enc(username),
        enc(issuer)
    )
}

/// The URI as a QR code, an SVG to show while setting up.
pub fn qr_svg(uri: &str) -> String {
    qrcode::QrCode::new(uri.as_bytes())
        .map(|code| {
            code.render::<qrcode::render::svg::Color>()
                .min_dimensions(200, 200)
                .quiet_zone(true)
                .build()
        })
        .unwrap_or_default()
}

/// A recovery code: ten characters from base32's letters and digits, in two
/// groups of five, such as `k3nq7-x2pfa`.
pub fn new_recovery_code() -> String {
    let mut bytes = [0u8; 10];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let chars: String = bytes
        .iter()
        .map(|b| BASE32[usize::from(b % 32)] as char)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("{}-{}", &chars[..5], &chars[5..])
}

/// A recovery code as typed: lower case, without spaces or dashes.
pub fn normalise_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// How a recovery code is stored: only its SHA-256.
pub fn hash_recovery_code(code: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(normalise_recovery_code(code).as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B's SHA1 secret.
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc_6238_test_vectors() {
        for (time, code) in [
            (59, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ] {
            assert_eq!(code_at_step(RFC_SECRET, step_at(time), 8), code, "{time}");
        }
        // Six digits are the last six of the eight.
        assert_eq!(code_at_step(RFC_SECRET, step_at(59), 6), "287082");
    }

    #[test]
    fn base32_round_trips() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(from_base32("mzxw 6ytb oi==").unwrap(), b"foobar");
        assert_eq!(from_base32(&base32(RFC_SECRET)).unwrap(), RFC_SECRET);
        assert!(from_base32("not base32!").is_none());
        assert_eq!(new_secret().len(), 32);
    }

    #[test]
    fn codes_are_accepted_one_step_either_way_and_never_twice() {
        let secret = base32(RFC_SECRET);
        let now = 1_111_111_111;
        let code = |at| code_at_step(RFC_SECRET, step_at(at), 6);
        assert_eq!(check(&secret, &code(now), now, None), Some(step_at(now)));
        assert!(check(&secret, &code(now - 30), now, None).is_some());
        assert!(check(&secret, &code(now + 30), now, None).is_some());
        assert!(check(&secret, &code(now - 90), now, None).is_none());
        // A step already used, or one before it, is refused: no replay.
        assert!(check(&secret, &code(now), now, Some(step_at(now))).is_none());
        assert!(check(&secret, "12345", now, None).is_none());
        assert!(check(&secret, "abcdef", now, None).is_none());
        let spaced = format!("{} {}", &code(now)[..3], &code(now)[3..]);
        assert!(check(&secret, &spaced, now, None).is_some());
    }

    #[test]
    fn the_uri_and_qr_code_are_what_apps_read() {
        let uri = uri("JBSWY3DPEHPK3PXP", "adrian", "Gantry");
        assert_eq!(
            uri,
            "otpauth://totp/Gantry:adrian?secret=JBSWY3DPEHPK3PXP&issuer=Gantry&algorithm=SHA1&digits=6&period=30"
        );
        assert!(qr_svg(&uri).contains("<svg"));
    }

    #[test]
    fn recovery_codes_are_hashed_and_typed_loosely() {
        let code = new_recovery_code();
        assert_eq!(code.len(), 11);
        assert_eq!(&code[5..6], "-");
        assert_eq!(
            hash_recovery_code(&code),
            hash_recovery_code(&code.to_uppercase().replace('-', " "))
        );
        assert_ne!(hash_recovery_code(&code), code);
    }
}
