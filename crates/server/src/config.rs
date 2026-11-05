//! Server settings: a small TOML file, every setting overridable by an
//! environment variable. It holds only what the server needs to start. Printers,
//! users and everything else live in SQLite (ADR-0004).
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::client::TrustedProxies;
use crate::crypto::SecretKey;
use crate::store::users::LoginLimits;

/// The default config file, read when it exists and no other is named.
pub const DEFAULT_CONFIG_FILE: &str = "gantry.toml";

/// How log lines are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

impl LogFormat {
    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(ConfigError::Invalid {
                key: "log_format",
                message: format!("expected `text` or `json`, got `{other}`"),
            }),
        }
    }
}

/// The settings as written in the file. Every field is optional there.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    secret_key: Option<String>,
    log_format: Option<LogFormat>,
    secure_cookies: Option<bool>,
    login_max_failures: Option<u32>,
    login_lockout_minutes: Option<u64>,
    login_slow_after: Option<u32>,
    login_client_attempts_per_minute: Option<u32>,
    /// Replaced by `trusted_proxies`; kept only to say so.
    trust_forwarded_for: Option<bool>,
    trusted_proxies: Option<Vec<String>>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    public_url: Option<String>,
    metrics_token: Option<String>,
    metrics_bind: Option<String>,
}

/// The certificate chain and private key Gantry serves HTTPS with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// The server settings. The secret key is checked separately, by
/// [`Config::require_secret_key`], so commands that don't need it still run.
#[derive(Clone)]
pub struct Config {
    /// Where the HTTP server listens. `GANTRY_BIND`.
    pub bind: SocketAddr,
    /// Where the SQLite database lives. `GANTRY_DATA_DIR`.
    pub data_dir: PathBuf,
    /// Encrypts Printer credentials. `GANTRY_SECRET_KEY`. Required to serve.
    pub secret_key: Option<String>,
    /// `GANTRY_LOG_FORMAT`: `text` or `json`.
    pub log_format: LogFormat,
    /// Always mark session cookies `Secure`, for when a TLS-terminating proxy
    /// doesn't send `X-Forwarded-Proto`. `GANTRY_SECURE_COOKIES`.
    pub secure_cookies: bool,
    /// Login rate limiting and lockout (PRD §9.2): `GANTRY_LOGIN_MAX_FAILURES`,
    /// `GANTRY_LOGIN_LOCKOUT_MINUTES`, `GANTRY_LOGIN_SLOW_AFTER` and
    /// `GANTRY_LOGIN_CLIENT_ATTEMPTS_PER_MINUTE`.
    pub login: LoginLimits,
    /// The proxies and tunnels whose `X-Forwarded-For` and
    /// `X-Forwarded-Proto` are believed. `GANTRY_TRUSTED_PROXIES`, a
    /// comma-separated list of addresses and CIDR ranges. Empty by default.
    pub trusted_proxies: TrustedProxies,
    /// Serve HTTPS directly with this certificate and key.
    /// `GANTRY_TLS_CERT` and `GANTRY_TLS_KEY`, both or neither.
    pub tls: Option<TlsFiles>,
    /// The address people reach Gantry at, such as `https://gantry.example.com`,
    /// with no trailing slash. Link previews (Open Graph and X cards) need
    /// absolute URLs; without it they are worked out from each request's
    /// `Host`. `GANTRY_PUBLIC_URL`.
    pub public_url: Option<String>,
    /// Prometheus metrics (PRD §9.2), off by default. `GANTRY_METRICS_TOKEN`:
    /// serve `/metrics` beside the dashboard, to requests that bring this
    /// bearer token.
    pub metrics_token: Option<String>,
    /// `GANTRY_METRICS_BIND`: serve `/metrics` on its own address instead,
    /// such as an internal-only port. It needs the token too, if one is set.
    pub metrics_bind: Option<SocketAddr>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("data_dir", &self.data_dir)
            .field("secret_key", &self.secret_key.as_ref().map(|_| "<set>"))
            .field("log_format", &self.log_format)
            .field("secure_cookies", &self.secure_cookies)
            .field("login", &self.login)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("tls", &self.tls)
            .field("public_url", &self.public_url)
            .field(
                "metrics_token",
                &self.metrics_token.as_ref().map(|_| "<set>"),
            )
            .field("metrics_bind", &self.metrics_bind)
            .finish()
    }
}

/// Why the config could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read the config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the config file {path} is not valid: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("`{key}` is not valid: {message}")]
    Invalid { key: &'static str, message: String },
    #[error(
        "no secret key is set, and Gantry needs one to encrypt Printer credentials.\n\
         Supply it with the GANTRY_SECRET_KEY environment variable, or `secret_key` in the config file.\n\
         Generate one with: openssl rand -base64 32"
    )]
    MissingSecretKey,
    #[error(
        "the secret key is too short: use at least {min} characters (try `openssl rand -base64 32`)"
    )]
    WeakSecretKey { min: usize },
}

impl Config {
    /// Loads the config from `path` (or `gantry.toml` if it exists), then
    /// applies environment overrides from `env`.
    pub fn load(
        path: Option<&Path>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let file = match path {
            Some(path) => read_file(path)?,
            None if Path::new(DEFAULT_CONFIG_FILE).exists() => {
                read_file(Path::new(DEFAULT_CONFIG_FILE))?
            }
            None => FileConfig::default(),
        };

        let bind = env("GANTRY_BIND")
            .or(file.bind)
            .unwrap_or_else(|| "127.0.0.1:8080".to_owned());
        let bind = bind.parse().map_err(|e| ConfigError::Invalid {
            key: "bind",
            message: format!("`{bind}` is not an address and port: {e}"),
        })?;
        let data_dir = env("GANTRY_DATA_DIR")
            .map(PathBuf::from)
            .or(file.data_dir)
            .unwrap_or_else(|| PathBuf::from("data"));
        let secret_key = env("GANTRY_SECRET_KEY")
            .or(file.secret_key)
            .filter(|k| !k.trim().is_empty());
        let log_format = match env("GANTRY_LOG_FORMAT") {
            Some(v) => LogFormat::parse(&v)?,
            None => file.log_format.unwrap_or_default(),
        };
        let secure_cookies = match env("GANTRY_SECURE_COOKIES") {
            Some(v) => parse_bool("secure_cookies", &v)?,
            None => file.secure_cookies.unwrap_or(false),
        };

        let defaults = LoginLimits::default();
        let number =
            |key: &'static str, var: &str, from_file: Option<u64>, default: u64| match env(var) {
                Some(v) => v.trim().parse::<u64>().map_err(|_| ConfigError::Invalid {
                    key,
                    message: format!("expected a whole number, got `{v}`"),
                }),
                None => Ok(from_file.unwrap_or(default)),
            };
        let small = |key: &'static str, value: u64| {
            u32::try_from(value)
                .ok()
                .filter(|v| *v > 0)
                .ok_or(ConfigError::Invalid {
                    key,
                    message: "must be at least 1".into(),
                })
        };
        let login = LoginLimits {
            max_failures: small(
                "login_max_failures",
                number(
                    "login_max_failures",
                    "GANTRY_LOGIN_MAX_FAILURES",
                    file.login_max_failures.map(u64::from),
                    defaults.max_failures.into(),
                )?,
            )?,
            lockout: std::time::Duration::from_secs(
                60 * number(
                    "login_lockout_minutes",
                    "GANTRY_LOGIN_LOCKOUT_MINUTES",
                    file.login_lockout_minutes,
                    defaults.lockout.as_secs() / 60,
                )?,
            ),
            slow_after: small(
                "login_slow_after",
                number(
                    "login_slow_after",
                    "GANTRY_LOGIN_SLOW_AFTER",
                    file.login_slow_after.map(u64::from),
                    defaults.slow_after.into(),
                )?,
            )?,
            client_attempts_per_minute: small(
                "login_client_attempts_per_minute",
                number(
                    "login_client_attempts_per_minute",
                    "GANTRY_LOGIN_CLIENT_ATTEMPTS_PER_MINUTE",
                    file.login_client_attempts_per_minute.map(u64::from),
                    defaults.client_attempts_per_minute.into(),
                )?,
            )?,
        };
        if env("GANTRY_TRUST_FORWARDED_FOR").is_some() || file.trust_forwarded_for.is_some() {
            return Err(ConfigError::Invalid {
                key: "trust_forwarded_for",
                message: "it has been replaced by `trusted_proxies` (GANTRY_TRUSTED_PROXIES): list the addresses of your proxy or tunnel, such as trusted_proxies = [\"127.0.0.1\"], and remove trust_forwarded_for"
                    .into(),
            });
        }
        let trusted_proxies = match env("GANTRY_TRUSTED_PROXIES") {
            Some(v) => TrustedProxies::parse_list(&v),
            None => TrustedProxies::parse_list(&file.trusted_proxies.unwrap_or_default().join(",")),
        }
        .map_err(|message| ConfigError::Invalid {
            key: "trusted_proxies",
            message,
        })?;
        let cert = env("GANTRY_TLS_CERT").map(PathBuf::from).or(file.tls_cert);
        let key = env("GANTRY_TLS_KEY").map(PathBuf::from).or(file.tls_key);
        let tls = match (cert, key) {
            (Some(cert), Some(key)) => Some(TlsFiles { cert, key }),
            (None, None) => None,
            (Some(_), None) | (None, Some(_)) => {
                return Err(ConfigError::Invalid {
                    key: "tls_cert",
                    message: "HTTPS needs both `tls_cert` and `tls_key` (GANTRY_TLS_CERT and GANTRY_TLS_KEY), or neither for plain HTTP"
                        .into(),
                });
            }
        };

        let public_url = env("GANTRY_PUBLIC_URL")
            .or(file.public_url)
            .filter(|u| !u.trim().is_empty())
            .map(|u| parse_public_url(&u))
            .transpose()?;

        let metrics_token = env("GANTRY_METRICS_TOKEN")
            .or(file.metrics_token)
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        if metrics_token.as_ref().is_some_and(|t| t.len() < 16) {
            return Err(ConfigError::Invalid {
                key: "metrics_token",
                message: "use at least 16 characters (try `openssl rand -hex 24`)".into(),
            });
        }
        let metrics_bind = env("GANTRY_METRICS_BIND")
            .or(file.metrics_bind)
            .filter(|b| !b.trim().is_empty())
            .map(|b| {
                b.trim().parse().map_err(|e| ConfigError::Invalid {
                    key: "metrics_bind",
                    message: format!("`{b}` is not an address and port: {e}"),
                })
            })
            .transpose()?;

        Ok(Self {
            bind,
            data_dir,
            secret_key,
            log_format,
            secure_cookies,
            login,
            trusted_proxies,
            tls,
            public_url,
            metrics_token,
            metrics_bind,
        })
    }

    /// Loads with the process environment.
    pub fn from_process(path: Option<&Path>) -> Result<Self, ConfigError> {
        Self::load(path, |k| std::env::var(k).ok())
    }

    /// The secret key, or the error that says how to supply one.
    pub fn require_secret_key(&self) -> Result<SecretKey, ConfigError> {
        let raw = self
            .secret_key
            .as_deref()
            .ok_or(ConfigError::MissingSecretKey)?;
        SecretKey::from_config(raw).ok_or(ConfigError::WeakSecretKey {
            min: SecretKey::MIN_LEN,
        })
    }

    /// The SQLite database file.
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("gantry.db")
    }
}

fn read_file(path: &Path) -> Result<FileConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    toml::from_str(&text).map_err(|e| ConfigError::Parse {
        path: path.to_owned(),
        message: e.message().to_owned(),
    })
}

/// An `http` or `https` origin, optionally with a path, and no trailing slash.
/// It is written into the page's `<head>`, so only URL characters are allowed.
fn parse_public_url(value: &str) -> Result<String, ConfigError> {
    let url = value.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    let invalid = |message: &str| ConfigError::Invalid {
        key: "public_url",
        message: format!("{message}, got `{value}`"),
    };
    let rest = rest.ok_or_else(|| invalid("expected it to start with https:// or http://"))?;
    if rest.is_empty() || rest.starts_with('/') {
        return Err(invalid("expected a host after the scheme"));
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || "-._~:/[]%".contains(c);
    if !rest.chars().all(allowed) {
        return Err(invalid("expected only a scheme, host, port and path"));
    }
    Ok(url.to_owned())
}

fn parse_bool(key: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(ConfigError::Invalid {
            key,
            message: format!("expected true or false, got `{other}`"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.get(k).cloned()
    }

    const FULL: &str = r#"
        bind = "0.0.0.0:9000"
        data_dir = "/var/lib/gantry"
        secret_key = "from-the-file-0123456789"
        log_format = "json"
        secure_cookies = true
    "#;

    fn write(dir: &tempfile::TempDir, text: &str) -> PathBuf {
        let path = dir.path().join("gantry.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn metrics_are_off_by_default_and_need_a_strong_token() {
        let config = Config::load(None, env(&[])).unwrap();
        assert!(config.metrics_token.is_none());
        assert!(config.metrics_bind.is_none());
        let config = Config::load(
            None,
            env(&[
                ("GANTRY_METRICS_TOKEN", "a long enough metrics token"),
                ("GANTRY_METRICS_BIND", "127.0.0.1:9187"),
            ]),
        )
        .unwrap();
        assert_eq!(
            config.metrics_token.as_deref(),
            Some("a long enough metrics token")
        );
        assert_eq!(config.metrics_bind, Some("127.0.0.1:9187".parse().unwrap()));
        assert!(!format!("{config:?}").contains("a long enough metrics token"));
        assert!(Config::load(None, env(&[("GANTRY_METRICS_TOKEN", "short")])).is_err());
        assert!(Config::load(None, env(&[("GANTRY_METRICS_BIND", "nowhere")])).is_err());
    }

    #[test]
    fn reads_every_setting_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(Some(&write(&dir, FULL)), env(&[])).unwrap();
        assert_eq!(config.bind, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/gantry"));
        assert_eq!(
            config.secret_key.as_deref(),
            Some("from-the-file-0123456789")
        );
        assert_eq!(config.log_format, LogFormat::Json);
        assert!(config.secure_cookies);
    }

    #[test]
    fn environment_overrides_every_setting() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(
            Some(&write(&dir, FULL)),
            env(&[
                ("GANTRY_BIND", "127.0.0.1:7000"),
                ("GANTRY_DATA_DIR", "/tmp/elsewhere"),
                ("GANTRY_SECRET_KEY", "from-the-environment-0123"),
                ("GANTRY_LOG_FORMAT", "text"),
                ("GANTRY_SECURE_COOKIES", "false"),
            ]),
        )
        .unwrap();
        assert_eq!(config.bind, "127.0.0.1:7000".parse().unwrap());
        assert_eq!(config.data_dir, PathBuf::from("/tmp/elsewhere"));
        assert_eq!(
            config.secret_key.as_deref(),
            Some("from-the-environment-0123")
        );
        assert_eq!(config.log_format, LogFormat::Text);
        assert!(!config.secure_cookies);
    }

    #[test]
    fn the_file_holds_only_server_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "[[printers]]\nname = \"X1\"\n");
        let err = Config::load(Some(&path), env(&[])).err().unwrap();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
    }

    #[test]
    fn a_missing_secret_key_says_how_to_supply_one() {
        let config = Config::load(None, env(&[])).unwrap();
        let message = config.require_secret_key().unwrap_err().to_string();
        assert!(message.contains("GANTRY_SECRET_KEY"), "{message}");
        assert!(message.contains("secret_key"), "{message}");
    }

    #[test]
    fn a_short_secret_key_is_refused() {
        let config = Config::load(None, env(&[("GANTRY_SECRET_KEY", "short")])).unwrap();
        assert!(matches!(
            config.require_secret_key(),
            Err(ConfigError::WeakSecretKey { .. })
        ));
    }

    #[test]
    fn a_named_config_file_must_exist() {
        let err = Config::load(Some(Path::new("/no/such/gantry.toml")), env(&[]))
            .err()
            .unwrap();
        assert!(matches!(err, ConfigError::Read { .. }));
    }

    #[test]
    fn login_limits_are_configurable() {
        let config = Config::load(None, env(&[])).unwrap();
        assert_eq!(config.login, LoginLimits::default());
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "login_max_failures = 3\nlogin_lockout_minutes = 5\nlogin_slow_after = 2\n\
             login_client_attempts_per_minute = 9\n",
        );
        let config = Config::load(Some(&path), env(&[("GANTRY_LOGIN_MAX_FAILURES", "7")])).unwrap();
        assert_eq!(
            config.login,
            LoginLimits {
                max_failures: 7,
                lockout: std::time::Duration::from_secs(300),
                slow_after: 2,
                client_attempts_per_minute: 9,
            }
        );
        assert!(Config::load(None, env(&[("GANTRY_LOGIN_MAX_FAILURES", "0")])).is_err());
        assert!(Config::load(None, env(&[("GANTRY_LOGIN_SLOW_AFTER", "lots")])).is_err());
    }

    #[test]
    fn trusted_proxies_and_tls_are_configurable() {
        let config = Config::load(None, env(&[])).unwrap();
        assert!(config.trusted_proxies.is_empty());
        assert_eq!(config.tls, None);
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "trusted_proxies = [\"127.0.0.1\", \"172.16.0.0/12\"]
             tls_cert = \"cert.pem\"
tls_key = \"key.pem\"
",
        );
        let config = Config::load(Some(&path), env(&[])).unwrap();
        assert!(config.trusted_proxies.trusts("172.20.0.3".parse().unwrap()));
        assert!(
            !config
                .trusted_proxies
                .trusts("192.168.1.2".parse().unwrap())
        );
        assert_eq!(
            config.tls,
            Some(TlsFiles {
                cert: "cert.pem".into(),
                key: "key.pem".into()
            })
        );
        let config =
            Config::load(Some(&path), env(&[("GANTRY_TRUSTED_PROXIES", "10.0.0.1")])).unwrap();
        assert!(config.trusted_proxies.trusts("10.0.0.1".parse().unwrap()));
        assert!(!config.trusted_proxies.trusts("127.0.0.1".parse().unwrap()));
        // Half a TLS setup, or a bad address, is refused.
        assert!(Config::load(None, env(&[("GANTRY_TLS_CERT", "cert.pem")])).is_err());
        assert!(Config::load(None, env(&[("GANTRY_TRUSTED_PROXIES", "nope")])).is_err());
    }

    #[test]
    fn the_old_trust_forwarded_for_setting_says_what_replaced_it() {
        let message = Config::load(None, env(&[("GANTRY_TRUST_FORWARDED_FOR", "true")]))
            .err()
            .unwrap()
            .to_string();
        assert!(message.contains("trusted_proxies"), "{message}");
    }

    #[test]
    fn the_public_url_is_checked_and_loses_its_trailing_slash() {
        let load = |url: &str| Config::load(None, env(&[("GANTRY_PUBLIC_URL", url)]));
        assert_eq!(
            load("https://gantry.example.com/")
                .unwrap()
                .public_url
                .as_deref(),
            Some("https://gantry.example.com")
        );
        assert_eq!(
            load("http://pi.local:8080/gantry")
                .unwrap()
                .public_url
                .as_deref(),
            Some("http://pi.local:8080/gantry")
        );
        assert!(load("").unwrap().public_url.is_none());
        for bad in [
            "gantry.example.com",
            "ftp://gantry.example.com",
            "https://",
            "https://x\"><script>",
        ] {
            assert!(
                matches!(
                    load(bad),
                    Err(ConfigError::Invalid {
                        key: "public_url",
                        ..
                    })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn debug_output_hides_the_secret_key() {
        let config =
            Config::load(None, env(&[("GANTRY_SECRET_KEY", "do-not-print-this-key")])).unwrap();
        assert!(!format!("{config:?}").contains("do-not-print-this-key"));
    }
}
