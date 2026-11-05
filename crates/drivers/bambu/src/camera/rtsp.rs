//! Just enough of an RTSP 1.0 client (RFC 2326) for one H.264 video track
//! over TCP-interleaved RTP: requests and responses, Basic and Digest
//! authentication (RFC 2617), and the parts of SDP that matter.
use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use md5::{Digest, Md5};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt};

/// The longest header block or body accepted.
const MAX_MESSAGE: usize = 64 << 10;

/// A response, headers keyed in lower case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// What arrives on the connection once playing.
#[derive(Debug)]
pub enum Incoming {
    /// An interleaved packet on `channel`.
    Data {
        channel: u8,
        payload: Vec<u8>,
    },
    Response(Response),
}

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

/// Reads the next interleaved packet or response.
pub async fn read_incoming<R: AsyncRead + AsyncBufReadExt + Unpin>(
    reader: &mut R,
) -> std::io::Result<Incoming> {
    let first = reader.read_u8().await?;
    if first == b'$' {
        let channel = reader.read_u8().await?;
        let len = reader.read_u16().await? as usize;
        let mut payload = vec![0u8; len];
        reader.read_exact(&mut payload).await?;
        return Ok(Incoming::Data { channel, payload });
    }
    Ok(Incoming::Response(
        read_response_after(reader, first).await?,
    ))
}

/// Reads one response, skipping any interleaved packets before it.
pub async fn read_response<R: AsyncRead + AsyncBufReadExt + Unpin>(
    reader: &mut R,
) -> std::io::Result<Response> {
    loop {
        if let Incoming::Response(response) = read_incoming(reader).await? {
            return Ok(response);
        }
    }
}

async fn read_response_after<R: AsyncRead + AsyncBufReadExt + Unpin>(
    reader: &mut R,
    first: u8,
) -> std::io::Result<Response> {
    let mut head = vec![first];
    loop {
        let before = head.len();
        // Bounded, so a stream with no newline can't grow the buffer forever.
        let room = (MAX_MESSAGE + 1).saturating_sub(head.len()) as u64;
        (&mut *reader)
            .take(room)
            .read_until(b'\n', &mut head)
            .await?;
        if head.len() == before {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        if head.ends_with(b"\r\n\r\n") || head.ends_with(b"\n\n") {
            break;
        }
        if head.len() > MAX_MESSAGE {
            return Err(invalid("an RTSP header block is too long"));
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split_whitespace();
    if !parts.next().is_some_and(|v| v.starts_with("RTSP/")) {
        return Err(invalid("not an RTSP response"));
    }
    let status = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("an RTSP response without a status"))?;
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    if length > MAX_MESSAGE {
        return Err(invalid("an RTSP body is too long"));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Ok(Response {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// A challenge from `WWW-Authenticate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Challenge {
    Basic,
    Digest {
        realm: String,
        nonce: String,
        qop_auth: bool,
        opaque: Option<String>,
    },
}

/// Parses `key="value", key=value` pairs.
fn params(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let Some((key, after)) = rest.split_once('=') else {
            break;
        };
        let key = key
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        let after = after.trim_start();
        let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
            match quoted.split_once('"') {
                Some((v, n)) => (v.to_owned(), n),
                None => (quoted.to_owned(), ""),
            }
        } else {
            match after.split_once(',') {
                Some((v, n)) => (v.trim().to_owned(), n),
                None => (after.trim().to_owned(), ""),
            }
        };
        out.insert(key, value);
        rest = next.trim_start().trim_start_matches(',').trim_start();
    }
    out
}

impl Challenge {
    /// The strongest challenge the server offers, Digest over Basic.
    pub fn choose(response: &Response) -> Option<Self> {
        // Several WWW-Authenticate headers fold into one, so look for each.
        let all = response.header("www-authenticate")?;
        if let Some(at) = all.find("Digest") {
            let p = params(&all[at + "Digest".len()..]);
            return Some(Self::Digest {
                realm: p.get("realm").cloned().unwrap_or_default(),
                nonce: p.get("nonce").cloned()?,
                qop_auth: p
                    .get("qop")
                    .is_some_and(|q| q.split(',').any(|q| q.trim() == "auth")),
                opaque: p.get("opaque").cloned(),
            });
        }
        all.contains("Basic").then_some(Self::Basic)
    }

    /// The `Authorization` header value for one request.
    pub fn authorize(
        &self,
        user: &str,
        password: &str,
        method: &str,
        uri: &str,
        nc: u32,
    ) -> String {
        match self {
            Self::Basic => format!("Basic {}", STANDARD.encode(format!("{user}:{password}"))),
            Self::Digest {
                realm,
                nonce,
                qop_auth,
                opaque,
            } => {
                let ha1 = md5_hex(&format!("{user}:{realm}:{password}"));
                let ha2 = md5_hex(&format!("{method}:{uri}"));
                let mut header = format!(
                    "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\""
                );
                if *qop_auth {
                    let nc = format!("{nc:08x}");
                    let cnonce = md5_hex(&format!("{nonce}:{nc}:gantry"))[..16].to_owned();
                    let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
                    header.push_str(&format!(
                        ", qop=auth, nc={nc}, cnonce=\"{cnonce}\", response=\"{response}\""
                    ));
                } else {
                    let response = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
                    header.push_str(&format!(", response=\"{response}\""));
                }
                if let Some(opaque) = opaque {
                    header.push_str(&format!(", opaque=\"{opaque}\""));
                }
                header
            }
        }
    }
}

pub fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The H.264 track in an SDP description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrack {
    /// `a=control`, as written.
    pub control: Option<String>,
    pub payload_type: Option<u8>,
    /// Decoded `sprop-parameter-sets`: the SPS and PPS, without start codes.
    pub parameter_sets: Vec<Vec<u8>>,
}

/// Finds the first H.264 video track.
pub fn video_track(sdp: &str) -> Option<VideoTrack> {
    let mut in_video = false;
    let mut track: Option<VideoTrack> = None;
    let mut h264_pt: Option<u8> = None;
    let mut fmtp: BTreeMap<u8, String> = BTreeMap::new();
    for line in sdp.lines().map(str::trim) {
        if let Some(media) = line.strip_prefix("m=") {
            if track.is_some() {
                break;
            }
            in_video = media.starts_with("video");
            if in_video {
                track = Some(VideoTrack {
                    control: None,
                    payload_type: media.split_whitespace().nth(3).and_then(|p| p.parse().ok()),
                    parameter_sets: Vec::new(),
                });
            }
            continue;
        }
        if !in_video {
            continue;
        }
        let Some(track) = track.as_mut() else {
            continue;
        };
        if let Some(control) = line.strip_prefix("a=control:") {
            track.control = Some(control.trim().to_owned());
        } else if let Some(map) = line.strip_prefix("a=rtpmap:") {
            let (pt, codec) = map.split_once(' ').unwrap_or((map, ""));
            if codec.to_ascii_uppercase().starts_with("H264") {
                h264_pt = pt.trim().parse().ok();
            }
        } else if let Some(f) = line.strip_prefix("a=fmtp:") {
            let (pt, rest) = f.split_once(' ').unwrap_or((f, ""));
            if let Ok(pt) = pt.trim().parse() {
                fmtp.insert(pt, rest.to_owned());
            }
        }
    }
    let mut track = track?;
    if h264_pt.is_some() {
        track.payload_type = h264_pt;
    }
    if let Some(f) = track.payload_type.and_then(|pt| fmtp.get(&pt)) {
        for part in f.split(';') {
            if let Some(sets) = part.trim().strip_prefix("sprop-parameter-sets=") {
                track.parameter_sets = sets
                    .split(',')
                    .filter_map(|s| STANDARD.decode(s.trim()).ok())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    Some(track)
}

/// Resolves a track's `a=control` against the session's base URL.
pub fn resolve_control(base: &str, control: Option<&str>) -> String {
    match control {
        None | Some("*") | Some("") => base.to_owned(),
        Some(c) if c.starts_with("rtsp://") || c.starts_with("rtsps://") => c.to_owned(),
        Some(c) => format!(
            "{}/{}",
            base.trim_end_matches('/'),
            c.trim_start_matches('/')
        ),
    }
}

/// The session id from a `Session` header, without its timeout.
pub fn session_id(header: &str) -> (String, Option<u64>) {
    let mut parts = header.split(';');
    let id = parts.next().unwrap_or_default().trim().to_owned();
    let timeout = parts
        .filter_map(|p| p.trim().strip_prefix("timeout="))
        .find_map(|t| t.trim().parse().ok());
    (id, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_matches_rfc_2617_without_qop() {
        let challenge = Challenge::Digest {
            realm: "testrealm@host.com".into(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".into(),
            qop_auth: false,
            opaque: None,
        };
        let header = challenge.authorize("Mufasa", "Circle Of Life", "GET", "/dir/index.html", 1);
        // HA1 and HA2 as RFC 2617 §3.5 computes them, without qop (RFC 2069).
        let ha1 = md5_hex("Mufasa:testrealm@host.com:Circle Of Life");
        let ha2 = md5_hex("GET:/dir/index.html");
        let expected = md5_hex(&format!("{ha1}:dcd98b7102dd2f0e8b11d0f600bfb0c093:{ha2}"));
        assert!(
            header.contains(&format!("response=\"{expected}\"")),
            "{header}"
        );
        assert_eq!(ha1, "939e7578ed9e3c518a452acee763bce9");
    }

    #[test]
    fn chooses_digest_over_basic() {
        let response = Response {
            status: 401,
            headers: [(
                "www-authenticate".to_owned(),
                "Basic realm=\"x\", Digest realm=\"LIVE555\", nonce=\"abc\", qop=\"auth,auth-int\""
                    .to_owned(),
            )]
            .into(),
            body: String::new(),
        };
        assert_eq!(
            Challenge::choose(&response),
            Some(Challenge::Digest {
                realm: "LIVE555".into(),
                nonce: "abc".into(),
                qop_auth: true,
                opaque: None
            })
        );
    }

    #[test]
    fn reads_the_video_track_from_sdp() {
        let sdp = "v=0\r\nm=audio 0 RTP/AVP 0\r\na=control:audio\r\n\
                   m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
                   a=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM48gA==\r\n\
                   a=control:trackID=1\r\n";
        let track = video_track(sdp).unwrap();
        assert_eq!(track.control.as_deref(), Some("trackID=1"));
        assert_eq!(track.payload_type, Some(96));
        assert_eq!(track.parameter_sets.len(), 2);
        assert_eq!(track.parameter_sets[0][0] & 0x1F, 7);
        assert_eq!(track.parameter_sets[1][0] & 0x1F, 8);
        assert_eq!(
            resolve_control("rtsps://h:322/streaming/live/1/", Some("trackID=1")),
            "rtsps://h:322/streaming/live/1/trackID=1"
        );
        assert_eq!(resolve_control("rtsps://h/x", Some("*")), "rtsps://h/x");
    }

    #[test]
    fn reads_session_ids() {
        assert_eq!(
            session_id("12345678;timeout=60"),
            ("12345678".into(), Some(60))
        );
        assert_eq!(session_id("abc"), ("abc".into(), None));
    }

    #[tokio::test]
    async fn reads_responses_between_interleaved_packets() {
        let bytes = b"$\x00\x00\x03abcRTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Length: 4\r\n\r\nbody$\x01\x00\x01z".to_vec();
        let mut reader = tokio::io::BufReader::new(bytes.as_slice());
        let response = read_response(&mut reader).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.header("CSeq"), Some("2"));
        assert_eq!(response.body, "body");
        match read_incoming(&mut reader).await.unwrap() {
            Incoming::Data { channel, payload } => {
                assert_eq!(channel, 1);
                assert_eq!(payload, b"z");
            }
            other => panic!("{other:?}"),
        }
    }
}
