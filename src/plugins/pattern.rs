//! Шаблоны адресов плагинов: на каких страницах работает скрипт.
//!
//! Как `matches` в расширениях Chrome, плюс короткая запись сайтом:
//! - `<all_urls>` или `*` — любая страница http и https;
//! - `youtube.com`, `*.youtube.com` — сайт и все его поддомены, любой путь;
//! - `https://*.example.com/watch*` — полная запись: схема (`http`, `https`, `*` —
//!   обе), хост (`*`, `*.сайт` — сайт с поддоменами, `сайт` — ровно он), путь со
//!   звёздочками. Путь сравнивается вместе со строкой запроса (`/watch?v=…`).
//!   Порт не важен, без пути — любой путь.
//!
//! Страницы браузера (`axiom://`), `file:`, `data:` и прочее плагинам недоступны.
//!
//! Проверяют дважды: Rust — какой странице подключить канал плагина, и сам скрипт
//! плагина в странице (`JsPattern`) — запускаться ли ему. Правило одно, тесты ниже.

use serde::Serialize;
use url::Url;

/// Длиннее шаблон не нужен никому.
const MAX_PATTERN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    http: bool,
    https: bool,
    host: Host,
    /// Путь со строкой запроса, `*` — любые знаки. Всегда начинается с `/`.
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    Any,
    /// Ровно этот хост.
    Exact(String),
    /// Этот хост и все его поддомены.
    Domain(String),
}

impl Pattern {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        let bad = |why: &str| Err(format!("шаблон «{s}»: {why}"));
        if s.is_empty() {
            return bad("пустой");
        }
        if s.len() > MAX_PATTERN {
            return bad("слишком длинный");
        }
        if s.chars().any(char::is_whitespace) {
            return bad("пробелы внутри");
        }
        if s == "<all_urls>" || s == "*" {
            return Ok(Self::any());
        }
        let Some((scheme, rest)) = s.split_once("://") else {
            // Короткая запись: сайт с поддоменами, любой путь.
            let host = s.strip_prefix("*.").unwrap_or(s);
            if host.contains('/') {
                return bad("путь пишется только в полной записи: https://сайт/путь*");
            }
            return Ok(Self {
                http: true,
                https: true,
                host: Host::Domain(
                    clean_host(host).ok_or_else(|| format!("шаблон «{s}»: неверный сайт"))?,
                ),
                path: "/*".into(),
            });
        };
        let (http, https) = match scheme {
            "*" => (true, true),
            "http" => (true, false),
            "https" => (false, true),
            _ => return bad("схема только http, https или *"),
        };
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/*"),
        };
        // Порт не сравниваем: `localhost:8080` — тот же `localhost`.
        let host = match host.rsplit_once(':') {
            Some((h, port)) if !h.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
            _ => host,
        };
        let host = if host == "*" {
            Host::Any
        } else if let Some(domain) = host.strip_prefix("*.") {
            Host::Domain(clean_host(domain).ok_or_else(|| format!("шаблон «{s}»: неверный хост"))?)
        } else {
            Host::Exact(clean_host(host).ok_or_else(|| format!("шаблон «{s}»: неверный хост"))?)
        };
        Ok(Self {
            http,
            https,
            host,
            path: path.to_owned(),
        })
    }

    fn any() -> Self {
        Self {
            http: true,
            https: true,
            host: Host::Any,
            path: "/*".into(),
        }
    }

    /// Подходит ли адрес страницы.
    pub fn matches(&self, url: &str) -> bool {
        let Ok(url) = Url::parse(url) else {
            return false;
        };
        let scheme_ok = match url.scheme() {
            "http" => self.http,
            "https" => self.https,
            _ => false,
        };
        if !scheme_ok {
            return false;
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let host_ok = match &self.host {
            Host::Any => true,
            Host::Exact(h) => host == *h,
            Host::Domain(d) => {
                host == *d
                    || host
                        .strip_suffix(d.as_str())
                        .is_some_and(|p| p.ends_with('.'))
            }
        };
        if !host_ok {
            return false;
        }
        let mut path = url.path().to_owned();
        if let Some(q) = url.query() {
            path.push('?');
            path.push_str(q);
        }
        glob(&self.path, &path)
    }

    /// То же правило для скрипта в странице.
    pub fn js(&self) -> JsPattern {
        let (host, sub) = match &self.host {
            Host::Any => (String::new(), false),
            Host::Exact(h) => (h.clone(), false),
            Host::Domain(d) => (d.clone(), true),
        };
        JsPattern {
            s: u8::from(self.http) | (u8::from(self.https) << 1),
            h: host,
            d: sub,
            p: glob_regex(&self.path),
        }
    }
}

/// Шаблон для `bootstrap.js`: `s` — схемы (1 — http, 2 — https), `h` — хост
/// (пусто — любой), `d` — с поддоменами, `p` — регулярное выражение пути.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct JsPattern {
    s: u8,
    h: String,
    d: bool,
    p: String,
}

/// Хост в нижнем регистре, без точки в конце. Только то, что бывает в адресе:
/// буквы, цифры, `-`, `_`, `.`; кириллический домен — в punycode, как его видит браузер.
fn clean_host(raw: &str) -> Option<String> {
    let host = raw.trim_end_matches('.');
    if host.is_empty() || host.len() > 253 || host.contains('*') {
        return None;
    }
    // `Url` переводит кириллицу в punycode и проверяет, что это вообще хост.
    let url = Url::parse(&format!("http://{host}/")).ok()?;
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'[' | b']' | b':'))
        .then_some(host)
}

/// `*` — любые знаки (и ни одного), остальное — буквально.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0, 0);
    // Где была последняя звёздочка и с какого места текста она «съедает».
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&b| b == b'*')
}

/// Тот же `glob` регулярным выражением JavaScript.
fn glob_regex(pattern: &str) -> String {
    let mut out = String::from("^");
    for c in pattern.chars() {
        match c {
            '*' => out.push_str("[\\s\\S]*"),
            '\\' | '^' | '$' | '.' | '|' | '?' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '/' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(pattern: &str, url: &str) -> bool {
        Pattern::parse(pattern)
            .map(|p| p.matches(url))
            .unwrap_or(false)
    }

    #[test]
    fn all_urls() {
        for p in ["<all_urls>", "*"] {
            assert!(hit(p, "https://example.com/"));
            assert!(hit(p, "http://localhost:8080/a?b"));
            assert!(!hit(p, "axiom://settings"));
            assert!(!hit(p, "file:///C:/x.html"));
            assert!(!hit(p, "data:text/html,hi"));
            assert!(!hit(p, "view-source:https://example.com/"));
        }
    }

    #[test]
    fn short_site() {
        for p in ["youtube.com", "*.youtube.com", "YouTube.com."] {
            assert!(hit(p, "https://youtube.com/"), "{p}");
            assert!(hit(p, "https://www.youtube.com/watch?v=1"), "{p}");
            assert!(hit(p, "http://m.youtube.com/"), "{p}");
            assert!(!hit(p, "https://notyoutube.com/"), "{p}");
            assert!(!hit(p, "https://youtube.com.evil.example/"), "{p}");
        }
        assert!(Pattern::parse("youtube.com/watch").is_err());
    }

    #[test]
    fn full_form() {
        assert!(hit(
            "https://*.example.com/watch*",
            "https://www.example.com/watch?v=1"
        ));
        assert!(hit(
            "https://*.example.com/watch*",
            "https://example.com/watch"
        ));
        assert!(!hit(
            "https://*.example.com/watch*",
            "http://example.com/watch"
        ));
        assert!(!hit(
            "https://*.example.com/watch*",
            "https://example.com/feed"
        ));
        assert!(hit("*://example.com/*", "http://example.com/"));
        assert!(!hit("*://example.com/*", "https://sub.example.com/"));
        assert!(hit("https://example.com", "https://example.com/any/path?q"));
        assert!(hit("http://localhost:3000/*", "http://localhost:5173/x"));
        assert!(hit("https://*/*.pdf", "https://a.b/c/d.pdf"));
        assert!(!hit("https://*/*.pdf", "https://a.b/c/d.pdf?x"));
        assert!(hit("https://*/*.pdf*", "https://a.b/c/d.pdf?x"));
        // Кириллический домен — как его видит браузер, в punycode.
        assert!(hit(
            "*://*.пример.рф/*",
            "https://www.xn--e1afmkfd.xn--p1ai/"
        ));
    }

    #[test]
    fn rejects_garbage() {
        for p in [
            "",
            "ftp://x/*",
            "https://exa mple.com/",
            "https://*x.com/",
            "https:///",
            "javascript://x/",
        ] {
            assert!(Pattern::parse(p).is_err(), "{p}");
        }
        assert!(Pattern::parse(&"a".repeat(600)).is_err());
    }

    #[test]
    fn glob_rules() {
        assert!(glob("/*", "/"));
        assert!(glob("/a*b*c", "/aXXbYYc"));
        assert!(glob("/a*b*c", "/abc"));
        assert!(!glob("/a*b*c", "/acb"));
        assert!(glob("/**", "/x"));
        assert!(!glob("/a", "/ab"));
        assert!(glob("*", ""));
    }

    #[test]
    fn regex_escapes() {
        assert_eq!(glob_regex("/a.b?c*"), "^\\/a\\.b\\?c[\\s\\S]*$");
        let js = Pattern::parse("*.example.com").map(|p| p.js()).ok();
        assert_eq!(
            js,
            Some(JsPattern {
                s: 3,
                h: "example.com".into(),
                d: true,
                p: "^\\/[\\s\\S]*$".into()
            })
        );
    }
}
