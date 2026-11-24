//! Разбор ввода адресной строки: адрес или поиск в выбранной поисковой системе.

use std::net::Ipv4Addr;

use serde::{Deserialize, Deserializer, Serialize};
use url::{Url, form_urlencoded};

use crate::ipc::MAX_INPUT;

/// Внутренний адрес новой вкладки. Такая вкладка не создаёт webview вообще.
pub const NEWTAB: &str = "axiom://newtab";
/// Самый длинный свой адрес поиска.
pub const MAX_TEMPLATE: usize = 1024;
const AI_BASE: &str = "https://www.google.com/search";
/// Стартовая страница AI Mode без запроса.
pub const AI_HOME: &str = "https://www.google.com/search?udm=50";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Search(String),
}

impl Target {
    pub fn into_url(self, search: Search) -> String {
        match self {
            Target::Url(u) => u,
            Target::Search(q) => search_url(&q, search),
        }
    }
}

/// Поисковая система адресной строки. В настройках хранится строкой:
/// duckduckgo, google, yandex, bing, brave, startpage, ecosia или custom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[default]
    DuckDuckGo,
    Google,
    Yandex,
    Bing,
    Brave,
    Startpage,
    Ecosia,
    /// Свой адрес с `%s` на месте запроса.
    Custom,
}

impl Engine {
    const BUILT_IN: [Engine; 7] = [
        Engine::DuckDuckGo,
        Engine::Google,
        Engine::Yandex,
        Engine::Bing,
        Engine::Brave,
        Engine::Startpage,
        Engine::Ecosia,
    ];

    /// Адрес страницы результатов без запроса.
    fn base(self) -> &'static str {
        match self {
            Engine::DuckDuckGo | Engine::Custom => "https://duckduckgo.com/",
            Engine::Google => "https://www.google.com/search",
            Engine::Yandex => "https://yandex.ru/search/",
            Engine::Bing => "https://www.bing.com/search",
            Engine::Brave => "https://search.brave.com/search",
            Engine::Startpage => "https://www.startpage.com/sp/search",
            Engine::Ecosia => "https://www.ecosia.org/search",
        }
    }

    /// В каком параметре адреса лежит запрос.
    fn param(self) -> &'static str {
        match self {
            Engine::Yandex => "text",
            Engine::Startpage => "query",
            _ => "q",
        }
    }

    /// Страница результатов этой системы: домен и путь.
    fn owns(self, host: &str, path: &str) -> bool {
        let host = host.strip_prefix("www.").unwrap_or(host);
        let search = path.trim_end_matches('/') == "/search";
        match self {
            Engine::DuckDuckGo => domain(host, "duckduckgo.com"),
            Engine::Google => brand(host, "google") && search,
            Engine::Yandex => (brand(host, "yandex") || host == "ya.ru") && search,
            Engine::Bing => domain(host, "bing.com") && search,
            Engine::Brave => host == "search.brave.com" && search,
            Engine::Startpage => {
                domain(host, "startpage.com")
                    && (search || matches!(path, "/sp/search" | "/do/search" | "/do/dsearch"))
            }
            Engine::Ecosia => domain(host, "ecosia.org") && search,
            Engine::Custom => false,
        }
    }
}

/// Неизвестная система (или мусор в файле настроек) — DuckDuckGo,
/// а не отказ читать все настройки целиком.
pub fn lenient_engine<'de, D: Deserializer<'de>>(d: D) -> Result<Engine, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(Engine::deserialize(v).unwrap_or_default())
}

/// Куда уходит поиск: система из настроек и свой адрес для `Engine::Custom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Search<'a> {
    pub engine: Engine,
    pub custom: &'a str,
}

impl Default for Search<'_> {
    fn default() -> Self {
        Self {
            engine: Engine::DuckDuckGo,
            custom: "",
        }
    }
}

/// Свой адрес поиска: http(s), с `%s` на месте запроса, и запрос не может
/// поменять сайт (`%s` только в пути или параметрах).
pub fn valid_template(template: &str) -> bool {
    if template.len() > MAX_TEMPLATE || !template.contains("%s") {
        return false;
    }
    let host = |fill: &str| {
        Url::parse(&template.replace("%s", fill))
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .and_then(|u| u.host_str().map(str::to_string))
    };
    host("a").is_some_and(|a| host("b").is_some_and(|b| a == b))
}

fn domain(host: &str, name: &str) -> bool {
    host == name
        || host
            .strip_suffix(name)
            .is_some_and(|sub| sub.ends_with('.'))
}

/// `google.com`, `google.ru`, `google.co.uk`: имя и короткая доменная зона.
fn brand(host: &str, name: &str) -> bool {
    host.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|zone| {
            !zone.is_empty()
                && zone.len() <= 6
                && zone.split('.').count() <= 2
                && zone
                    .split('.')
                    .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_lowercase()))
        })
}

fn clean(q: &str) -> Option<String> {
    let q = q.trim();
    (!q.is_empty()).then(|| q.to_string())
}

/// Запрос со страницы результатов любой поддерживаемой системы или своего адреса
/// (`custom` — шаблон с `%s`, пустой — без него). AI Mode Google поиском не считается.
pub fn search_query(url: &str, custom: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?;
    if let Some(engine) = Engine::BUILT_IN
        .into_iter()
        .find(|e| e.owns(host, u.path()))
    {
        if engine == Engine::Google && u.query_pairs().any(|(k, v)| k == "udm" && v == "50") {
            return None;
        }
        // Старые ссылки Startpage передают запрос в `q`.
        let keys: &[&str] = match engine {
            Engine::Startpage => &["query", "q"],
            _ => &[engine.param()],
        };
        return keys
            .iter()
            .find_map(|key| u.query_pairs().find(|(k, _)| k == key))
            .and_then(|(_, v)| clean(&v));
    }
    custom_query(&u, custom)
}

/// Метка на месте `%s`, чтобы найти запрос в адресе по шаблону.
const MARK: &str = "axiomquerymark";

fn custom_query(u: &Url, template: &str) -> Option<String> {
    if !valid_template(template) {
        return None;
    }
    let t = Url::parse(&template.replace("%s", MARK)).ok()?;
    if t.host_str() != u.host_str() {
        return None;
    }
    let pair = t
        .query_pairs()
        .find(|(_, v)| v.contains(MARK))
        .map(|(k, v)| (k.into_owned(), v.into_owned()));
    if let Some((key, value)) = pair {
        let (pre, post) = value.split_once(MARK)?;
        if t.path() != u.path() {
            return None;
        }
        let (_, got) = u.query_pairs().find(|(k, _)| *k == key)?;
        return clean(got.strip_prefix(pre)?.strip_suffix(post)?);
    }
    let (pre, post) = t.path().split_once(MARK)?;
    let middle = u.path().strip_prefix(pre)?.strip_suffix(post)?;
    // В пути «+» — это плюс, а не пробел: декодируем только %XX.
    let decoded = form_urlencoded::parse(format!("q={}", middle.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())?;
    clean(&decoded)
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

pub fn search_url(query: &str, search: Search) -> String {
    if search.engine == Engine::Custom && valid_template(search.custom) {
        // %20 вместо «+»: в пути адреса плюс остался бы плюсом.
        let q = form_urlencoded::byte_serialize(query.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        if let Ok(u) = Url::parse(&search.custom.replace("%s", &q)) {
            return u.into();
        }
    }
    let engine = match search.engine {
        Engine::Custom => Engine::DuckDuckGo,
        e => e,
    };
    Url::parse_with_params(engine.base(), &[(engine.param(), query)])
        .map(String::from)
        .unwrap_or_else(|_| engine.base().to_string())
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

    fn engine(engine: Engine) -> Search<'static> {
        Search { engine, custom: "" }
    }

    #[test]
    fn builds_search_urls() {
        assert_eq!(
            search_url("как дела?", Search::default()),
            "https://duckduckgo.com/?q=%D0%BA%D0%B0%D0%BA+%D0%B4%D0%B5%D0%BB%D0%B0%3F"
        );
        assert!(ai_url("rust").starts_with("https://www.google.com/search?udm=50&q=rust"));
        let cases = [
            (Engine::Google, "https://www.google.com/search?q=rust+wry"),
            (Engine::Yandex, "https://yandex.ru/search/?text=rust+wry"),
            (Engine::Bing, "https://www.bing.com/search?q=rust+wry"),
            (Engine::Brave, "https://search.brave.com/search?q=rust+wry"),
            (
                Engine::Startpage,
                "https://www.startpage.com/sp/search?query=rust+wry",
            ),
            (Engine::Ecosia, "https://www.ecosia.org/search?q=rust+wry"),
        ];
        for (e, url) in cases {
            assert_eq!(search_url("rust wry", engine(e)), url, "{e:?}");
            assert_eq!(Target::Search("rust wry".into()).into_url(engine(e)), url);
        }
        // Адреса поиском не становятся, какая бы система ни была выбрана.
        assert_eq!(
            resolve("example.com").map(|t| t.into_url(engine(Engine::Yandex))),
            Some("https://example.com/".to_string())
        );
    }

    #[test]
    fn custom_template() {
        let path = Search {
            engine: Engine::Custom,
            custom: "https://search.example.org/find/%s?lang=ru",
        };
        assert_eq!(
            search_url("a b+c/д", path),
            "https://search.example.org/find/a%20b%2Bc%2F%D0%B4?lang=ru"
        );
        let query = Search {
            engine: Engine::Custom,
            custom: "http://localhost:8080/?s=%s&ia=web",
        };
        assert_eq!(
            search_url("rust & wry", query),
            "http://localhost:8080/?s=rust%20%26%20wry&ia=web"
        );
        // Без годного шаблона — DuckDuckGo.
        let broken = Search {
            engine: Engine::Custom,
            custom: "javascript:alert(%s)",
        };
        assert_eq!(search_url("x", broken), "https://duckduckgo.com/?q=x");
    }

    #[test]
    fn checks_templates() {
        for ok in [
            "https://example.com/?q=%s",
            "http://192.168.1.2:8888/search?q=%s&format=html",
            "https://example.com/search/%s",
        ] {
            assert!(valid_template(ok), "{ok}");
        }
        for bad in [
            "",
            "https://example.com/?q=",
            "example.com/?q=%s",
            "ftp://example.com/%s",
            "javascript:alert(%s)",
            "file:///C:/%s",
            "https://%s.example.com/",
            "https://%s/",
            "https://exa mple.com/?q=%s",
        ] {
            assert!(!valid_template(bad), "{bad}");
        }
        assert!(!valid_template(&format!(
            "https://example.com/?q=%s&x={}",
            "a".repeat(MAX_TEMPLATE)
        )));
    }

    #[test]
    fn engines_round_trip() {
        // Что адресная строка отправила, то двойной поиск и узнает.
        for e in Engine::BUILT_IN {
            let url = search_url("как дела + rust", engine(e));
            assert_eq!(
                search_query(&url, "").as_deref(),
                Some("как дела + rust"),
                "{url}"
            );
        }
        for custom in [
            "https://search.example.org/find/%s?lang=ru",
            "https://example.com/s?lang=ru&query=%s",
            "https://example.com/s?query=site%3Aexample.com+%s",
        ] {
            let url = search_url(
                "как дела + rust",
                Search {
                    engine: Engine::Custom,
                    custom,
                },
            );
            assert_eq!(
                search_query(&url, custom).as_deref(),
                Some("как дела + rust"),
                "{url}"
            );
            // Без шаблона чужой сайт поиском не считается.
            assert_eq!(search_query(&url, ""), None);
        }
    }

    #[test]
    fn finds_queries() {
        let q = |url: &str| search_query(url, "");
        let some = |s: &str| Some(s.to_string());
        assert_eq!(q("https://duckduckgo.com/?q=rust&ia=web"), some("rust"));
        assert_eq!(q("https://html.duckduckgo.com/html/?q=rust"), some("rust"));
        assert_eq!(q("https://www.google.com/search?q=wry&hl=ru"), some("wry"));
        assert_eq!(q("https://www.google.co.uk/search?q=tea"), some("tea"));
        assert_eq!(q("https://google.ru/search?q=%D0%B4%D0%B0"), some("да"));
        assert_eq!(
            q("https://yandex.ru/search/?text=%D0%BF%D0%BE%D0%B3%D0%BE%D0%B4%D0%B0&lr=213"),
            some("погода")
        );
        assert_eq!(q("https://ya.ru/search/?text=rust"), some("rust"));
        assert_eq!(q("https://yandex.com.tr/search/?text=rust"), some("rust"));
        assert_eq!(
            q("https://www.bing.com/search?q=rust+lang&form=QBLH"),
            some("rust lang")
        );
        assert_eq!(
            q("https://search.brave.com/search?q=rust&source=web"),
            some("rust")
        );
        assert_eq!(
            q("https://www.startpage.com/sp/search?query=rust"),
            some("rust")
        );
        assert_eq!(
            q("https://www.startpage.com/do/search?q=rust"),
            some("rust")
        );
        assert_eq!(
            q("https://www.ecosia.org/search?method=index&q=rust"),
            some("rust")
        );

        // AI Mode, пустые запросы, не страницы результатов и похожие домены — мимо.
        assert_eq!(q("https://www.google.com/search?udm=50&q=rust"), None);
        assert_eq!(q("https://duckduckgo.com/?q=++"), None);
        assert_eq!(q("https://duckduckgo.com/"), None);
        assert_eq!(q("https://www.google.com/maps?q=rust"), None);
        assert_eq!(q("https://yandex.ru/maps/?text=rust"), None);
        assert_eq!(q("https://notgoogle.com/search?q=rust"), None);
        assert_eq!(q("https://google.evil.example/search?q=rust"), None);
        assert_eq!(q("https://bing.com.evil.example/search?q=rust"), None);
        assert_eq!(q("https://example.com/search?q=rust"), None);
        assert_eq!(q("not a url"), None);
        assert_eq!(
            search_query(
                "https://example.com/other?query=rust",
                "https://example.com/s?query=%s"
            ),
            None
        );
    }

    #[test]
    fn reads_engine_leniently() {
        #[derive(Deserialize)]
        struct S {
            #[serde(deserialize_with = "lenient_engine")]
            e: Engine,
        }
        let e = |json: &str| serde_json::from_str::<S>(json).map(|s| s.e).ok();
        assert_eq!(e(r#"{"e":"yandex"}"#), Some(Engine::Yandex));
        assert_eq!(e(r#"{"e":"custom"}"#), Some(Engine::Custom));
        assert_eq!(e(r#"{"e":"altavista"}"#), Some(Engine::DuckDuckGo));
        assert_eq!(e(r#"{"e":42}"#), Some(Engine::DuckDuckGo));
    }

    #[test]
    fn navigable_schemes() {
        assert!(is_navigable("https://a.b"));
        assert!(!is_navigable("javascript:alert(1)"));
        assert!(!is_navigable("ms-settings:"));
    }
}
