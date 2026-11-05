//! The address of a Gantry server, as someone types it into the desktop app.
use url::Url;

/// Why what was typed is not a server address.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServerUrlError {
    #[error("enter the address of your Gantry server, such as http://192.168.1.20:8080")]
    Empty,
    #[error("`{0}` is not a web address")]
    Invalid(String),
    #[error("the address must start with http:// or https://, not {0}://")]
    Scheme(String),
    #[error("leave the username and password out of the address: you log in on the next page")]
    Credentials,
}

/// A Gantry server's base address: `http` or `https`, a host, an optional
/// port and path (for a server behind a proxy at a sub-path), and nothing
/// else. It never ends with a slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerUrl(Url);

impl ServerUrl {
    /// Reads what someone typed. A bare host or `host:port` means `http://`,
    /// as a Gantry server on the LAN usually is; a query or fragment is
    /// dropped.
    pub fn parse(input: &str) -> Result<Self, ServerUrlError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(ServerUrlError::Empty);
        }
        let with_scheme = if input.contains("://") {
            input.to_owned()
        } else {
            format!("http://{input}")
        };
        let mut url =
            Url::parse(&with_scheme).map_err(|_| ServerUrlError::Invalid(input.to_owned()))?;
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(ServerUrlError::Scheme(other.to_owned())),
        }
        if url.host_str().is_none_or(str::is_empty) {
            return Err(ServerUrlError::Invalid(input.to_owned()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ServerUrlError::Credentials);
        }
        url.set_query(None);
        url.set_fragment(None);
        let path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&path);
        Ok(Self(url))
    }

    /// The address of `path` (which starts with `/`) on this server.
    pub fn join(&self, path: &str) -> Url {
        let mut url = self.0.clone();
        let base = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{base}{path}"));
        url
    }

    /// The dashboard's own page, where the window goes once connected.
    pub fn home(&self) -> Url {
        self.join("/")
    }

    /// The server's origin, such as `http://192.168.1.20:8080`.
    pub fn origin(&self) -> String {
        self.0.origin().ascii_serialization()
    }

    /// The URL pattern a Tauri capability uses to mean every page of this
    /// server's origin.
    pub fn capability_pattern(&self) -> String {
        format!("{}/*", self.origin())
    }

    /// Whether `url` is a page of this server's origin.
    pub fn same_origin(&self, url: &Url) -> bool {
        url.origin() == self.0.origin()
    }
}

impl std::fmt::Display for ServerUrl {
    /// The address with no trailing slash, as it is saved.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str().trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> String {
        ServerUrl::parse(s).unwrap().to_string()
    }

    #[test]
    fn a_bare_host_and_port_means_http() {
        assert_eq!(parse("192.168.1.20:8080"), "http://192.168.1.20:8080");
        assert_eq!(parse("  gantry.local "), "http://gantry.local");
    }

    #[test]
    fn https_and_a_sub_path_are_kept_without_a_trailing_slash() {
        assert_eq!(
            parse("https://gantry.example.com/"),
            "https://gantry.example.com"
        );
        assert_eq!(
            parse("https://example.com/gantry/?x=1#top"),
            "https://example.com/gantry"
        );
    }

    #[test]
    fn pages_are_joined_under_the_sub_path() {
        let url = ServerUrl::parse("https://example.com/gantry/").unwrap();
        assert_eq!(
            url.join("/healthz").as_str(),
            "https://example.com/gantry/healthz"
        );
        assert_eq!(url.home().as_str(), "https://example.com/gantry/");
        let root = ServerUrl::parse("http://pi:8080").unwrap();
        assert_eq!(
            root.join("/api/session").as_str(),
            "http://pi:8080/api/session"
        );
        assert_eq!(root.home().as_str(), "http://pi:8080/");
    }

    #[test]
    fn the_origin_and_capability_pattern_leave_out_the_path() {
        let url = ServerUrl::parse("https://example.com:8443/gantry").unwrap();
        assert_eq!(url.origin(), "https://example.com:8443");
        assert_eq!(url.capability_pattern(), "https://example.com:8443/*");
        // The default port is not written.
        let url = ServerUrl::parse("http://pi:80").unwrap();
        assert_eq!(url.origin(), "http://pi");
    }

    #[test]
    fn same_origin_compares_scheme_host_and_port() {
        let url = ServerUrl::parse("http://pi:8080").unwrap();
        assert!(url.same_origin(&Url::parse("http://pi:8080/printers/1").unwrap()));
        assert!(!url.same_origin(&Url::parse("http://pi:8081/").unwrap()));
        assert!(!url.same_origin(&Url::parse("https://pi:8080/").unwrap()));
        assert!(!url.same_origin(&Url::parse("http://evil.example/").unwrap()));
    }

    #[test]
    fn other_schemes_empty_input_and_credentials_are_refused() {
        assert_eq!(ServerUrl::parse("   "), Err(ServerUrlError::Empty));
        assert_eq!(
            ServerUrl::parse("ftp://pi"),
            Err(ServerUrlError::Scheme("ftp".into()))
        );
        assert_eq!(
            ServerUrl::parse("file:///etc/passwd"),
            Err(ServerUrlError::Scheme("file".into()))
        );
        assert_eq!(
            ServerUrl::parse("http://admin:hunter2@pi:8080"),
            Err(ServerUrlError::Credentials)
        );
        assert!(matches!(
            ServerUrl::parse("http://"),
            Err(ServerUrlError::Invalid(_))
        ));
        assert!(matches!(
            ServerUrl::parse("not a host"),
            Err(ServerUrlError::Invalid(_))
        ));
    }
}
