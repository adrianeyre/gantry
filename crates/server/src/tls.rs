//! Serving HTTPS directly with `rustls` (PRD §9.1), for when Gantry isn't
//! behind a TLS-terminating proxy or tunnel. Given a certificate chain and a
//! private key in PEM files, Gantry speaks HTTPS (HTTP/1.1 and HTTP/2) on its
//! one port. Without them it serves plain HTTP, as before.
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::server::graceful::GracefulShutdown;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::Service;

use crate::AppState;
use crate::client::DirectTls;

/// How long a client gets to finish the TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long open connections get to finish when Gantry stops.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Reads a PEM certificate chain and private key into a server config.
pub fn load(cert: &Path, key: &Path) -> anyhow::Result<Arc<rustls::ServerConfig>> {
    let chain = CertificateDer::pem_file_iter(cert)
        .with_context(|| format!("could not read the TLS certificate {}", cert.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("the TLS certificate {} is not valid PEM", cert.display()))?;
    if chain.is_empty() {
        anyhow::bail!(
            "the TLS certificate {} has no certificate in it",
            cert.display()
        );
    }
    let key = PrivateKeyDer::from_pem_file(key)
        .with_context(|| format!("could not read a private key from {}", key.display()))?;
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .context("the TLS provider supports no protocol versions")?
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .context("the TLS certificate and private key don't belong together")?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// Serves the app over HTTPS on `listener` until the process is told to stop.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    tls: Arc<rustls::ServerConfig>,
) -> std::io::Result<()> {
    serve_until(listener, state, tls, crate::shutdown_signal()).await
}

/// [`serve`], stopping when `stop` resolves.
pub async fn serve_until(
    listener: TcpListener,
    state: AppState,
    tls: Arc<rustls::ServerConfig>,
    stop: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let acceptor = TlsAcceptor::from(tls);
    let app: Router = crate::app(state.clone());
    let graceful = GracefulShutdown::new();
    tokio::pin!(stop);
    loop {
        let (tcp, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(e) => {
                    // Out of file descriptors and the like: wait, don't spin.
                    tracing::warn!(error = %e, "could not accept a connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            () = &mut stop => break,
        };
        let acceptor = acceptor.clone();
        let app = app.clone();
        let watcher = graceful.watcher();
        tokio::spawn(async move {
            let stream = match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    tracing::debug!(client = %peer, error = %e, "TLS handshake failed");
                    return;
                }
                Err(_) => {
                    tracing::debug!(client = %peer, "TLS handshake timed out");
                    return;
                }
            };
            let service = hyper::service::service_fn(
                move |mut request: axum::http::Request<hyper::body::Incoming>| {
                    request.extensions_mut().insert(ConnectInfo(peer));
                    request.extensions_mut().insert(DirectTls);
                    let mut app = app.clone();
                    async move {
                        Ok::<_, Infallible>(
                            app.call(request.map(axum::body::Body::new))
                                .await
                                .unwrap_or_else(|never| match never {}),
                        )
                    }
                },
            );
            let builder = Builder::new(TokioExecutor::new());
            let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
            if let Err(e) = watcher.watch(connection.into_owned()).await {
                tracing::debug!(client = %peer, error = %e, "connection ended with an error");
            }
        });
    }
    drop(listener);
    // End the streams that never finish on their own, then let requests drain.
    state.begin_shutdown();
    if tokio::time::timeout(DRAIN_TIMEOUT, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("some connections were still open when Gantry stopped");
    }
    Ok(())
}

/// The address a listener serves, for log lines.
pub fn describe(address: SocketAddr, tls: bool) -> String {
    format!("{}://{address}", if tls { "https" } else { "http" })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::store::users::Role;
    use crate::test_support;

    struct Generated {
        _dir: tempfile::TempDir,
        cert_path: std::path::PathBuf,
        key_path: std::path::PathBuf,
        cert: CertificateDer<'static>,
    }

    fn generate() -> Generated {
        let generated = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");
        std::fs::write(&cert_path, generated.cert.pem()).unwrap();
        std::fs::write(&key_path, generated.signing_key.serialize_pem()).unwrap();
        Generated {
            _dir: dir,
            cert_path,
            key_path,
            cert: generated.cert.der().clone(),
        }
    }

    /// Sends one HTTP/1.1 request over TLS, trusting only `cert`, and returns
    /// the raw response.
    async fn request(address: SocketAddr, cert: &CertificateDer<'static>, raw: String) -> String {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.clone()).unwrap();
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut tls = connector
            .connect("localhost".try_into().unwrap(), tcp)
            .await
            .unwrap();
        tls.write_all(raw.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        tls.read_to_end(&mut response).await.unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    #[tokio::test]
    async fn serves_https_with_the_configured_certificate_and_key() {
        let generated = generate();
        let tls = load(&generated.cert_path, &generated.key_path).unwrap();
        let state = test_support::state().await;
        state
            .store()
            .create_user("adrian", test_support::PASSWORD, Role::Admin)
            .await
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve_until(listener, state, tls, async {
            let _ = stop_rx.await;
        }));

        let health = request(
            address,
            &generated.cert,
            "GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n".into(),
        )
        .await;
        assert!(health.starts_with("HTTP/1.1 200"), "{health}");
        assert!(health.ends_with("ok"), "{health}");

        // A login over Gantry's own TLS gets a Secure cookie, with no
        // forwarded headers involved.
        let body = serde_json::json!({ "username": "adrian", "password": test_support::PASSWORD })
            .to_string();
        let login = request(
            address,
            &generated.cert,
            format!(
                "POST /api/login HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
        .await;
        assert!(login.starts_with("HTTP/1.1 200"), "{login}");
        let cookie = login
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("set-cookie:"))
            .unwrap();
        assert!(cookie.contains("; Secure"), "{cookie}");

        stop_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(15), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[test]
    fn a_missing_or_mismatched_key_says_what_is_wrong() {
        let generated = generate();
        let err = load(&generated.cert_path, Path::new("/no/such/key.pem"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("private key"), "{err}");
        let other = generate();
        let err = format!(
            "{:#}",
            load(&generated.cert_path, &other.key_path).unwrap_err()
        );
        assert!(err.contains("don't belong together"), "{err}");
        let err = load(&generated.key_path, &generated.key_path)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no certificate"), "{err}");
    }
}
