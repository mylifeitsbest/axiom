//! Разбор ввода адресной строки: адрес или поиск в DuckDuckGo.

use std::net::Ipv4Addr;

use url::Url;

use crate::ipc::MAX_INPUT;

/// Внутренний адрес новой вкладки. Такая вкладка не создаёт webview вообще.
pub const NEWTAB: &str = "axiom://newtab";
const SEARCH_BASE: &str = "https://duckduckgo.com/";
const AI_BASE: &str = "https://www.google.com/search";
/// Стартовая страница AI Mode без запроса.
pub const AI_HOME: &str = "https://www.google.com/search?udm=50";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Search(String),
}

impl Target {
    pub fn into_url(self) -> String {
        match self {
            Target::Url(u) => u,
            Target::Search(q) => search_url(&q),
        }
    }
}

pub fn resolve(input: &str) -> Option<Target> {
    let s = input.trim();
    if s.is_empty() || s.len() > MAX_INPUT {
        return None;
    }
    if s == NEWTAB {
        return Some(Target::Url(NEWTAB.into()));
    }
    if let Ok(u) = Url::parse(s) {
        match u.scheme() {
            "http" | "https" | "file" => return Some(Target::Url(u.into())),
            "about" if s == "about:blank" => return Some(Target::Url(s.into())),
            _ => {}
        }
    }
    if !s.chars().any(char::is_whitespace) && looks_like_host(s) {
        let scheme = if is_local(s) { "http" } else { "https" };
        if let Ok(u) = Url::parse(&format!("{scheme}://{s}"))
            && u.host().is_some()
        {
            return Some(Target::Url(u.into()));
        }
    }
    Some(Target::Search(s.to_string()))
}

pub fn search_url(query: &str) -> String {
    Url::parse_with_params(SEARCH_BASE, &[("q", query)])
        .map(String::from)
        .unwrap_or_else(|_| SEARCH_BASE.to_string())
}

pub fn ai_url(query: &str) -> String {
    Url::parse_with_params(AI_BASE, &[("udm", "50"), ("q", query)])
        .map(String::from)
        .unwrap_or_else(|_| AI_HOME.to_string())
}

/// Разрешённые схемы для навигации внутри вкладки.
pub fn is_navigable(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        scheme.as_str(),
        "http" | "https" | "about" | "data" | "blob" | "file"
    )
}

fn host_part(s: &str) -> &str {
    s.split(['/', '?', '#']).next().unwrap_or("")
}

fn strip_port(host_port: &str) -> &str {
    match host_port.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => host_port,
    }
}

fn is_local(s: &str) -> bool {
    let host = strip_port(host_part(s));
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<Ipv4Addr>()
            .is_ok_and(|ip| ip.is_private() || ip.is_loopback())
}

fn looks_like_host(s: &str) -> bool {
    let host_port = host_part(s);
    if host_port.starts_with('[') {
        return host_port.contains(']');
    }
    let host = strip_port(host_port);
    if host.eq_ignore_ascii_case("localhost") || host.parse::<Ipv4Addr>().is_ok() {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let Some(tld) = labels.last() else {
        return false;
    };
    labels.len() >= 2
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && tld.chars().count() >= 2
        && tld.chars().all(char::is_alphabetic)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Option<Target> {
        Some(Target::Url(s.to_string()))
    }

    #[test]
    fn urls() {
        assert_eq!(resolve("example.com"), url("https://example.com/"));
        assert_eq!(
            resolve("  https://rust-lang.org/learn "),
            url("https://rust-lang.org/learn")
        );
        assert_eq!(resolve("localhost:3000"), url("http://localhost:3000/"));
        assert_eq!(
            resolve("192.168.1.1/admin"),
            url("http://192.168.1.1/admin")
        );
        assert_eq!(resolve("пример.рф"), url("https://xn--e1afmkfd.xn--p1ai/"));
        assert_eq!(
            resolve("github.com/tauri-apps/wry"),
            url("https://github.com/tauri-apps/wry")
        );
        assert_eq!(resolve(NEWTAB), url(NEWTAB));
    }

    #[test]
    fn searches() {
        for q in [
            "rust webview",
            "javascript:alert(1)",
            "data:text/html,hi",
            "version 1.2",
            "c++",
            "a.b1",
        ] {
            assert_eq!(resolve(q), Some(Target::Search(q.to_string())), "{q}");
        }
        assert_eq!(resolve("   "), None);
    }

    #[test]
    fn builds_search_urls() {
        assert_eq!(
            search_url("как дела?"),
            "https://duckduckgo.com/?q=%D0%BA%D0%B0%D0%BA+%D0%B4%D0%B5%D0%BB%D0%B0%3F"
        );
        assert!(ai_url("rust").starts_with("https://www.google.com/search?udm=50&q=rust"));
    }

    #[test]
    fn navigable_schemes() {
        assert!(is_navigable("https://a.b"));
        assert!(!is_navigable("javascript:alert(1)"));
        assert!(!is_navigable("ms-settings:"));
    }
}
