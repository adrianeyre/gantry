//! The P1 and A1 camera: TLS to port 6000, an 80-byte login, then JPEG
//! frames, each after a 16-byte header.
//!
//! The protocol is community-documented (OpenBambuAPI's `video.md`, and the
//! ha-bambulab integration's `ChamberImageThread`), never verified here
//! against a real camera.
//!
//! The login packet, all little-endian:
//!
//! | Bytes | Value |
//! |---|---|
//! | 0..4 | `0x40`, the payload length |
//! | 4..8 | `0x3000`, the packet type |
//! | 8..16 | zero |
//! | 16..48 | the username `bblp`, zero-padded |
//! | 48..80 | the access code, zero-padded |
//!
//! Each frame: a 16-byte header whose first four bytes are the JPEG's length
//! (little-endian), then the JPEG (`FF D8` … `FF D9`).
use bytes::Bytes;
use futures::stream::BoxStream;
use gantry_core::{CameraError, CameraSource};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::{Ended, Target, USERNAME, connect_tls, frames};

/// The largest frame accepted. Real frames are well under 1 MB.
pub const MAX_FRAME: usize = 4 << 20;

/// How long the camera may go quiet before the connection is retried.
const FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// The 80-byte login packet.
pub fn login_packet(access_code: &str) -> [u8; 80] {
    let mut packet = [0u8; 80];
    packet[0..4].copy_from_slice(&0x40u32.to_le_bytes());
    packet[4..8].copy_from_slice(&0x3000u32.to_le_bytes());
    let user = USERNAME.as_bytes();
    packet[16..16 + user.len()].copy_from_slice(user);
    let code = access_code.as_bytes();
    let len = code.len().min(32);
    packet[48..48 + len].copy_from_slice(&code[..len]);
    packet
}

/// Whether `data` looks like a whole JPEG.
pub fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 4 && data.starts_with(&[0xFF, 0xD8]) && data.ends_with(&[0xFF, 0xD9])
}

/// A P1 or A1 series camera.
#[derive(Debug, Clone)]
pub struct P1Camera {
    target: Target,
}

impl P1Camera {
    pub fn new(host: &str, port: u16, access_code: &str) -> Self {
        Self {
            target: Target {
                host: host.trim().to_owned(),
                port,
                access_code: access_code.to_owned(),
            },
        }
    }
}

impl CameraSource for P1Camera {
    fn open(&self) -> BoxStream<'static, Result<Bytes, CameraError>> {
        frames("p1", self.target.clone(), session)
    }
}

/// One connection: log in, then forward frames until something ends it.
async fn session(target: Target, tx: mpsc::Sender<Result<Bytes, CameraError>>) -> Ended {
    let mut tls = match connect_tls(&target).await {
        Ok(tls) => tls,
        Err(detail) => {
            return Ended::Failed {
                detail,
                had_frames: false,
            };
        }
    };
    if let Err(e) = tls.write_all(&login_packet(&target.access_code)).await {
        return Ended::Failed {
            detail: format!("sending the login: {}", e.kind()),
            had_frames: false,
        };
    }
    let _ = tls.flush().await;

    let mut had_frames = false;
    loop {
        let frame = match tokio::time::timeout(FRAME_TIMEOUT, read_frame(&mut tls)).await {
            Ok(Ok(frame)) => frame,
            Ok(Err(e)) if !had_frames && e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ended::LoginUnanswered;
            }
            Ok(Err(e)) => {
                return Ended::Failed {
                    detail: e.to_string(),
                    had_frames,
                };
            }
            Err(_) => {
                return Ended::Failed {
                    detail: "no frame for 15 s".into(),
                    had_frames,
                };
            }
        };
        had_frames = true;
        if tx.send(Ok(frame)).await.is_err() {
            return Ended::Unwatched;
        }
    }
}

/// Reads one header and the JPEG after it.
async fn read_frame<S: AsyncReadExt + Unpin>(stream: &mut S) -> std::io::Result<Bytes> {
    let mut header = [0u8; 16];
    stream.read_exact(&mut header).await?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("a frame header claims {len} bytes"),
        ));
    }
    let mut jpeg = vec![0u8; len];
    stream.read_exact(&mut jpeg).await?;
    if !is_jpeg(&jpeg) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "a frame is not a JPEG",
        ));
    }
    Ok(Bytes::from(jpeg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_packet_is_laid_out_as_documented() {
        let packet = login_packet("12345678");
        assert_eq!(&packet[0..8], &[0x40, 0, 0, 0, 0, 0x30, 0, 0]);
        assert_eq!(&packet[8..16], &[0; 8]);
        assert_eq!(&packet[16..20], b"bblp");
        assert!(packet[20..48].iter().all(|b| *b == 0));
        assert_eq!(&packet[48..56], b"12345678");
        assert!(packet[56..80].iter().all(|b| *b == 0));
    }

    #[tokio::test]
    async fn frames_must_be_whole_jpegs() {
        let mut ok = 4u32.to_le_bytes().to_vec();
        ok.extend_from_slice(&[0; 12]);
        ok.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
        assert!(read_frame(&mut ok.as_slice()).await.is_ok());

        let mut bad = 4u32.to_le_bytes().to_vec();
        bad.extend_from_slice(&[0; 12]);
        bad.extend_from_slice(b"nope");
        assert!(read_frame(&mut bad.as_slice()).await.is_err());

        let mut huge = (u32::MAX).to_le_bytes().to_vec();
        huge.extend_from_slice(&[0; 12]);
        assert!(read_frame(&mut huge.as_slice()).await.is_err());
    }
}
