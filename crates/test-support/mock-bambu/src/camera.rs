//! A mock of a P1 or A1 printer's camera (port 6000), for tests.
//!
//! TLS with a self-signed certificate, then the 80-byte login packet the
//! community documents (see `fixtures/README.md`), then JPEG frames, each
//! after a 16-byte header whose first four bytes are the JPEG's length. A
//! wrong login is answered by closing the connection, which is assumed (not
//! documented) to be what a real camera does.
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// How often the mock sends a frame.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(50);

struct Shared {
    access_code: String,
    tls: tokio_rustls::TlsAcceptor,
    cut: watch::Sender<u64>,
    open: AtomicUsize,
    accepted: AtomicUsize,
    refused: AtomicUsize,
    frames_sent: AtomicUsize,
}

/// A running mock P1/A1 camera.
pub struct MockP1Camera {
    shared: Arc<Shared>,
    address: SocketAddr,
    front: JoinHandle<()>,
}

/// A small test JPEG whose colour changes with `n`.
pub fn test_frame(n: usize) -> Bytes {
    let (w, h) = (64u16, 48u16);
    let shade = (n * 37 % 200) as u8 + 30;
    let mut rgb = Vec::with_capacity(usize::from(w) * usize::from(h) * 3);
    for y in 0..h {
        for x in 0..w {
            rgb.extend_from_slice(&[shade, (x * 4) as u8, (y * 5) as u8]);
        }
    }
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, 80)
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .expect("an in-memory JPEG encodes");
    Bytes::from(jpeg)
}

impl MockP1Camera {
    /// Starts a camera on a random local port that accepts `access_code`.
    pub async fn start(access_code: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let (cut, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            access_code: access_code.to_owned(),
            tls: crate::tls_acceptor("camera"),
            cut,
            open: AtomicUsize::new(0),
            accepted: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            frames_sent: AtomicUsize::new(0),
        });
        let accepting = shared.clone();
        let front = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                tokio::spawn(connection(accepting.clone(), stream));
            }
        });
        Self {
            shared,
            address,
            front,
        }
    }

    /// Where the camera listens (instead of port 6000).
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Logged-in connections streaming right now.
    pub fn open_connections(&self) -> usize {
        self.shared.open.load(Ordering::SeqCst)
    }

    pub fn accepted_logins(&self) -> usize {
        self.shared.accepted.load(Ordering::SeqCst)
    }

    pub fn refused_logins(&self) -> usize {
        self.shared.refused.load(Ordering::SeqCst)
    }

    pub fn frames_sent(&self) -> usize {
        self.shared.frames_sent.load(Ordering::SeqCst)
    }

    /// Cuts every open connection, as a network drop would. The camera keeps
    /// listening, so a client can reconnect.
    pub fn drop_connections(&self) {
        self.shared.cut.send_modify(|n| *n += 1);
    }

    /// Waits until `pred` holds for the number of open connections.
    pub async fn wait_for_open(&self, timeout: Duration, pred: impl Fn(usize) -> bool) -> bool {
        tokio::time::timeout(timeout, async {
            while !pred(self.open_connections()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }
}

impl Drop for MockP1Camera {
    fn drop(&mut self) {
        self.front.abort();
        self.drop_connections();
    }
}

/// Whether an 80-byte login packet carries `access_code`.
fn login_ok(packet: &[u8; 80], access_code: &str) -> bool {
    let field = |range: std::ops::Range<usize>| {
        let bytes = &packet[range];
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        bytes[..end].to_vec()
    };
    packet[0..4] == 0x40u32.to_le_bytes()
        && packet[4..8] == 0x3000u32.to_le_bytes()
        && field(16..48) == crate::USERNAME.as_bytes()
        && field(48..80) == access_code.as_bytes()
}

async fn connection(shared: Arc<Shared>, stream: TcpStream) {
    let mut cut = shared.cut.subscribe();
    let Ok(Ok(mut tls)) =
        tokio::time::timeout(Duration::from_secs(5), shared.tls.accept(stream)).await
    else {
        return;
    };
    let mut login = [0u8; 80];
    let read = tokio::time::timeout(Duration::from_secs(5), tls.read_exact(&mut login)).await;
    if !matches!(read, Ok(Ok(_))) {
        return;
    }
    if !login_ok(&login, &shared.access_code) {
        shared.refused.fetch_add(1, Ordering::SeqCst);
        let _ = tls.shutdown().await;
        return;
    }
    shared.accepted.fetch_add(1, Ordering::SeqCst);
    shared.open.fetch_add(1, Ordering::SeqCst);
    let (mut reader, mut writer) = tokio::io::split(tls);
    // The client never sends anything after the login, so a read ending
    // means it has gone.
    let gone = async move {
        let mut buf = [0u8; 64];
        while matches!(reader.read(&mut buf).await, Ok(n) if n > 0) {}
    };
    let stream = async {
        let mut n = 0;
        loop {
            let frame = test_frame(n);
            let mut header = [0u8; 16];
            header[0..4].copy_from_slice(&(frame.len() as u32).to_le_bytes());
            let sent = async {
                writer.write_all(&header).await?;
                writer.write_all(&frame).await?;
                writer.flush().await
            };
            if sent.await.is_err() {
                break;
            }
            shared.frames_sent.fetch_add(1, Ordering::SeqCst);
            n += 1;
            tokio::time::sleep(FRAME_INTERVAL).await;
        }
    };
    tokio::select! {
        () = stream => {}
        () = gone => {}
        _ = cut.changed() => {}
    }
    shared.open.fetch_sub(1, Ordering::SeqCst);
}
