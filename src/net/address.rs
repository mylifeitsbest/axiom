//! Адрес прокси из настроек: разбор и строгая проверка.
//!
//! Строгость важна: из адреса релей соединяется с прокси, а имя показывается
//! в интерфейсе, поэтому пробелы, логин и лишние символы недопустимы.

use thiserror::Error;

use super::relay::{Route, Upstream};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProxyError {
    #[error("поддерживаются только http:// и socks5://")]
    Scheme,
    #[error("неверный хост (логин и пароль не поддерживаются)")]
    Host,
    #[error("неверный порт")]
    Port,
}

/// Пустая строка — прокси нет. Без схемы — `http://`.
pub fn parse(input: &str) -> Result<Option<Route>, ProxyError> {
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
    let port: u16 = port.parse().map_err(|_| ProxyError::Port)?;
    if port == 0 {
        return Err(ProxyError::Port);
    }
    let upstream = Upstream {
        host: host.to_string(),
        port,
    };
    match scheme.as_str() {
        "http" => Ok(Some(Route::Http(upstream))),
        "socks5" | "socks5h" | "socks" => Ok(Some(Route::Socks5(upstream))),
        _ => Err(ProxyError::Scheme),
    }
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

    fn upstream(route: Option<Route>) -> Option<(String, u16, bool)> {
        match route? {
            Route::Http(u) => Some((u.host, u.port, false)),
            Route::Socks5(u) => Some((u.host, u.port, true)),
            Route::Direct => None,
        }
    }

    #[test]
    fn valid() {
        assert_eq!(parse("").ok().map(upstream), Some(None));
        assert_eq!(
            parse("socks5://127.0.0.1:18080").ok().and_then(upstream),
            Some(("127.0.0.1".into(), 18080, true))
        );
        assert_eq!(
            parse(" SOCKS5H://warp.local:1080/ ")
                .ok()
                .and_then(upstream),
            Some(("warp.local".into(), 1080, true))
        );
        assert_eq!(
            parse("proxy.example.com:3128").ok().and_then(upstream),
            Some(("proxy.example.com".into(), 3128, false))
        );
        // IPv6 релею нужен без скобок.
        assert_eq!(
            parse("http://[::1]:8080/").ok().and_then(upstream),
            Some(("::1".into(), 8080, false))
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
        assert_eq!(parse("http://host:0").err(), Some(ProxyError::Port));
        assert_eq!(parse("http://host").err(), Some(ProxyError::Port));
        assert_eq!(parse("http://user@host:80").err(), Some(ProxyError::Host));
    }
}
