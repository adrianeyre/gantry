//! Where the window may go. It shows the app's own page and the chosen Gantry
//! server's pages; any other web address opens in the system browser, so no
//! other site ever runs inside the app.
use url::Url;

use crate::server_url::ServerUrl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Navigation {
    /// Show it in the window.
    Allow,
    /// Open it in the system browser instead.
    OpenOutside,
    /// Go nowhere.
    Block,
}

/// Whether `url` is the app's own bundled page (`tauri://localhost` on macOS
/// and Linux, `http://tauri.localhost` on Windows).
pub fn is_app_page(url: &Url) -> bool {
    url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost")
}

/// Decides a navigation, given the server the window is connected to.
pub fn decide(url: &Url, server: Option<&ServerUrl>) -> Navigation {
    if is_app_page(url) || server.is_some_and(|s| s.same_origin(url)) {
        return Navigation::Allow;
    }
    match url.scheme() {
        // Downloads and blank frames the dashboard makes itself.
        "blob" => Navigation::Allow,
        "about" if url.path() == "blank" => Navigation::Allow,
        "http" | "https" | "mailto" => Navigation::OpenOutside,
        _ => Navigation::Block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn the_app_page_is_always_allowed() {
        assert_eq!(
            decide(&url("tauri://localhost/index.html"), None),
            Navigation::Allow
        );
        assert_eq!(
            decide(&url("http://tauri.localhost/index.html?choose"), None),
            Navigation::Allow
        );
    }

    #[test]
    fn only_the_chosen_server_opens_in_the_window() {
        let server = ServerUrl::parse("http://pi:8080").unwrap();
        assert_eq!(
            decide(&url("http://pi:8080/login"), Some(&server)),
            Navigation::Allow
        );
        assert_eq!(
            decide(&url("http://pi:8080/login"), None),
            Navigation::OpenOutside
        );
        assert_eq!(
            decide(&url("https://github.com/adrianeyre/gantry"), Some(&server)),
            Navigation::OpenOutside
        );
        assert_eq!(
            decide(&url("http://pi:9090/"), Some(&server)),
            Navigation::OpenOutside
        );
    }

    #[test]
    fn other_schemes_go_nowhere() {
        let server = ServerUrl::parse("http://pi:8080").unwrap();
        for bad in ["file:///C:/Windows/", "javascript:alert(1)", "ms-settings:"] {
            assert_eq!(decide(&url(bad), Some(&server)), Navigation::Block, "{bad}");
        }
        assert_eq!(
            decide(&url("about:blank"), Some(&server)),
            Navigation::Allow
        );
    }
}
