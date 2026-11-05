//! Uploading a Print File to a Bambu printer over FTPS (PRD §10.3).
//!
//! The printer runs an FTP server with implicit TLS on port 990: TLS from the
//! first byte, username `bblp` and the access code as password. The file goes
//! to the root of the SD card. The data connection is TLS too (`PROT P`), and
//! Bambu's server is reported to require it to resume the control
//! connection's TLS session, as vsftpd's `require_ssl_reuse` does. Both
//! connections use one rustls config with the same server name, so rustls
//! resumes the session on its own.
//!
//! The steps follow Python's `ftplib`, which community tools use with these
//! printers: open the data connection after `EPSV` (or `PASV`), send `STOR`,
//! and start TLS on it once the server says `150`.
//!
//! Tested against `gantry-mock-bambu`'s FTPS server only. Real firmware is
//! unverified (tracked in #43).
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// The port a Bambu printer's FTPS server listens on.
pub const FTPS_PORT: u16 = 990;

/// The username every Bambu printer expects.
const USERNAME: &str = "bblp";

/// How long to wait for a connection, TLS included.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for each reply on the control connection. The one after
/// the transfer may come once the printer has written the file to its card.
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);
/// How long one chunk of the file may take to send.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// The file is sent in chunks this big, each with its own timeout.
const CHUNK: usize = 64 << 10;
/// The longest reply line read, so a broken server can't fill memory.
const MAX_LINE: usize = 4096;

/// Why an upload failed.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("could not reach the printer's FTPS server: {0}")]
    Unreachable(String),
    #[error("TLS with the printer's FTPS server failed: {0}")]
    Tls(String),
    #[error("the printer refused the access code for FTPS")]
    LoginRefused,
    /// The server answered a step with an error, in its own words.
    #[error("the printer refused the upload: {0}")]
    Rejected(String),
    #[error("the printer's FTPS server didn't answer in time")]
    TimedOut,
    #[error("the printer's FTPS server said something unexpected: {0}")]
    Protocol(String),
}

/// Where and how to log in. Never `Debug`: it holds the access code.
pub struct Target {
    pub host: String,
    pub port: u16,
    pub access_code: String,
}

/// One reply: its code and the whole text, every line.
struct Reply {
    code: u16,
    text: String,
}

impl Reply {
    fn is(&self, class: u16) -> bool {
        self.code / 100 == class
    }
}

/// The control connection.
struct Control {
    stream: BufReader<TlsStream<TcpStream>>,
}

impl Control {
    async fn read(&mut self) -> Result<Reply, UploadError> {
        let first = self.line().await?;
        let code = reply_code(&first)
            .ok_or_else(|| UploadError::Protocol(format!("not an FTP reply: {first}")))?;
        let mut text = first.clone();
        // A multi-line reply runs from `123-` to `123 `.
        if first.as_bytes().get(3) == Some(&b'-') {
            let end = format!("{code} ");
            loop {
                let line = self.line().await?;
                text.push('\n');
                text.push_str(&line);
                if line.starts_with(&end) {
                    break;
                }
            }
        }
        Ok(Reply { code, text })
    }

    async fn line(&mut self) -> Result<String, UploadError> {
        let mut line = Vec::new();
        let read = tokio::time::timeout(REPLY_TIMEOUT, async {
            loop {
                let buf = self.stream.fill_buf().await?;
                if buf.is_empty() {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                let (take, done) = match buf.iter().position(|b| *b == b'\n') {
                    Some(at) => (at + 1, true),
                    None => (buf.len(), false),
                };
                line.extend_from_slice(&buf[..take]);
                self.stream.consume(take);
                if done {
                    return Ok(());
                }
                if line.len() > MAX_LINE {
                    return Err(std::io::Error::other("reply line too long"));
                }
            }
        })
        .await;
        match read {
            Err(_) => Err(UploadError::TimedOut),
            Ok(Err(e)) => Err(UploadError::Protocol(format!(
                "the control connection failed: {e}"
            ))),
            Ok(Ok(())) => Ok(String::from_utf8_lossy(&line).trim_end().to_owned()),
        }
    }

    async fn send(&mut self, command: &str) -> Result<(), UploadError> {
        let line = format!("{command}\r\n");
        let stream = self.stream.get_mut();
        tokio::time::timeout(WRITE_TIMEOUT, async {
            stream.write_all(line.as_bytes()).await?;
            stream.flush().await
        })
        .await
        .map_err(|_| UploadError::TimedOut)?
        .map_err(|e| UploadError::Protocol(format!("the control connection failed: {e}")))
    }

    /// Sends a command and reads its reply.
    async fn command(&mut self, command: &str) -> Result<Reply, UploadError> {
        self.send(command).await?;
        self.read().await
    }

    /// Sends a command whose reply must be a success (2xx).
    async fn expect_ok(&mut self, command: &str) -> Result<Reply, UploadError> {
        let reply = self.command(command).await?;
        if reply.is(2) {
            Ok(reply)
        } else {
            Err(UploadError::Rejected(reply.text))
        }
    }
}

/// The three-digit code a reply line starts with.
fn reply_code(line: &str) -> Option<u16> {
    let digits = line.get(..3)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match line.as_bytes().get(3) {
        None | Some(b' ' | b'-') => digits.parse().ok(),
        _ => None,
    }
}

/// The port from an `EPSV` reply: `229 Entering Extended Passive Mode (|||6446|)`.
fn epsv_port(text: &str) -> Option<u16> {
    let open = text.find('(')?;
    let inner = &text[open + 1..text[open..].find(')')? + open];
    let delimiter = inner.chars().next()?;
    let fields: Vec<&str> = inner.split(delimiter).collect();
    // `|||port|` splits into ["", "", "", "port", ""].
    fields.get(3)?.parse().ok().filter(|p| *p != 0)
}

/// The port from a `PASV` reply: `227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)`.
/// Its address is ignored: the data connection goes to the printer's own.
fn pasv_port(text: &str) -> Option<u16> {
    let open = text.find('(')?;
    let close = text[open..].find(')')? + open;
    let numbers: Vec<u16> = text[open + 1..close]
        .split(',')
        .map(|n| n.trim().parse().ok().filter(|n| *n <= 255))
        .collect::<Option<_>>()?;
    let [_, _, _, _, high, low] = numbers.as_slice() else {
        return None;
    };
    Some(high * 256 + low).filter(|p| *p != 0)
}

/// The file's name on the printer: the last part of the path, with nothing
/// that could end the FTP command line.
pub fn file_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    (!clean.is_empty() && clean != "." && clean != "..").then(|| clean.to_owned())
}

async fn connect_tls(
    connector: &TlsConnector,
    server_name: &ServerName<'static>,
    host: &str,
    port: u16,
) -> Result<TlsStream<TcpStream>, UploadError> {
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| UploadError::TimedOut)?
        .map_err(|e| UploadError::Unreachable(format!("port {port}: {}", e.kind())))?;
    handshake(connector, server_name, tcp).await
}

async fn handshake(
    connector: &TlsConnector,
    server_name: &ServerName<'static>,
    tcp: TcpStream,
) -> Result<TlsStream<TcpStream>, UploadError> {
    tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name.clone(), tcp))
        .await
        .map_err(|_| UploadError::TimedOut)?
        .map_err(|e| UploadError::Tls(e.to_string()))
}

/// Writes the whole file, a chunk at a time, then closes TLS (close_notify)
/// and the connection.
async fn send_file<S: AsyncRead + AsyncWrite + Unpin>(
    data: &mut S,
    bytes: &[u8],
) -> Result<(), UploadError> {
    let failed = |e: std::io::Error| UploadError::Protocol(format!("the transfer failed: {e}"));
    for chunk in bytes.chunks(CHUNK) {
        tokio::time::timeout(WRITE_TIMEOUT, data.write_all(chunk))
            .await
            .map_err(|_| UploadError::TimedOut)?
            .map_err(failed)?;
    }
    tokio::time::timeout(WRITE_TIMEOUT, async {
        data.flush().await?;
        data.shutdown().await
    })
    .await
    .map_err(|_| UploadError::TimedOut)?
    .map_err(failed)
}

impl Target {
    /// Uploads `bytes` as `name` to the root of the printer's SD card.
    pub async fn upload(&self, name: &str, bytes: &[u8]) -> Result<(), UploadError> {
        let config: Arc<rustls::ClientConfig> = crate::tls::client_config();
        let connector = TlsConnector::from(config);
        let server_name = ServerName::try_from(self.host.clone())
            .map_err(|_| UploadError::Unreachable("the address is not a host name".into()))?;

        let tls = connect_tls(&connector, &server_name, &self.host, self.port).await?;
        let mut control = Control {
            stream: BufReader::new(tls),
        };
        let greeting = control.read().await?;
        if !greeting.is(2) {
            return Err(UploadError::Rejected(greeting.text));
        }

        let mut login = control.command(&format!("USER {USERNAME}")).await?;
        if login.code == 331 {
            login = control
                .command(&format!("PASS {}", self.access_code))
                .await?;
        }
        match login.code {
            530 => return Err(UploadError::LoginRefused),
            code if code / 100 != 2 => return Err(UploadError::Rejected(login.text)),
            _ => {}
        }

        control.expect_ok("TYPE I").await?;
        control.expect_ok("PBSZ 0").await?;
        control.expect_ok("PROT P").await?;

        let epsv = control.command("EPSV").await?;
        let port = if epsv.code == 229 {
            epsv_port(&epsv.text)
        } else {
            let pasv = control.command("PASV").await?;
            if pasv.code != 227 {
                return Err(UploadError::Rejected(pasv.text));
            }
            pasv_port(&pasv.text)
        }
        .ok_or_else(|| UploadError::Protocol("no data port in the passive reply".into()))?;

        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((&*self.host, port)))
            .await
            .map_err(|_| UploadError::TimedOut)?
            .map_err(|e| UploadError::Unreachable(format!("data port {port}: {}", e.kind())))?;

        let stor = control.command(&format!("STOR {name}")).await?;
        if !stor.is(1) {
            return Err(UploadError::Rejected(stor.text));
        }
        // The same config and server name, so the session is resumed.
        let mut data = handshake(&connector, &server_name, tcp).await?;
        send_file(&mut data, bytes).await?;
        drop(data);

        let done = control.read().await?;
        if !done.is(2) {
            return Err(UploadError::Rejected(done.text));
        }
        // The file is stored; a failed goodbye doesn't change that.
        let _ = control.command("QUIT").await;
        tracing::debug!(bytes = bytes.len(), "uploaded a Print File over FTPS");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reply_codes() {
        assert_eq!(reply_code("220 ready"), Some(220));
        assert_eq!(reply_code("230-welcome"), Some(230));
        assert_eq!(reply_code("226"), Some(226));
        assert_eq!(reply_code("22x no"), None);
        assert_eq!(reply_code("2200 no"), None);
        assert_eq!(reply_code("hi"), None);
    }

    #[test]
    fn reads_passive_ports() {
        assert_eq!(
            epsv_port("229 Entering Extended Passive Mode (|||6446|)"),
            Some(6446)
        );
        assert_eq!(
            epsv_port("229 Extended Passive Mode (!!!50000!)"),
            Some(50000)
        );
        assert_eq!(epsv_port("229 nothing"), None);
        assert_eq!(
            pasv_port("227 Entering Passive Mode (192,168,1,50,195,80)."),
            Some(195 * 256 + 80)
        );
        assert_eq!(pasv_port("227 Entering Passive Mode (1,2,3,4,256,1)"), None);
        assert_eq!(pasv_port("227 Entering Passive Mode (1,2,3)"), None);
    }

    #[test]
    fn file_names_lose_their_path_and_control_characters() {
        assert_eq!(
            file_name("cube.gcode.3mf").as_deref(),
            Some("cube.gcode.3mf")
        );
        assert_eq!(file_name("../../etc/cube.3mf").as_deref(), Some("cube.3mf"));
        assert_eq!(
            file_name("C:\\prints\\cube.gcode").as_deref(),
            Some("cube.gcode")
        );
        assert_eq!(
            file_name("cube\r\nDELE x.3mf").as_deref(),
            Some("cubeDELE x.3mf")
        );
        assert_eq!(file_name("prints/"), None);
        assert_eq!(file_name(".."), None);
    }
}
