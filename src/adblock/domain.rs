//! Домен сайта (eTLD+1) для движка фильтров: по нему решается, свой запрос или
//! чужой (`$third-party`), и к какому сайту относятся правила вида `site.ru##…`.
//!
//! Полный список публичных суффиксов (крейт psl) прибавил бы к Axiom 0,8 МБ.
//! Вместо него — правило, которое верно почти везде: домен — последние две части
//! имени, а в зонах второго уровня (`co.uk`, `com.ru`, `msk.ru`, `github.io`…) —
//! три. Ошибка здесь стоит немногого: сайт в редкой зоне примет соседа по зоне
//! за «своего», и правило `$third-party` между ними не сработает.

use adblock::url_parser::ResolvesDomain;

/// Вторые уровни, которые в двухбуквенных национальных зонах почти всегда
/// публичные: `bbc.co.uk`, `site.com.ua`, `gov.kz`.
const SECOND_LEVEL: &[&str] = &[
    "ac", "biz", "co", "com", "edu", "go", "gob", "gov", "gv", "info", "int", "ltd", "mil", "ne",
    "net", "nic", "nom", "or", "org", "plc", "sch",
];

/// Прочие зоны, где сайты — на третьем уровне: города и области России и
/// Украины, бесплатные хостинги и облака.
const ZONES: &[&str] = &[
    "msk.ru",
    "spb.ru",
    "nov.ru",
    "pp.ru",
    "narod.ru",
    "ucoz.ru",
    "kiev.ua",
    "kharkov.ua",
    "odessa.ua",
    "dp.ua",
    "lviv.ua",
    "in.ua",
    "github.io",
    "gitlab.io",
    "blogspot.com",
    "appspot.com",
    "herokuapp.com",
    "netlify.app",
    "vercel.app",
    "pages.dev",
    "workers.dev",
    "web.app",
    "firebaseapp.com",
    "azurewebsites.net",
    "cloudfront.net",
    "wordpress.com",
    "tumblr.com",
    "livejournal.com",
];

pub struct Resolver;

impl ResolvesDomain for Resolver {
    fn get_host_domain(&self, host: &str) -> (usize, usize) {
        let (start, end) = domain_range(host);
        (start, end)
    }

    fn is_public_suffix(&self, domain: &str) -> bool {
        !domain.contains('.') || suffix_labels(domain) >= labels(domain)
    }
}

/// Движку нужен разбор доменов раньше первого правила: без него он падает.
pub fn install() {
    // Уже поставлен (второй вызов, тесты) — тот же самый.
    let _ = adblock::url_parser::set_domain_resolver(Box::new(Resolver));
}

fn labels(host: &str) -> usize {
    host.split('.').count()
}

/// Сколько последних частей имени — публичная зона: 1 (`ru`) или 2 (`co.uk`).
fn suffix_labels(host: &str) -> usize {
    let mut parts = host.rsplit('.');
    let (Some(tld), Some(second)) = (parts.next(), parts.next()) else {
        return 1;
    };
    let last_two = &host[host.len() - tld.len() - second.len() - 1..];
    if (tld.len() == 2 && SECOND_LEVEL.contains(&second)) || ZONES.contains(&last_two) {
        2
    } else {
        1
    }
}

/// Где в имени хоста его домен (начало и конец). IP-адрес — целиком.
fn domain_range(host: &str) -> (usize, usize) {
    let host_end = host.trim_end_matches('.').len();
    let host = &host[..host_end];
    if host.is_empty() {
        return (0, 0);
    }
    // IPv6 (с двоеточиями) и IPv4 (только цифры и точки) — без доменов.
    if host.contains(':') || host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return (0, host_end);
    }
    let keep = suffix_labels(host) + 1;
    match host.rmatch_indices('.').nth(keep - 1) {
        Some((dot, _)) => (dot + 1, host_end),
        None => (0, host_end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(host: &str) -> &str {
        let (start, end) = domain_range(host);
        &host[start..end]
    }

    #[test]
    fn registrable_domains() {
        for (host, want) in [
            ("example.com", "example.com"),
            ("www.example.com", "example.com"),
            ("api.m.example.com", "example.com"),
            ("lenta.ru", "lenta.ru"),
            ("an.yandex.ru", "yandex.ru"),
            ("news.bbc.co.uk", "bbc.co.uk"),
            ("bbc.co.uk", "bbc.co.uk"),
            ("shop.site.com.ua", "site.com.ua"),
            ("a.b.msk.ru", "b.msk.ru"),
            ("user.github.io", "user.github.io"),
            ("x.user.github.io", "user.github.io"),
            ("пример.рф", "пример.рф"),
            ("www.пример.рф", "пример.рф"),
            ("localhost", "localhost"),
            ("co.uk", "co.uk"),
            ("example.com.", "example.com"),
            ("192.168.0.1", "192.168.0.1"),
            ("::1", "::1"),
            ("", ""),
        ] {
            assert_eq!(domain(host), want, "{host}");
        }
    }

    #[test]
    fn public_suffixes() {
        let r = Resolver;
        assert!(r.is_public_suffix("com"));
        assert!(r.is_public_suffix("co.uk"));
        assert!(r.is_public_suffix("github.io"));
        assert!(!r.is_public_suffix("example.com"));
        assert!(!r.is_public_suffix("bbc.co.uk"));
    }
}
