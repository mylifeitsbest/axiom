//! Разбор и строгая проверка прокси для панели ИИ.
//!
//! Строгость важна: wry подставляет хост и порт прямо в аргументы запуска
//! WebView2 (`--proxy-server=...`), поэтому пробелы и лишние символы недопустимы.

use thiserror::Error;
use wry::{ProxyConfig, ProxyEndpoint};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProxyError {
    #[error("поддерживаются только http:// и socks5://")]
    Scheme,
    #[error("неверный хост")]
    Host,
    #[error("неверный порт")]
    Port,
}

/// Пустая строка — прокси не используется.
pub fn parse(input: &str) -> Result<Option<ProxyConfig>, ProxyError> {
    let s = input.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let (scheme, rest) = match s.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("http".to_string(), s),
    };
    let rest = rest.trim_end_matches('/');
    let (host, port, bracketed) = split_host_port(rest).ok_or(ProxyError::Port)?;
    validate_host(host, bracketed)?;
    let port_num: u16 = port.parse().map_err(|_| ProxyError::Port)?;
    if port_num == 0 {
        return Err(ProxyError::Port);
    }
    let endpoint = ProxyEndpoint {
        // IPv6 для Chromium записывается в квадратных скобках.
        host: if bracketed {
            format!("[{host}]")
        } else {
            host.to_string()
        },
        port: port_num.to_string(),
    };
    match scheme.as_str() {
        "http" => Ok(Some(ProxyConfig::Http(endpoint))),
        "socks5" | "socks5h" | "socks" => Ok(Some(ProxyConfig::Socks5(endpoint))),
        _ => Err(ProxyError::Scheme),
    }
}

/// Короткий стабильный ключ для папки данных панели ИИ.
pub fn data_key(input: &str) -> String {
    let normalized = input.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return "direct".into();
    }
    // FNV-1a: детерминированный хеш без зависимостей.
    let hash = normalized.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// `host:port` или `[ipv6]:port` → (хост, порт, был ли в скобках).
fn split_host_port(s: &str) -> Option<(&str, &str, bool)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':')?;
        return Some((host, port, true));
    }
    let (host, port) = s.rsplit_once(':')?;
    Some((host, port, false))
}

fn validate_host(host: &str, bracketed: bool) -> Result<(), ProxyError> {
    let ok = if bracketed {
        !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
    } else {
        !host.is_empty()
            && host.len() <= 253
            && !host.starts_with('-')
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    };
    if ok { Ok(()) } else { Err(ProxyError::Host) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(cfg: Option<ProxyConfig>) -> Option<(String, String, bool)> {
        match cfg? {
            ProxyConfig::Http(e) => Some((e.host, e.port, false)),
            ProxyConfig::Socks5(e) => Some((e.host, e.port, true)),
        }
    }

    #[test]
    fn valid() {
        assert_eq!(parse("").ok().map(endpoint), Some(None));
        assert_eq!(
            parse("socks5://127.0.0.1:1080").ok().and_then(endpoint),
            Some(("127.0.0.1".into(), "1080".into(), true))
        );
        assert_eq!(
            parse("proxy.example.com:3128").ok().and_then(endpoint),
            Some(("proxy.example.com".into(), "3128".into(), false))
        );
        assert_eq!(
            parse("http://[::1]:8080/").ok().and_then(endpoint),
            Some(("[::1]".into(), "8080".into(), false))
        );
    }

    #[test]
    fn invalid() {
        assert_eq!(parse("ftp://a:1").err(), Some(ProxyError::Scheme));
        assert_eq!(
            parse("socks5://1.2.3.4 --flag:1080").err(),
            Some(ProxyError::Host)
        );
        assert_eq!(parse("http://host:99999").err(), Some(ProxyError::Port));
        assert_eq!(parse("http://host").err(), Some(ProxyError::Port));
        assert_eq!(parse("http://user@host:80").err(), Some(ProxyError::Host));
    }

    #[test]
    fn keys_are_stable() {
        assert_eq!(data_key(""), "direct");
        assert_eq!(data_key("socks5://A:1"), data_key(" socks5://a:1 "));
        assert_ne!(data_key("socks5://a:1"), data_key("socks5://a:2"));
    }
}
