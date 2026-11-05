//! A mock of a Bambu printer's FTPS server, for upload tests (PRD §5.6).
//!
//! Implicit TLS from the first byte, as on port 990, with a self-signed
//! certificate. It takes the commands an upload needs: `USER` (only `bblp`),
//! `PASS` (the access code), `TYPE`, `PBSZ`, `PROT`, `EPSV`, `PASV`, `STOR`
//! and `QUIT`, and answers `502` to anything else. Data connections are TLS
//! too, and must resume the control connection's TLS session, as Bambu's
//! server is reported to require. Stored files are kept in memory.
//!
//! What a real printer's server says is unverified (#43): the replies here
//! are vsftpd's, which Bambu's is reported to resemble.
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::HandshakeKind;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::USERNAME;

/// The largest file the mock stores.
const MAX_FILE: usize = 64 << 20;

/// One login attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub user: String,
    pub accepted: bool,
}

struct Shared {
    access_code: String,
    tls: TlsAcceptor,
    files: Mutex<Vec<(String, Vec<u8>)>>,
    file_arrived: Notify,
    logins: Mutex<Vec<Login>>,
    refuse_login: Mutex<bool>,
    fail_stor: Mutex<Option<(u16, String)>>,
    /// While true, the reply after a transfer waits.
    hold: watch::Sender<bool>,
    /// Whether each data connection resumed the control session.
    resumed: Mutex<Vec<bool>>,
}

/// A running mock FTPS server.
pub struct MockFtps {
    shared: Arc<Shared>,
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl MockFtps {
    /// Starts a server on a random local port for a printer with this
    /// access code.
    pub async fn start(access_code: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let (hold, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            access_code: access_code.to_owned(),
            tls: crate::tls_acceptor("localhost"),
            files: Mutex::new(Vec::new()),
            file_arrived: Notify::new(),
            logins: Mutex::new(Vec::new()),
            refuse_login: Mutex::new(false),
            fail_stor: Mutex::new(None),
            hold,
            resumed: Mutex::new(Vec::new()),
        });
        let server = shared.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                tokio::spawn(session(server.clone(), stream));
            }
        });
        Self {
            shared,
            address,
            task,
        }
    }

    /// Where the server listens (instead of port 990).
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    /// Every file stored so far, by name, in order.
    pub fn files(&self) -> Vec<(String, Vec<u8>)> {
        self.shared.files.lock().expect("lock").clone()
    }

    /// Waits until a file has been stored under `name`.
    pub async fn wait_for_file(&self, name: &str, timeout: Duration) -> Option<Vec<u8>> {
        tokio::time::timeout(timeout, async {
            loop {
                let arrived = self.shared.file_arrived.notified();
                if let Some((_, bytes)) = self.files().into_iter().find(|(n, _)| n == name) {
                    return bytes;
                }
                arrived.await;
            }
        })
        .await
        .ok()
    }

    /// Every login attempt so far.
    pub fn logins(&self) -> Vec<Login> {
        self.shared.logins.lock().expect("lock").clone()
    }

    /// Whether each data connection so far resumed the control connection's
    /// TLS session. One that didn't was refused.
    pub fn data_sessions_resumed(&self) -> Vec<bool> {
        self.shared.resumed.lock().expect("lock").clone()
    }

    /// Refuses every login from now on, whatever the access code.
    pub fn refuse_login(&self) {
        *self.shared.refuse_login.lock().expect("lock") = true;
    }

    /// Answers every `STOR` from now on with this reply instead of storing.
    pub fn fail_stor(&self, code: u16, message: &str) {
        *self.shared.fail_stor.lock().expect("lock") = Some((code, message.to_owned()));
    }

    /// Holds the reply after each transfer (the file is stored first) until
    /// [`release`](Self::release), so a test can see what happens before it.
    pub fn hold(&self) {
        self.shared.hold.send_replace(true);
    }

    pub fn release(&self) {
        self.shared.hold.send_replace(false);
    }
}

impl Drop for MockFtps {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type Control = BufReader<tokio_rustls::server::TlsStream<TcpStream>>;

async fn reply(control: &mut Control, line: &str) -> std::io::Result<()> {
    let stream = control.get_mut();
    stream.write_all(format!("{line}\r\n").as_bytes()).await?;
    stream.flush().await
}

/// One client, from the TLS handshake to `QUIT`.
async fn session(shared: Arc<Shared>, stream: TcpStream) {
    let Ok(Ok(tls)) = tokio::time::timeout(Duration::from_secs(5), shared.tls.accept(stream)).await
    else {
        return;
    };
    let mut control = BufReader::new(tls);
    if reply(&mut control, "220 (vsFTPd 3.0.3)").await.is_err() {
        return;
    }
    let mut user: Option<String> = None;
    let mut logged_in = false;
    let mut protected = false;
    let mut passive: Option<TcpListener> = None;
    loop {
        let mut line = String::new();
        match tokio::time::timeout(Duration::from_secs(30), control.read_line(&mut line)).await {
            Ok(Ok(n)) if n > 0 => {}
            _ => return,
        }
        let line = line.trim_end();
        let (command, argument) = line.split_once(' ').unwrap_or((line, ""));
        let command = command.to_ascii_uppercase();
        let answer = match command.as_str() {
            "USER" => {
                user = Some(argument.to_owned());
                "331 Please specify the password.".to_owned()
            }
            "PASS" => {
                let name = user.take().unwrap_or_default();
                logged_in = name == USERNAME
                    && argument == shared.access_code
                    && !*shared.refuse_login.lock().expect("lock");
                shared.logins.lock().expect("lock").push(Login {
                    user: name,
                    accepted: logged_in,
                });
                if logged_in {
                    "230 Login successful.".to_owned()
                } else {
                    "530 Login incorrect.".to_owned()
                }
            }
            "QUIT" => {
                let _ = reply(&mut control, "221 Goodbye.").await;
                let _ = control.get_mut().shutdown().await;
                return;
            }
            _ if !logged_in => "530 Please login with USER and PASS.".to_owned(),
            "TYPE" => "200 Switching to Binary mode.".to_owned(),
            "PBSZ" => "200 PBSZ set to 0.".to_owned(),
            "PROT" if argument.eq_ignore_ascii_case("P") => {
                protected = true;
                "200 PROT now Private.".to_owned()
            }
            "PROT" => "536 PROT not supported.".to_owned(),
            "EPSV" | "PASV" => match TcpListener::bind("127.0.0.1:0").await {
                Ok(listener) => {
                    let port = listener.local_addr().expect("address").port();
                    passive = Some(listener);
                    if command == "EPSV" {
                        format!("229 Entering Extended Passive Mode (|||{port}|)")
                    } else {
                        format!(
                            "227 Entering Passive Mode (127,0,0,1,{},{}).",
                            port / 256,
                            port % 256
                        )
                    }
                }
                Err(_) => "425 Can't open a data port.".to_owned(),
            },
            "STOR" => {
                let failure = shared.fail_stor.lock().expect("lock").clone();
                if let Some((code, message)) = failure {
                    passive = None;
                    format!("{code} {message}")
                } else if !protected {
                    "522 Data connections must be encrypted.".to_owned()
                } else if let Some(listener) = passive.take() {
                    match store(&shared, &mut control, listener, argument).await {
                        Ok(answer) => answer,
                        Err(_) => return,
                    }
                } else {
                    "425 Use PORT or PASV first.".to_owned()
                }
            }
            _ => "502 Command not implemented.".to_owned(),
        };
        if reply(&mut control, &answer).await.is_err() {
            return;
        }
    }
}

/// Takes one file over a TLS data connection, answering `150` first. Returns
/// the reply to send once it is done.
async fn store(
    shared: &Shared,
    control: &mut Control,
    listener: TcpListener,
    name: &str,
) -> std::io::Result<String> {
    let accepted = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await;
    let Ok(Ok((stream, _))) = accepted else {
        return Ok("425 Failed to establish connection.".into());
    };
    reply(control, "150 Ok to send data.").await?;
    let Ok(Ok(mut data)) =
        tokio::time::timeout(Duration::from_secs(5), shared.tls.accept(stream)).await
    else {
        return Ok("522 SSL connection failed.".into());
    };
    let resumed = data.get_ref().1.handshake_kind() == Some(HandshakeKind::Resumed);
    shared.resumed.lock().expect("lock").push(resumed);
    if !resumed {
        return Ok("522 SSL connection failed: session reuse required".into());
    }
    let mut bytes = Vec::new();
    let read = tokio::time::timeout(
        Duration::from_secs(30),
        (&mut data)
            .take(MAX_FILE as u64 + 1)
            .read_to_end(&mut bytes),
    )
    .await;
    if !matches!(read, Ok(Ok(_))) || bytes.len() > MAX_FILE {
        return Ok("426 Failure reading network stream.".into());
    }
    let _ = data.shutdown().await;
    shared
        .files
        .lock()
        .expect("lock")
        .push((name.to_owned(), bytes));
    shared.file_arrived.notify_waiters();
    let mut hold = shared.hold.subscribe();
    let _ = hold.wait_for(|held| !*held).await;
    Ok("226 Transfer complete.".into())
}
