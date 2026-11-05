//! HTTP Digest authentication (RFC 7616, MD5), as PrusaLink asks for it:
//! the printer answers `401` with a `WWW-Authenticate: Digest …` challenge,
//! and every request after carries an `Authorization: Digest …` answer made
//! from the username, the password and the challenge's nonce. The password
//! itself is never sent.
//!
//! Both forms are answered: with `qop=auth` (a client nonce and a counter
//! in each answer) and without a `qop` (the older RFC 2069 form, which
//! Prusa's printer firmware has used). Only MD5 is supported, which is what
//! PrusaLink uses.
use md5::{Digest as _, Md5};

/// A server's Digest challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    /// Whether the server takes `qop=auth`.
    pub qop_auth: bool,
    /// The server says the nonce was out of date, not the credentials wrong.
    pub stale: bool,
}

/// Why a challenge couldn't be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeError {
    /// Not a Digest challenge at all.
    NotDigest,
    /// A Digest challenge with an algorithm other than MD5.
    Algorithm(String),
    /// A Digest challenge without a realm or nonce.
    Incomplete,
}

/// The `key=value` pairs of a challenge or answer, values unquoted.
pub fn params(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq]
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        rest = rest[eq + 1..].trim_start();
        let value;
        if let Some(quoted) = rest.strip_prefix('"') {
            let mut text = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, next)) = chars.next() {
                            text.push(next);
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    c => text.push(c),
                }
            }
            value = text;
            rest = &quoted[end..];
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].trim().to_owned();
            rest = &rest[end..];
        }
        rest = rest.trim_start().trim_start_matches(',').trim_start();
        out.push((key, value));
    }
    out
}

impl Challenge {
    /// Reads a `WWW-Authenticate` header's value.
    pub fn parse(header: &str) -> Result<Self, ChallengeError> {
        let header = header.trim();
        let (scheme, rest) = header.split_once(' ').unwrap_or((header, ""));
        if !scheme.eq_ignore_ascii_case("digest") {
            return Err(ChallengeError::NotDigest);
        }
        let params = params(rest);
        let get = |key: &str| {
            params
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        if let Some(algorithm) = get("algorithm")
            && !algorithm.eq_ignore_ascii_case("md5")
        {
            return Err(ChallengeError::Algorithm(algorithm));
        }
        let (Some(realm), Some(nonce)) = (get("realm"), get("nonce")) else {
            return Err(ChallengeError::Incomplete);
        };
        Ok(Self {
            realm,
            nonce,
            opaque: get("opaque"),
            qop_auth: get("qop")
                .is_some_and(|q| q.split(',').any(|q| q.trim().eq_ignore_ascii_case("auth"))),
            stale: get("stale").is_some_and(|s| s.eq_ignore_ascii_case("true")),
        })
    }

    /// The `Authorization` header's value for one request. `nc` counts the
    /// answers made to this nonce, from 1; `cnonce` is the client's own
    /// random nonce (used only with `qop=auth`).
    pub fn answer(
        &self,
        username: &str,
        password: &str,
        method: &str,
        uri: &str,
        nc: u32,
        cnonce: &str,
    ) -> String {
        let ha1 = md5_hex(&format!("{username}:{}:{password}", self.realm));
        let ha2 = md5_hex(&format!("{method}:{uri}"));
        let mut header = format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", algorithm=MD5",
            quote(username),
            quote(&self.realm),
            quote(&self.nonce),
            quote(uri)
        );
        let response = if self.qop_auth {
            let nc = format!("{nc:08x}");
            header.push_str(&format!(
                ", qop=auth, nc={nc}, cnonce=\"{}\"",
                quote(cnonce)
            ));
            md5_hex(&format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", self.nonce))
        } else {
            md5_hex(&format!("{ha1}:{}:{ha2}", self.nonce))
        };
        header.push_str(&format!(", response=\"{response}\""));
        if let Some(opaque) = &self.opaque {
            header.push_str(&format!(", opaque=\"{}\"", quote(opaque)));
        }
        header
    }
}

fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// MD5 of `text`, as lowercase hex.
pub fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param<'a>(params: &'a [(String, String)], key: &str) -> &'a str {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap()
    }

    #[test]
    fn answers_rfc_2617s_own_example() {
        let challenge = Challenge::parse(
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();
        assert!(challenge.qop_auth);
        let answer = challenge.answer(
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            1,
            "0a4f113b",
        );
        let params = params(answer.strip_prefix("Digest ").unwrap());
        assert_eq!(
            param(&params, "response"),
            "6629fae49393a05397450978507c4ef1"
        );
        assert_eq!(param(&params, "nc"), "00000001");
        assert_eq!(param(&params, "opaque"), "5ccc069c403ebaf9f0171e9517f40e41");
    }

    #[test]
    fn answers_a_challenge_without_qop() {
        let challenge =
            Challenge::parse(r#"Digest realm="Printer API", nonce="0ab5c2a8d1e3f456""#).unwrap();
        assert!(!challenge.qop_auth && !challenge.stale);
        let answer = challenge.answer("maker", "secret", "GET", "/api/v1/status", 1, "x");
        let params = params(answer.strip_prefix("Digest ").unwrap());
        let ha1 = md5_hex("maker:Printer API:secret");
        let ha2 = md5_hex("GET:/api/v1/status");
        assert_eq!(
            param(&params, "response"),
            md5_hex(&format!("{ha1}:0ab5c2a8d1e3f456:{ha2}"))
        );
        assert!(!answer.contains("qop"));
        assert!(!answer.contains("secret"));
    }

    #[test]
    fn reads_stale_and_refuses_what_it_cant_answer() {
        let stale =
            Challenge::parse(r#"Digest realm="Printer API", nonce="n2", stale=TRUE"#).unwrap();
        assert!(stale.stale);
        assert_eq!(
            Challenge::parse("Basic realm=\"x\""),
            Err(ChallengeError::NotDigest)
        );
        assert_eq!(
            Challenge::parse(r#"Digest realm="x", nonce="n", algorithm=SHA-256"#),
            Err(ChallengeError::Algorithm("SHA-256".into()))
        );
        assert_eq!(
            Challenge::parse(r#"Digest realm="x""#),
            Err(ChallengeError::Incomplete)
        );
    }

    #[test]
    fn quoted_values_may_hold_commas_and_escapes() {
        let params = params(r#"realm="a, \"b\"", nonce=xyz"#);
        assert_eq!(param(&params, "realm"), r#"a, "b""#);
        assert_eq!(param(&params, "nonce"), "xyz");
    }
}
