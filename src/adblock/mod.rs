//! Блокировка рекламы: списки фильтров EasyList и RuAdList (синтаксис Adblock Plus)
//! и движок Brave (крейт `adblock`).
//!
//! - Запросы страниц к рекламе не уходят в сеть: во вкладке и во всех её фреймах —
//!   скрипты, картинки, фреймы, запросы `fetch`/XHR, в том числе из воркеров сайта.
//!   Переход самой вкладки не запрещается: человек сам открыл этот адрес.
//! - Элементы рекламы на страницах скрыты (косметические правила `##`).
//!
//! Windows (WebView2, `windows.rs`): каждый запрос проверяет движок в
//! `WebResourceRequested` — микросекунды. Косметику страница (каждый её фрейм)
//! берёт у браузера сама, скриптом `cosmetic.js`: правила своего сайта — сразу
//! при создании документа, общие правила — только для классов и id, которые на ней
//! есть (так делает Brave: весь общий список в каждом фрейме — мегабайты памяти).
//! Linux (WebKitGTK, `linux.rs`): правила переводятся в формат блокировщиков Safari,
//! WebKit один раз компилирует их в профиль и сам применяет к запросам и элементам.
//!
//! Списки — встроенный набор и свежие копии, которые Axiom скачивает раз в 4 дня
//! (`lists.rs`; выключается в настройках, см. PRIVACY.md). Движок собирается в фоне:
//! запуск его не ждёт, а пока его нет, запросы идут как шли.
//!
//! «Не блокировать на этом сайте» — по сайту вкладки: на нём ничего не блокируется
//! и не скрывается, в том числе в его фреймах.

pub mod domain;
#[cfg(target_os = "linux")]
mod linux;
mod lists;
#[cfg(windows)]
mod windows;

use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tao::event_loop::EventLoopProxy;

use crate::{events::UserEvent, storage::Paths, tabs::TabId};

/// Папка блокировщика в профиле: скачанные списки, счётчик, правила WebKit.
const FOLDER: &str = "adblock";
const STATE: &str = "state.json";
/// Не вышло скачать списки — следующая попытка не раньше чем через час.
const RETRY: Duration = Duration::from_secs(60 * 60);
/// Первая проверка списков — не сразу при запуске: сначала открываются вкладки.
const FIRST_CHECK: Duration = Duration::from_secs(20);

/// Скрипт страницы: косметика на Windows. На Linux элементы скрывает сам WebKit.
#[cfg(windows)]
pub const PAGE_SCRIPT: &str = include_str!("cosmetic.js");
#[cfg(not(windows))]
pub const PAGE_SCRIPT: &str = "";

/// Настройки блокировщика.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    pub enabled: bool,
    /// Скачивать свежие списки.
    pub updates: bool,
    /// Сайты, на которых ничего не блокировать (вид `sleep::site`).
    pub sites: Vec<String>,
}

/// Что приходит в главный цикл.
pub enum Event {
    /// Списки прочитаны, движок (Windows) или правила для WebKit (Linux) готовы.
    Built(Box<Built>),
    /// Проверка списков закончилась: какие скачаны заново, или почему не вышло.
    Checked(Result<Vec<&'static str>, String>),
    /// Во вкладках что-то заблокировано: счётчик в интерфейсе устарел.
    Counted,
    /// Linux: WebKit скомпилировал правила (или не смог).
    #[cfg(target_os = "linux")]
    Compiled(Result<(), String>),
    /// Linux: готовых правил в профиле не оказалось — собрать заново.
    #[cfg(target_os = "linux")]
    Stale,
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Built(built) => write!(f, "Built({:?})", built.lists),
            Self::Checked(result) => write!(f, "Checked({result:?})"),
            Self::Counted => write!(f, "Counted"),
            #[cfg(target_os = "linux")]
            Self::Compiled(result) => write!(f, "Compiled({result:?})"),
            #[cfg(target_os = "linux")]
            Self::Stale => write!(f, "Stale"),
        }
    }
}

pub struct Built {
    lists: Vec<ListInfo>,
    #[cfg(windows)]
    engine: std::sync::Arc<adblock::Engine>,
    #[cfg(target_os = "linux")]
    rules: linux::Rules,
    took: Duration,
}

/// Список фильтров, как его показывают настройки.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListInfo {
    #[serde(skip)]
    pub id: &'static str,
    pub title: &'static str,
    pub version: u64,
    pub modified: String,
    pub downloaded: bool,
}

/// Состояние для настроек и окошка в адресной строке.
#[derive(Debug, Serialize)]
pub struct View<'a> {
    pub enabled: bool,
    pub updates: bool,
    pub sites: &'a [String],
    pub lists: &'a [ListInfo],
    /// Когда списки последний раз проверяли (секунды Unix), 0 — ни разу.
    pub checked: u64,
    pub checking: bool,
    pub error: Option<&'a str>,
    /// Заблокировано за всё время.
    pub total: u64,
    /// Движок готов: блокировка уже работает.
    pub ready: bool,
}

/// Что лежит в `state.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    checked: u64,
    total: u64,
}

// ---------- горячее состояние: его читают обработчики запросов ----------

/// То, что нужно на каждый запрос и в обработчиках webview. Живёт в потоке
/// интерфейса, как и сами обработчики.
#[derive(Default)]
struct Hot {
    enabled: bool,
    ready: bool,
    sites: Vec<String>,
    /// Заблокировано на нынешней странице вкладки.
    counts: HashMap<TabId, u32>,
    total: u64,
    events: Option<EventLoopProxy<UserEvent>>,
    /// Событие `Counted` уже в очереди: второе не нужно.
    pending: bool,
}

thread_local! {
    static HOT: RefCell<Hot> = RefCell::new(Hot::default());
}

/// Движок (или правила WebKit) готов: блокировка работает.
fn mark_ready() {
    HOT.with(|h| h.borrow_mut().ready = true);
}

/// Блокировать сейчас: включено и движок готов.
fn active() -> bool {
    HOT.with(|h| {
        let h = h.borrow();
        h.enabled && h.ready
    })
}

/// На сайте страницы `url` ничего не блокируем: выключено вовсе или для этого сайта.
pub fn site_allowed(url: &str) -> bool {
    HOT.with(|h| {
        let h = h.borrow();
        !h.enabled || crate::tabs::sleep::site_kept(&h.sites, url)
    })
}

/// Заблокировано на странице вкладки.
pub fn blocked(tab: TabId) -> u32 {
    HOT.with(|h| h.borrow().counts.get(&tab).copied().unwrap_or(0))
}

/// Ещё один запрос вкладки заблокирован.
fn count(tab: TabId) {
    HOT.with(|h| {
        let mut h = h.borrow_mut();
        *h.counts.entry(tab).or_insert(0) += 1;
        h.total += 1;
        notify(&mut h);
    });
}

/// Во вкладке началась новая страница: её счёт — с нуля.
fn reset(tab: TabId) {
    HOT.with(|h| {
        let mut h = h.borrow_mut();
        if h.counts.remove(&tab).is_some_and(|n| n > 0) {
            notify(&mut h);
        }
    });
}

fn notify(h: &mut Hot) {
    if !h.pending
        && let Some(events) = &h.events
    {
        h.pending = events
            .send_event(UserEvent::Adblock(Event::Counted))
            .is_ok();
    }
}

/// Вкладка закрыта: её счёт больше не нужен.
pub fn forget(tab: TabId) {
    HOT.with(|h| h.borrow_mut().counts.remove(&tab));
    #[cfg(windows)]
    windows::forget(tab);
}

/// Подключает блокировку к webview вкладки. Звать сразу после сборки, до загрузки.
pub fn install(webview: &wry::WebView, tab: TabId) {
    #[cfg(windows)]
    windows::install(webview, tab);
    #[cfg(target_os = "linux")]
    linux::install(webview, tab);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (webview, tab);
}

// ---------- списки, сборка, обновления: живёт в App ----------

pub struct Manager {
    dir: PathBuf,
    events: EventLoopProxy<UserEvent>,
    updates: bool,
    saved: Saved,
    /// `total` поменялся, а файл ещё не записан.
    dirty: bool,
    lists: Vec<ListInfo>,
    building: bool,
    /// Пока собирали, списки снова поменялись: собрать ещё раз.
    rebuild: bool,
    checking: bool,
    /// Раньше этого времени списки не проверять (после неудачи, при запуске).
    not_before: Instant,
    error: Option<String>,
}

impl Manager {
    /// Читает списки и собирает движок в фоне.
    pub fn start(paths: &Paths, prefs: &Prefs, events: EventLoopProxy<UserEvent>) -> Self {
        let dir = paths.root.join(FOLDER);
        let saved: Saved = crate::storage::read_json(&dir.join(STATE)).unwrap_or_default();
        HOT.with(|h| {
            let mut h = h.borrow_mut();
            h.enabled = prefs.enabled;
            h.sites = prefs.sites.clone();
            h.total = saved.total;
            h.events = Some(events.clone());
        });
        #[cfg(target_os = "linux")]
        linux::prepare(&dir);
        let mut manager = Self {
            dir,
            events,
            updates: prefs.updates,
            saved,
            dirty: false,
            lists: Vec::new(),
            building: false,
            rebuild: false,
            checking: false,
            not_before: Instant::now() + FIRST_CHECK,
            error: None,
        };
        // Выключена — ни движка, ни списков в памяти: соберутся, когда включат.
        if prefs.enabled {
            manager.load();
        }
        manager
    }

    /// Движок (Linux — правила WebKit) ещё не собран и не собирается.
    fn idle(&self) -> bool {
        !self.building && !HOT.with(|h| h.borrow().ready)
    }

    /// Готовит блокировку: собирает движок или (Linux) читает готовые правила.
    fn load(&mut self) {
        // Linux: правила для тех же списков уже скомпилированы — WebKit их только
        // прочитает, а списки и движок не нужны вовсе.
        #[cfg(target_os = "linux")]
        if let Some(lists) = linux::compiled(&self.dir) {
            self.lists = lists;
            self.building = true;
            linux::load(self.events.clone());
            return;
        }
        self.build();
    }

    fn build(&mut self) {
        if self.building {
            self.rebuild = true;
            return;
        }
        self.building = true;
        let (dir, events) = (self.dir.clone(), self.events.clone());
        let spawned = thread::Builder::new()
            .name("adblock".into())
            .spawn(move || {
                let built = build(&dir);
                let _ = events.send_event(UserEvent::Adblock(Event::Built(Box::new(built))));
            });
        if let Err(err) = spawned {
            eprintln!("блокировка рекламы: поток не запустился: {err}");
            self.building = false;
        }
    }

    /// Новые настройки. `true` — включили, выключили или поменяли список сайтов:
    /// открытым страницам нужна перезагрузка (или хотя бы новые правила).
    pub fn set_prefs(&mut self, prefs: &Prefs) -> bool {
        self.updates = prefs.updates;
        let changed = HOT.with(|h| {
            let mut h = h.borrow_mut();
            let changed = h.enabled != prefs.enabled || h.sites != prefs.sites;
            h.enabled = prefs.enabled;
            h.sites = prefs.sites.clone();
            changed
        });
        if changed {
            #[cfg(target_os = "linux")]
            linux::refresh();
        }
        if prefs.enabled && self.idle() {
            self.load();
        }
        // Выключили — движок больше не нужен: память отдаём.
        #[cfg(windows)]
        if !prefs.enabled && HOT.with(|h| h.borrow().ready) {
            windows::drop_engine();
            HOT.with(|h| h.borrow_mut().ready = false);
        }
        changed
    }

    /// Счётчик в интерфейсе обновлён: следующий заблокированный запрос пришлёт событие.
    pub fn counted(&mut self) {
        HOT.with(|h| {
            let mut h = h.borrow_mut();
            h.pending = false;
            if h.total != self.saved.total {
                self.saved.total = h.total;
                self.dirty = true;
            }
        });
    }

    /// Событие блокировщика. `true` — поменялось то, что видно в настройках.
    pub fn on_event(&mut self, event: Event) -> bool {
        match event {
            Event::Built(built) => {
                eprintln!(
                    "блокировка рекламы: списки готовы за {} мс ({})",
                    built.took.as_millis(),
                    built
                        .lists
                        .iter()
                        .map(|l| format!("{} {}", l.title, l.version))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                self.lists = built.lists;
                // Пока собирали, блокировку выключили — движок не нужен.
                #[cfg(windows)]
                {
                    self.building = false;
                    if HOT.with(|h| h.borrow().enabled) {
                        windows::set_engine(built.engine);
                        mark_ready();
                    }
                    if std::mem::take(&mut self.rebuild) {
                        self.build();
                    }
                }
                // Linux: сборка закончится, когда WebKit скомпилирует правила (`Compiled`).
                #[cfg(target_os = "linux")]
                linux::compile(built.rules, self.events.clone());
                true
            }
            Event::Checked(result) => {
                self.checking = false;
                match result {
                    Ok(changed) => {
                        self.error = None;
                        self.saved.checked = now();
                        self.dirty = true;
                        if !changed.is_empty() {
                            eprintln!("блокировка рекламы: новые списки — {}", changed.join(", "));
                            self.build();
                        }
                    }
                    Err(err) => {
                        self.error = Some(err);
                        self.not_before = Instant::now() + RETRY;
                    }
                }
                self.save_if_dirty();
                true
            }
            Event::Counted => {
                self.counted();
                false
            }
            #[cfg(target_os = "linux")]
            Event::Compiled(result) => {
                self.building = false;
                if let Err(err) = result {
                    self.error = Some(err);
                }
                if std::mem::take(&mut self.rebuild) {
                    self.build();
                }
                true
            }
            #[cfg(target_os = "linux")]
            Event::Stale => {
                self.building = false;
                self.build();
                false
            }
        }
    }

    /// Раз в полминуты: пора ли проверить списки. `true` — проверка началась.
    pub fn tick(&mut self) -> bool {
        self.save_if_dirty();
        let due = now().saturating_sub(self.saved.checked) >= lists::UPDATE_EVERY.as_secs();
        // Блокировка выключена — и списки качать незачем.
        let enabled = HOT.with(|h| h.borrow().enabled);
        if self.updates && enabled && due && Instant::now() >= self.not_before {
            return self.check();
        }
        false
    }

    /// Проверить списки сейчас (кнопка в настройках). `true` — проверка началась.
    pub fn check(&mut self) -> bool {
        if self.checking {
            return false;
        }
        self.checking = true;
        let (dir, events) = (self.dir.clone(), self.events.clone());
        let spawned = thread::Builder::new()
            .name("adblock-update".into())
            .spawn(move || {
                let result =
                    lists::update(&dir).map_err(|err| format!("не вышло скачать списки: {err:#}"));
                let _ = events.send_event(UserEvent::Adblock(Event::Checked(result)));
            });
        if spawned.is_err() {
            self.checking = false;
        }
        self.checking
    }

    pub fn save_if_dirty(&mut self) {
        let total = HOT.with(|h| h.borrow().total);
        if total != self.saved.total {
            self.saved.total = total;
            self.dirty = true;
        }
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let result = crate::storage::private_dir(&self.dir)
            .map_err(anyhow::Error::from)
            .and_then(|()| crate::storage::write_json(&self.dir.join(STATE), &self.saved));
        if let Err(err) = result {
            eprintln!("блокировка рекламы: счётчик не сохранён: {err:#}");
        }
    }

    pub fn view<'a>(&'a self, prefs: &'a Prefs) -> View<'a> {
        let (total, ready) = HOT.with(|h| {
            let h = h.borrow();
            (h.total, h.ready)
        });
        View {
            enabled: prefs.enabled,
            updates: prefs.updates,
            sites: &prefs.sites,
            lists: &self.lists,
            checked: self.saved.checked,
            checking: self.checking,
            error: self.error.as_deref(),
            total,
            ready,
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Читает списки и собирает из них движок (в фоновом потоке).
fn build(dir: &Path) -> Built {
    let started = Instant::now();
    domain::install();
    let loaded = lists::load(dir);
    let lists = loaded
        .iter()
        .map(|l| ListInfo {
            id: l.id,
            title: l.title,
            version: l.version,
            modified: l.modified.clone(),
            downloaded: l.downloaded,
        })
        .collect::<Vec<ListInfo>>();
    let texts: Vec<String> = loaded.into_iter().map(|l| l.text).collect();
    #[cfg(windows)]
    let engine = std::sync::Arc::new(engine(texts));
    #[cfg(target_os = "linux")]
    let rules = linux::Rules::build(dir, texts, &lists);
    #[cfg(not(any(windows, target_os = "linux")))]
    drop(texts);
    Built {
        lists,
        #[cfg(windows)]
        engine,
        #[cfg(target_os = "linux")]
        rules,
        took: started.elapsed(),
    }
}

/// Движок из текстов списков.
#[cfg_attr(not(windows), allow(dead_code))]
fn engine(texts: Vec<String>) -> adblock::Engine {
    domain::install();
    let mut set = adblock::lists::FilterSet::new(false);
    for text in texts {
        set.add_filter_list(text, adblock::lists::ParseOptions::default());
    }
    adblock::Engine::new_with_filter_set(set)
}

/// Тип запроса для движка: как его называют правила (`$script`, `$image`…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Kind {
    Frame,
    Script,
    Image,
    Stylesheet,
    Media,
    Xhr,
    Ping,
    Other,
}

impl Kind {
    #[cfg_attr(not(windows), allow(dead_code))]
    fn as_str(self) -> &'static str {
        match self {
            Self::Frame => "sub_frame",
            Self::Script => "script",
            Self::Image => "image",
            Self::Stylesheet => "stylesheet",
            Self::Media => "media",
            Self::Xhr => "xmlhttprequest",
            Self::Ping => "ping",
            Self::Other => "other",
        }
    }
}

/// Блокировать ли запрос `url` со страницы `page`.
#[cfg_attr(not(windows), allow(dead_code))]
fn should_block(engine: &adblock::Engine, url: &str, page: &str, kind: Kind) -> bool {
    match adblock::request::Request::new(url, page, kind.as_str(), "get") {
        Ok(request) => engine.check_network_request(&request).should_block(),
        Err(_) => false,
    }
}

// ---------- косметика: ответы скрипту страницы (cosmetic.js) ----------

/// Больше классов и id за раз не принимаем: страница может прислать что угодно.
const MAX_NAMES: usize = 2000;
const MAX_NAME: usize = 200;
/// Столько последних документов помним (их исключения из общих правил).
const RECENT: usize = 16;

/// Что спрашивает скрипт страницы.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Ask {
    /// Документ только создан: нужны правила его сайта.
    start: bool,
    /// Классы и id, которых скрипт ещё не спрашивал.
    c: Vec<String>,
    i: Vec<String>,
}

/// Правила сайта документа, которые нужны для общих правил: исключения
/// (`site.ru#@#.ad`) и `$generichide`.
#[cfg_attr(not(windows), allow(dead_code))]
struct Known {
    url: String,
    exceptions: std::collections::HashSet<String>,
    generichide: bool,
}

#[derive(Default)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Cosmetic {
    recent: std::collections::VecDeque<Known>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Cosmetic {
    /// Ответ скрипту документа `url` (JSON). `off` — на этом сайте не блокируем.
    /// `None` — отвечать нечего.
    fn answer(
        &mut self,
        engine: &adblock::Engine,
        url: &str,
        off: bool,
        raw: &str,
    ) -> Option<String> {
        let ask: Ask = serde_json::from_str(raw).ok()?;
        if off {
            return Some(r#"{"axab":1,"off":true}"#.into());
        }
        if ask.start {
            let resources = engine.url_cosmetic_resources(url);
            let procedural: Vec<serde_json::Value> = resources
                .procedural_actions
                .iter()
                .filter_map(|p| serde_json::from_str(p).ok())
                .collect();
            let reply = serde_json::json!({
                "axab": 1,
                "s": resources.hide_selectors,
                "p": procedural,
                "g": !resources.generichide,
            });
            self.remember(Known {
                url: url.to_string(),
                exceptions: resources.exceptions,
                generichide: resources.generichide,
            });
            return Some(reply.to_string());
        }
        let names = |list: Vec<String>| -> Vec<String> {
            list.into_iter()
                .filter(|n| !n.is_empty() && n.len() <= MAX_NAME)
                .take(MAX_NAMES)
                .collect()
        };
        let (classes, ids) = (names(ask.c), names(ask.i));
        if classes.is_empty() && ids.is_empty() {
            return None;
        }
        let known = match self.recent.iter().position(|k| k.url == url) {
            Some(i) => self.recent.remove(i)?,
            None => {
                let resources = engine.url_cosmetic_resources(url);
                Known {
                    url: url.to_string(),
                    exceptions: resources.exceptions,
                    generichide: resources.generichide,
                }
            }
        };
        let selectors = if known.generichide {
            Vec::new()
        } else {
            engine.hidden_class_id_selectors(&classes, &ids, &known.exceptions)
        };
        self.remember(known);
        (!selectors.is_empty())
            .then(|| serde_json::json!({ "axab": 1, "s": selectors }).to_string())
    }

    fn remember(&mut self, known: Known) {
        self.recent.retain(|k| k.url != known.url);
        if self.recent.len() >= RECENT {
            self.recent.pop_back();
        }
        self.recent.push_front(known);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::OnceLock;

    use super::*;

    /// Движок на встроенных списках — один на все тесты.
    fn real() -> &'static adblock::Engine {
        static ENGINE: OnceLock<adblock::Engine> = OnceLock::new();
        ENGINE.get_or_init(|| {
            let dir = std::env::temp_dir().join("axiom-adblock-no-such-dir");
            let texts = lists::load(&dir).into_iter().map(|l| l.text).collect();
            engine(texts)
        })
    }

    #[test]
    fn blocks_ads_from_both_lists() {
        let e = real();
        for (url, page, kind) in [
            (
                "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
                "https://example.com/",
                Kind::Script,
            ),
            (
                "https://securepubads.g.doubleclick.net/tag/js/gpt.js",
                "https://news.example/",
                Kind::Script,
            ),
            (
                "https://an.yandex.ru/system/context.js",
                "https://lenta.ru/",
                Kind::Script,
            ),
            (
                "https://yandex.ru/ads/system/context.js",
                "https://lenta.ru/",
                Kind::Script,
            ),
            (
                "https://ads.adfox.ru/123/getBulk",
                "https://www.rbc.ru/",
                Kind::Xhr,
            ),
            (
                "https://googleads.g.doubleclick.net/pagead/ads?client=1",
                "https://example.com/",
                Kind::Frame,
            ),
            (
                "https://example.com/banners/ad_728x90.gif",
                "https://example.com/",
                Kind::Image,
            ),
        ] {
            assert!(should_block(e, url, page, kind), "{url}");
        }
    }

    #[test]
    fn leaves_ordinary_requests() {
        let e = real();
        for (url, page, kind) in [
            (
                "https://cdn.jsdelivr.net/npm/jquery@3/dist/jquery.min.js",
                "https://example.com/",
                Kind::Script,
            ),
            (
                "https://www.google.com/recaptcha/api.js",
                "https://example.com/",
                Kind::Script,
            ),
            (
                "https://i.ytimg.com/vi/abc/hqdefault.jpg",
                "https://www.youtube.com/",
                Kind::Image,
            ),
            (
                "https://yastatic.net/s3/home-static/_/main.css",
                "https://ya.ru/",
                Kind::Stylesheet,
            ),
            (
                "https://lenta.ru/news/2026/11/27/",
                "https://lenta.ru/",
                Kind::Xhr,
            ),
            (
                "https://github.githubassets.com/assets/app.js",
                "https://github.com/",
                Kind::Script,
            ),
            // Не веб-адрес — движку не по зубам, не блокируем.
            (
                "data:image/png;base64,AAAA",
                "https://example.com/",
                Kind::Image,
            ),
        ] {
            assert!(!should_block(e, url, page, kind), "{url}");
        }
    }

    #[test]
    fn cosmetic_rules_for_sites_and_classes() {
        let e = real();
        // Правила сайта из RuAdList и общие правила (не по классам).
        let lenta = e.url_cosmetic_resources("https://lenta.ru/");
        assert!(!lenta.generichide);
        assert!(lenta.hide_selectors.len() > 100);
        // Общие правила по классам и id.
        let generic = e.hidden_class_id_selectors(
            ["adsbygoogle-box", "SC_TBlock", "content"],
            ["main"],
            &lenta.exceptions,
        );
        assert!(
            generic.iter().any(|s| s == ".adsbygoogle-box"),
            "{generic:?}"
        );
        assert!(generic.iter().any(|s| s == ".SC_TBlock"), "{generic:?}");
        assert!(!generic.iter().any(|s| s == ".content"));
    }

    #[test]
    fn answers_page_script() {
        let e = real();
        let mut c = Cosmetic::default();
        let url = "https://lenta.ru/news/1";
        let start: serde_json::Value = serde_json::from_str(
            &c.answer(e, url, false, r#"{"axab":1,"start":true}"#)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(start["g"], true);
        assert!(start["s"].as_array().unwrap().len() > 100);
        let generic = c
            .answer(
                e,
                url,
                false,
                r#"{"axab":1,"c":["adsbygoogle-box","content"],"i":["main"]}"#,
            )
            .unwrap();
        assert!(generic.contains(".adsbygoogle-box"), "{generic}");
        // Нечего скрывать — нечего и отвечать.
        assert_eq!(
            c.answer(e, url, false, r#"{"axab":1,"c":["no-such-class-x1"]}"#),
            None
        );
        assert_eq!(c.answer(e, url, false, r#"{"axab":1}"#), None);
        assert_eq!(c.answer(e, url, false, "не json"), None);
        // Сайт в списке «не блокировать».
        assert_eq!(
            c.answer(e, url, true, r#"{"axab":1,"start":true}"#)
                .as_deref(),
            Some(r#"{"axab":1,"off":true}"#)
        );
        // Мусор от страницы: длинные и пустые имена отбрасываются, документы — не больше RECENT.
        let junk = format!(r#"{{"axab":1,"c":["{}",""]}}"#, "a".repeat(MAX_NAME + 1));
        assert_eq!(c.answer(e, url, false, &junk), None);
        for n in 0..RECENT + 5 {
            let _ = c.answer(
                e,
                &format!("https://s{n}.example/"),
                false,
                r#"{"axab":1,"start":true}"#,
            );
        }
        assert_eq!(c.recent.len(), RECENT);
    }

    #[test]
    fn checks_take_microseconds() {
        let e = real();
        let urls = [
            "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
            "https://cdn.example.com/static/app.0123456789.js",
            "https://www.example.com/images/photo.jpg?w=640",
            "https://mc.yandex.ru/watch/123456",
        ];
        let rounds = 2000u32;
        let started = Instant::now();
        for _ in 0..rounds {
            for url in urls {
                let _ = should_block(e, url, "https://news.example.com/", Kind::Script);
            }
        }
        let each = started.elapsed() / (rounds * urls.len() as u32);
        // В выпуске ~2 мкс; в тестах наш код без оптимизации (движок — с ней,
        // см. Cargo.toml) и машина CI медленнее.
        assert!(each < Duration::from_micros(100), "{each:?} на запрос");
    }

    #[test]
    fn page_counters() {
        let tab = 9_000_001;
        assert_eq!(blocked(tab), 0);
        count(tab);
        count(tab);
        assert_eq!(blocked(tab), 2);
        reset(tab);
        assert_eq!(blocked(tab), 0);
        count(tab);
        forget(tab);
        assert_eq!(blocked(tab), 0);
    }

    #[test]
    fn allowed_sites() {
        HOT.with(|h| {
            let mut h = h.borrow_mut();
            h.enabled = true;
            h.sites = vec!["example.com".into()];
        });
        assert!(site_allowed("https://www.example.com/page"));
        assert!(site_allowed("https://example.com/"));
        assert!(!site_allowed("https://notexample.com/"));
        assert!(!site_allowed("https://lenta.ru/"));
        HOT.with(|h| h.borrow_mut().enabled = false);
        assert!(site_allowed("https://lenta.ru/"));
    }
}
