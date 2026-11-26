//! Усыпление вкладок: что спрашиваем у страницы перед выгрузкой
//! и какие фоновые вкладки лишние.
//!
//! Вкладку с недописанным текстом не усыпляем, как и Chrome при выгрузке вкладок:
//! иначе после пробуждения пропадёт начатый комментарий или письмо.
//!
//! Вкладку, которую человек попросил не усыплять, не усыпляют ни таймер, ни нехватка
//! памяти, ни лимит живых вкладок. Место в лимите она всё равно занимает. Так же
//! живут вкладки сайтов из списка «не усыплять» (настройки и меню вкладки):
//! `youtube.com` в нём — это и `www.youtube.com`, и `m.youtube.com`.
//!
//! Живые фоновые вкладки не замораживаем (`TrySuspend`) и не переводим в экономный
//! режим памяти: WebView2 тогда выталкивает почти всю память страницы в сжатие и
//! подкачку, и при возврате на вкладку она подолгу дёргается, пока память
//! подгружается обратно. Спрятанную страницу Chromium и так притормаживает сам.

use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer};
use url::{Host, Url};

use super::TabId;

/// Сколько сайтов можно держать в списке «не усыплять».
pub const MAX_SITES: usize = 200;

/// Когда системе не хватает памяти, фоновые вкладки засыпают уже через минуту.
pub const LOW_MEMORY_IDLE: Duration = Duration::from_secs(60);

/// Скрипт в каждой странице вкладки: запоминает поле, в которое человек печатал.
/// Флаг выставляют только настоящие нажатия (`isTrusted`), не скрипты сайта.
pub const DRAFT_TRACKER: &str = r"(() => {
  let field = null;
  const skip = /^(checkbox|radio|range|color|file|submit|button|reset|image|hidden|select-one|select-multiple)$/;
  addEventListener('input', (e) => {
    const t = e.composedPath()[0];
    if (e.isTrusted && t && !skip.test(t.type || '')) field = t;
  }, true);
  Object.defineProperty(window, '__axiomDraft', {
    value: () => {
      const f = field;
      if (!f || !f.isConnected) return false;
      const text = typeof f.value === 'string' ? f.value : f.textContent;
      return (text || '').trim() !== '';
    },
  });
})();";

/// Вопрос странице перед усыплением: место прокрутки и остался ли недописанный текст.
pub const PROBE: &str = "(() => ({ y: window.scrollY, \
    keep: typeof window.__axiomDraft === 'function' && window.__axiomDraft() }))()";

/// Ответ страницы на [`PROBE`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct PageState {
    pub y: f64,
    /// В поле остался текст, который человек начал печатать.
    pub keep: bool,
}

impl PageState {
    /// Ответ скрипта приходит в JSON. Невнятный ответ — усыпляем как обычно.
    pub fn parse(raw: &str) -> Self {
        serde_json::from_str(raw.trim()).unwrap_or_default()
    }
}

/// Вкладки сверх лимита: оставляем `keep` недавно открытых, остальные — на усыпление.
pub fn excess(mut live: Vec<(Instant, TabId)>, keep: usize) -> Vec<TabId> {
    live.sort_by_key(|&(seen, _)| std::cmp::Reverse(seen));
    live.into_iter().skip(keep).map(|(_, id)| id).collect()
}

/// Сколько из `candidates` усыпляемых вкладок остаётся живыми при лимите `limit`.
/// `live` — все живые фоновые вкладки: те, что усыплять нельзя, занимают место первыми.
pub fn room(limit: usize, live: usize, candidates: usize) -> usize {
    limit.saturating_sub(live.saturating_sub(candidates))
}

// ---------- сайты, которые не усыпляем ----------

/// Сайт для списка «не усыплять» из того, что вписали или открыто во вкладке:
/// `https://www.YouTube.com:443/watch?v=1` → `youtube.com`. Без схемы — тоже адрес
/// (`m.youtube.com/feed`), `*.` в начале — то же, что без него. Не сайт (пусто,
/// пробелы, не http(s), одно слово, кроме `localhost`) — `None`.
/// Как `sleepSite` в ui/settings.js.
pub fn site(input: &str) -> Option<String> {
    let s = input.trim().to_lowercase();
    if s.is_empty() || s.len() > 2048 || s.chars().any(char::is_whitespace) {
        return None;
    }
    let s = s.trim_start_matches("*.").trim_start_matches('.');
    let scheme = s.split_once("://").is_some_and(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_lowercase())
            && scheme
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+-.".contains(&b))
    });
    let url = if scheme {
        Url::parse(s)
    } else {
        Url::parse(&format!("https://{s}"))
    }
    .ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    match url.host()? {
        Host::Domain(d) => {
            let d = d.strip_suffix('.').unwrap_or(d);
            if !valid_domain(d) {
                return None;
            }
            // `www.com` — сайт, а не «все сайты в .com».
            let d = d
                .strip_prefix("www.")
                .filter(|r| r.contains('.'))
                .unwrap_or(d);
            Some(d.to_string())
        }
        Host::Ipv4(_) | Host::Ipv6(_) => url.host_str().map(str::to_string),
    }
}

/// Домен из букв, цифр, `-` и `_`, с точкой (или `localhost`). Русские домены
/// парсер адреса уже перевёл в `xn--…`.
fn valid_domain(d: &str) -> bool {
    let label = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    };
    d.len() <= 253 && (d.contains('.') || d == "localhost") && d.split('.').all(label)
}

/// Хост страницы вкладки для сравнения со списком; не http(s) — `None`.
fn host(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    let host = host.strip_suffix('.').unwrap_or(host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// `site` из списка покрывает `host`: сам сайт или его поддомен.
fn covers(site: &str, host: &str) -> bool {
    host.strip_suffix(site)
        .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
}

/// Вкладку с адресом `url` не усыпляют: её сайт в списке.
pub fn site_kept(sites: &[String], url: &str) -> bool {
    !sites.is_empty() && host(url).is_some_and(|h| sites.iter().any(|s| covers(s, &h)))
}

/// Список из настроек или от интерфейса: каждый сайт приведён к виду [`site`],
/// мусор выброшен, повторы убраны, не длиннее [`MAX_SITES`]. Порядок — как был.
pub fn clean_sites(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in list.iter().filter_map(|s| site(s)) {
        if out.len() == MAX_SITES {
            break;
        }
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Список сайтов в файле настроек: не массив — пустой, не строки внутри пропускаются,
/// а не отказ читать все настройки целиком.
pub fn lenient_sites<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// Что сделал пункт меню «Не усыплять на этом сайте».
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteToggle {
    /// Сайт вкладки добавлен в список.
    Added(String),
    /// Убраны все записи, которые покрывали вкладку (сайт и его родители).
    Removed(Vec<String>),
    /// Список полон.
    Full,
}

/// Сайт вкладки с адресом `url` — в список или из него. `None` — у вкладки нет сайта.
pub fn toggle_site(sites: &mut Vec<String>, url: &str) -> Option<SiteToggle> {
    let host = host(url)?;
    let removed: Vec<String> = sites.iter().filter(|s| covers(s, &host)).cloned().collect();
    if !removed.is_empty() {
        sites.retain(|s| !covers(s, &host));
        return Some(SiteToggle::Removed(removed));
    }
    let site = site(&host)?;
    if sites.len() >= MAX_SITES {
        return Some(SiteToggle::Full);
    }
    sites.push(site.clone());
    Some(SiteToggle::Added(site))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn excess_drops_the_oldest() {
        let base = Instant::now();
        let at = |s| base + Duration::from_secs(s);
        let live = vec![(at(5), 1), (at(1), 2), (at(9), 3), (at(3), 4)];
        // Свежее всех 3 и 1 — остаются; 4 и 2 — на усыпление, давние в конце.
        assert_eq!(excess(live.clone(), 2), vec![4, 2]);
        assert_eq!(excess(live.clone(), 4), Vec::<TabId>::new());
        assert_eq!(excess(live, 0).len(), 4);
    }

    #[test]
    fn room_counts_tabs_that_cannot_sleep() {
        // Все живые можно усыпить: в лимите остаётся сколько разрешено.
        assert_eq!(room(3, 5, 5), 3);
        // Две живые усыплять нельзя: они занимают два места из трёх.
        assert_eq!(room(3, 5, 3), 1);
        // Неусыпляемых больше лимита: живыми не остаётся ни одна из остальных.
        assert_eq!(room(3, 5, 1), 0);
        assert_eq!(room(3, 0, 0), 3);
    }

    /// Фоновые живые вкладки 1..=n: первая открыта давнее всех.
    fn live_tabs(n: u64) -> crate::tabs::TabStore {
        let base = Instant::now();
        let mut s = crate::tabs::TabStore::default();
        for i in 0..n {
            let id = s.add(format!("https://t{i}.example/"), String::new(), false, None);
            if let Some(tab) = s.get_mut(id) {
                tab.last_active = base + Duration::from_secs(i);
            }
        }
        s
    }

    /// Кого усыпит лимит живых вкладок, как `App::limit_live_tabs`.
    fn over_limit(s: &crate::tabs::TabStore, limit: usize) -> Vec<TabId> {
        over_limit_with(s, limit, &[])
    }

    fn over_limit_with(s: &crate::tabs::TabStore, limit: usize, sites: &[String]) -> Vec<TabId> {
        let candidates: Vec<_> = s
            .iter()
            .filter(|t| t.may_sleep(sites))
            .map(|t| (t.last_active, t.id))
            .collect();
        let keep = room(limit, s.len(), candidates.len());
        excess(candidates, keep)
    }

    #[test]
    fn no_sleep_tab_is_never_chosen() {
        let mut s = live_tabs(4);
        assert_eq!(over_limit(&s, 3), vec![1]);
        // Самую давнюю вкладку попросили не усыплять.
        if let Some(tab) = s.get_mut(1) {
            tab.no_sleep = true;
        }
        assert!(s.get(1).is_some_and(|t| !t.may_sleep(&[])));
        // Она живая и занимает место в лимите: вместо неё засыпает следующая по давности.
        assert_eq!(over_limit(&s, 3), vec![2]);
        assert_eq!(over_limit(&s, 1), vec![4, 3, 2]);
        assert_eq!(over_limit(&s, 4), Vec::<TabId>::new());
        // Не усыплять попросили все: никто не засыпает, даже сверх лимита.
        for tab in s.iter_mut() {
            tab.no_sleep = true;
        }
        assert_eq!(over_limit(&s, 1), Vec::<TabId>::new());
    }

    #[test]
    fn may_sleep_respects_every_flag() {
        let flags: [fn(&mut crate::tabs::Tab); 3] = [
            |t| t.keep_alive = true,
            |t| t.sleep_requested = Some(Instant::now()),
            |t| t.no_sleep = true,
        ];
        assert!(live_tabs(1).iter().all(|t| t.may_sleep(&[])));
        for set in flags {
            let mut s = live_tabs(1);
            s.iter_mut().for_each(set);
            assert!(s.iter().all(|t| !t.may_sleep(&[])));
        }
        // Сайт вкладки в списке «не усыплять» — как её собственный флаг.
        let s = live_tabs(1);
        assert!(s.iter().all(|t| !t.may_sleep(&sites(&["t0.example"]))));
        assert!(
            s.iter()
                .all(|t| t.kept_awake(&sites(&["t0.example"])) && !t.no_sleep)
        );
        assert!(s.iter().all(|t| t.may_sleep(&sites(&["t1.example"]))));
    }

    #[test]
    fn site_tabs_are_never_chosen() {
        let s = live_tabs(4);
        // Сайт самой давней вкладки в списке: вместо неё засыпает следующая по давности.
        let list = sites(&["t0.example"]);
        assert_eq!(over_limit_with(&s, 3, &list), vec![2]);
        assert_eq!(over_limit_with(&s, 1, &list), vec![4, 3, 2]);
    }

    fn sites(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn site_normalises_input() {
        let ok = [
            ("youtube.com", "youtube.com"),
            ("  YouTube.COM  ", "youtube.com"),
            ("https://www.youtube.com/watch?v=1#t=3", "youtube.com"),
            ("http://m.youtube.com:8080/feed", "m.youtube.com"),
            ("www.youtube.com/", "youtube.com"),
            ("WWW.Example.org.", "example.org"),
            ("*.github.com", "github.com"),
            (".github.com", "github.com"),
            ("user@mail.example.com/inbox", "mail.example.com"),
            ("music.youtube.com", "music.youtube.com"),
            // `www.com` — это сайт, а не «все сайты в .com».
            ("www.com", "www.com"),
            ("localhost:3000", "localhost"),
            ("http://127.0.0.1:8080/", "127.0.0.1"),
            ("[::1]:8080", "[::1]"),
            ("пример.рф", "xn--e1afmkfd.xn--p1ai"),
            ("my_site.example", "my_site.example"),
            ("youtube.com/redirect?u=https://x.org", "youtube.com"),
        ];
        for (input, want) in ok {
            assert_eq!(site(input).as_deref(), Some(want), "{input}");
        }
        let bad = [
            "",
            "   ",
            "youtube",
            "com",
            "you tube.com",
            "ftp://example.com/",
            "javascript:alert(1)",
            "file:///C:/x.html",
            "axiom://settings",
            "about:blank",
            "a..b.com",
            "exa$mple.com",
            "example.com:port",
            "*.",
        ];
        for input in bad {
            assert_eq!(site(input), None, "{input}");
        }
        assert_eq!(site(&format!("{}.com", "a".repeat(64))), None);
        assert_eq!(site(&format!("{}.com", "a".repeat(2050))), None);
    }

    #[test]
    fn site_covers_subdomains() {
        let list = sites(&["youtube.com", "127.0.0.1"]);
        for url in [
            "https://youtube.com/",
            "https://www.youtube.com/watch?v=1",
            "https://m.youtube.com/",
            "http://music.youtube.com:8080/x",
            "https://YOUTUBE.com./",
            "http://127.0.0.1:3000/",
        ] {
            assert!(site_kept(&list, url), "{url}");
        }
        for url in [
            "https://notyoutube.com/",
            "https://youtube.com.evil.example/",
            "https://youtu.be/xyz",
            "http://127.0.0.10/",
            "axiom://newtab",
            "about:blank",
            "file:///C:/youtube.com",
            "",
        ] {
            assert!(!site_kept(&list, url), "{url}");
        }
        assert!(!site_kept(&[], "https://youtube.com/"));
    }

    #[test]
    fn clean_sites_dedupes_and_caps() {
        let list = sites(&[
            "https://www.youtube.com/",
            "junk",
            "youtube.com",
            "GitHub.com",
            "",
            "github.com/",
        ]);
        assert_eq!(clean_sites(&list), sites(&["youtube.com", "github.com"]));
        let many: Vec<String> = (0..MAX_SITES + 20)
            .map(|i| format!("s{i}.example"))
            .collect();
        let cleaned = clean_sites(&many);
        assert_eq!(cleaned.len(), MAX_SITES);
        assert_eq!(cleaned.first().map(String::as_str), Some("s0.example"));
    }

    #[test]
    fn lenient_sites_keeps_strings() -> serde_json::Result<()> {
        #[derive(Deserialize)]
        struct File {
            #[serde(default, deserialize_with = "lenient_sites")]
            sites: Vec<String>,
        }
        let read = |json: &str| serde_json::from_str::<File>(json).map(|f| f.sites);
        assert_eq!(
            read(r#"{"sites":["a.com",1,null,"b.com"]}"#)?,
            sites(&["a.com", "b.com"])
        );
        assert_eq!(read(r#"{"sites":"a.com"}"#)?, Vec::<String>::new());
        assert_eq!(read("{}")?, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn toggle_site_adds_and_removes() {
        let mut list = Vec::new();
        assert_eq!(
            toggle_site(&mut list, "https://www.youtube.com/watch?v=1"),
            Some(SiteToggle::Added("youtube.com".into()))
        );
        assert_eq!(list, sites(&["youtube.com"]));
        // С поддомена сайт убирается целиком: вкладка там снова засыпает.
        assert_eq!(
            toggle_site(&mut list, "https://m.youtube.com/"),
            Some(SiteToggle::Removed(sites(&["youtube.com"])))
        );
        assert!(list.is_empty());
        // Убираются все записи, что покрывали вкладку, остальные остаются.
        let mut list = sites(&["youtube.com", "music.youtube.com", "github.com"]);
        assert_eq!(
            toggle_site(&mut list, "https://music.youtube.com/"),
            Some(SiteToggle::Removed(sites(&[
                "youtube.com",
                "music.youtube.com"
            ])))
        );
        assert_eq!(list, sites(&["github.com"]));
        assert!(!site_kept(&list, "https://music.youtube.com/"));
        // У служебных страниц и адресов из одного слова сайта для списка нет.
        assert_eq!(toggle_site(&mut list, "axiom://newtab"), None);
        assert_eq!(toggle_site(&mut list, "http://intranet/"), None);
        // Список полон: новый сайт не добавляется, а убрать можно.
        let mut full: Vec<String> = (0..MAX_SITES).map(|i| format!("s{i}.example")).collect();
        assert_eq!(
            toggle_site(&mut full, "https://new.example/"),
            Some(SiteToggle::Full)
        );
        assert_eq!(full.len(), MAX_SITES);
        assert!(matches!(
            toggle_site(&mut full, "https://s5.example/"),
            Some(SiteToggle::Removed(_))
        ));
    }

    #[test]
    fn page_state_parsing() {
        assert_eq!(
            PageState::parse(r#"{"y":120.5,"keep":true}"#),
            PageState {
                y: 120.5,
                keep: true
            }
        );
        assert_eq!(PageState::parse(r#"{"y":0}"#), PageState::default());
        assert_eq!(PageState::parse("null"), PageState::default());
        assert_eq!(PageState::parse("oops"), PageState::default());
    }
}
