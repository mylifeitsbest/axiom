//! Протокол между интерфейсом (JS) и Rust.
//!
//! Все входящие сообщения проходят через [`parse`]: ограничение размера,
//! строгая схема serde и проверка значений. Сайты в вкладках IPC не имеют.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

/// Максимальный размер одного сообщения от интерфейса.
const MAX_MESSAGE: usize = 256 * 1024;
/// Максимальная длина ввода адресной строки.
pub const MAX_INPUT: usize = 8 * 1024;
/// Максимальный размер настроек внешнего вида (включая свой CSS).
const MAX_APPEARANCE: usize = 96 * 1024;
/// Больше записей истории за один запрос интерфейс не получит.
pub const MAX_HISTORY_PAGE: usize = 1000;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("сообщение слишком большое: {0} байт")]
    TooLarge(usize),
    #[error("неверный формат: {0}")]
    Format(#[from] serde_json::Error),
    #[error("неверное значение: {0}")]
    Invalid(&'static str),
}

impl IpcError {
    /// Что сказать человеку, если отказ — следствие его ввода, а не ошибка интерфейса.
    /// Иначе, например, слишком большой свой CSS молча отменял бы сохранение всех настроек.
    pub fn user_text(&self) -> Option<&'static str> {
        match self {
            Self::TooLarge(_) | Self::Invalid("appearance") => {
                Some("Свой CSS слишком большой — настройки не сохранены")
            }
            Self::Invalid("input" | "query" | "report") => Some("Слишком длинный текст"),
            Self::Invalid("report_empty") => Some("Опишите, что не работает"),
            Self::Invalid("proxy") => Some("Слишком длинный адрес прокси"),
            _ => None,
        }
    }
}

/// Прямоугольник в логических пикселях (CSS px интерфейса).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn validate(&self) -> Result<(), IpcError> {
        let ok = [self.x, self.y, self.w, self.h]
            .iter()
            .all(|v| v.is_finite() && (-1.0..=100_000.0).contains(v))
            && self.w >= 0.0
            && self.h >= 0.0;
        if ok {
            Ok(())
        } else {
            Err(IpcError::Invalid("rect"))
        }
    }
}

/// Чёлка островка в окне (логические px): середина, верх острова с видео и его ширина,
/// верх самой чёлки (у карточки в боковой панели — не край окна), ширина чёлки, пока
/// видео нет (`from`, 0 — как у острова: из неё остров раздаётся вширь), и скорость
/// анимаций интерфейса (`speed`, 0 — без анимаций): остров выезжает в такт чёлке.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct IslandDock {
    pub cx: f64,
    pub top: f64,
    pub width: f64,
    #[serde(default)]
    pub notch_top: f64,
    #[serde(default)]
    pub from: f64,
    #[serde(default = "one")]
    pub speed: f64,
}

fn one() -> f64 {
    1.0
}

impl IslandDock {
    fn validate(&self) -> Result<(), IpcError> {
        let ok = [self.cx, self.top, self.width, self.notch_top, self.from]
            .iter()
            .all(|v| v.is_finite() && (0.0..=100_000.0).contains(v))
            && (120.0..=2000.0).contains(&self.width)
            && self.from <= 2000.0
            && self.speed.is_finite()
            && (0.0..=3.0).contains(&self.speed)
            && self.notch_top <= self.top;
        if ok {
            Ok(())
        } else {
            Err(IpcError::Invalid("dock"))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowAction {
    Minimize,
    Maximize,
    Close,
    Drag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    N,
    S,
    E,
    W,
    Ne,
    Nw,
    Se,
    Sw,
}

/// Настройки, которые интерфейс присылает на сохранение.
#[derive(Debug, Clone, Deserialize)]
pub struct SettingsIn {
    pub appearance: Map<String, Value>,
    /// Неизвестная система — DuckDuckGo; свой адрес проверяют настройки.
    #[serde(default, deserialize_with = "crate::tabs::omnibox::lenient_engine")]
    pub search_engine: crate::tabs::omnibox::Engine,
    #[serde(default)]
    pub search_custom: String,
    pub double_search: bool,
    /// Адрес прокси для вкладок; проверяет `net::address`. Нет поля — не менять.
    #[serde(default)]
    pub proxy: Option<String>,
    pub sleep_minutes: u32,
    pub live_tabs: u32,
    /// Сайты «не усыплять»; чистят настройки. Нет поля или не список — не менять.
    #[serde(default, deserialize_with = "crate::tabs::identity::lenient")]
    pub no_sleep_sites: Option<Vec<String>>,
    #[serde(default)]
    pub onboarded: bool,
    #[serde(default)]
    pub force_dark: bool,
    #[serde(default = "yes")]
    pub check_updates: bool,
    /// Убирать метки слежки из адресов. Нет поля — не менять.
    #[serde(default)]
    pub strip_tracking: Option<bool>,
    /// Отправлять отчёты об ошибках. Нет поля — не менять.
    #[serde(default)]
    pub crash_reports: Option<bool>,
    /// Статистика использования. Нет поля — не менять.
    #[serde(default)]
    pub usage_stats: Option<bool>,
    /// Блокировка рекламы и обновление её списков. Нет поля — не менять.
    #[serde(default)]
    pub adblock: Option<bool>,
    #[serde(default)]
    pub adblock_updates: Option<bool>,
    /// Подсказки поисковой системы. Нет поля — не менять.
    #[serde(default)]
    pub search_suggest: Option<bool>,
    /// Новый разговор в панели ИИ при каждом открытии. Нет поля или null — не менять.
    #[serde(default)]
    pub ai_new_chat: Option<bool>,
    /// Бета-версии. Нет поля или null — не менять.
    #[serde(default)]
    pub beta: Option<bool>,
    /// Отрисовка страниц на Linux. Нет поля — не менять.
    #[serde(default)]
    pub gpu: Option<crate::system::gpu::Gpu>,
    /// Подмена User-Agent и Referer; нет поля — не менять, неизвестный выбор — тоже.
    #[serde(default, deserialize_with = "crate::tabs::identity::lenient")]
    pub user_agent: Option<crate::tabs::identity::Agent>,
    #[serde(default)]
    pub user_agent_custom: Option<String>,
    #[serde(default, deserialize_with = "crate::tabs::identity::lenient")]
    pub referer: Option<crate::tabs::identity::Referer>,
    #[serde(default)]
    pub referer_custom: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum UiCmd {
    Ready,
    Layout {
        content: Rect,
        ai: Option<Rect>,
    },
    Navigate {
        input: String,
    },
    NewTab {
        #[serde(default)]
        input: Option<String>,
    },
    CloseTab {
        id: u64,
    },
    ActivateTab {
        id: u64,
    },
    MoveTab {
        id: u64,
        to: usize,
    },
    RestoreTab,
    Back,
    Forward,
    Reload,
    Hotkey {
        action: String,
    },
    Ai {
        open: bool,
        #[serde(default)]
        query: Option<String>,
    },
    AiPopOut,
    Overlay {
        open: bool,
    },
    /// Снимок страницы лёг на место: теперь её webview можно прятать.
    SnapshotShown {
        seq: u64,
    },
    /// Кнопка настроек, Ctrl+,: открыть вкладку настроек (или перейти на неё).
    OpenSettings,
    /// Кнопка прокси на панели.
    Proxy {
        on: bool,
    },
    /// Лист закрытых настроек плавно ушёл: можно показывать страницу под ним.
    SettingsLeft,
    /// Ctrl+H, ссылка «История»: открыть вкладку истории (или перейти на неё).
    OpenHistory,
    /// Страница истории: `limit` записей с `offset`, свежие сверху; `q` — поиск по всей истории.
    History {
        #[serde(default)]
        q: String,
        #[serde(default)]
        offset: usize,
        limit: usize,
    },
    /// Клик по записи истории: открыть в этой же вкладке, `background` — в новой за ней.
    HistoryOpen {
        url: String,
        #[serde(default)]
        background: bool,
    },
    /// Крестик у записи истории.
    HistoryDelete {
        url: String,
    },
    /// «Очистить историю»: адреса, открытые с `since` (Unix-секунды; 0 — вся история).
    HistoryClear {
        since: u64,
    },
    /// Сколько адресов уйдёт при очистке с `since`.
    HistoryCount {
        since: u64,
    },
    /// Shift+Esc, пункт меню: открыть диспетчер задач (или перейти на него).
    OpenTasks,
    /// Свежий замер памяти и процессора: диспетчер задач на экране (раз в секунду)
    /// или карточка вкладки под курсором.
    Tasks,
    /// «Усыпить» у вкладки в диспетчере: выгрузить её страницу из памяти.
    TasksSleep {
        id: u64,
    },
    /// «Усыпить фоновые»: все вкладки, которые можно усыпить.
    TasksSleepAll,
    Suggest {
        q: String,
    },
    SaveSettings {
        settings: SettingsIn,
    },
    ThemeColors {
        window: String,
        page: String,
        /// Тема для сайтов (`prefers-color-scheme`): dark, light или auto.
        #[serde(default)]
        scheme: String,
    },
    Window {
        action: WindowAction,
    },
    ResizeWindow {
        dir: Edge,
    },
    Find {
        q: String,
        backwards: bool,
    },
    /// Звёздочка в адресной строке: добавить или убрать закладку активной вкладки.
    ToggleBookmark,
    /// Кнопка «не усыплять» в адресной строке.
    ToggleNoSleep {
        id: u64,
    },
    /// Меню вкладки: «Не усыплять на этом сайте» и «Снова усыплять на этом сайте».
    ToggleSiteNoSleep {
        id: u64,
    },
    /// Крестик на плитке новой вкладки (`hidden: true`) и «Вернуть» (`false`).
    HideSite {
        url: String,
        hidden: bool,
    },
    /// Меню вкладки и значок динамика: команды получают только номер вкладки.
    ReloadTab {
        id: u64,
    },
    DuplicateTab {
        id: u64,
    },
    ToggleMute {
        id: u64,
    },
    /// Островок: спросить, что играет во вкладке, и (если есть) нажать кнопку.
    Media {
        id: u64,
        #[serde(default)]
        action: Option<crate::tabs::media::Action>,
    },
    /// Где чёлка островка: под ней встаёт остров с видео. Без `dock` — чёлки нет.
    IslandDock {
        #[serde(default)]
        dock: Option<IslandDock>,
    },
    /// Ползунок в меню вкладки и колесо над значком динамика: громкость 0–100.
    SetVolume {
        id: u64,
        volume: u8,
    },
    /// Кнопки окошка масштаба: −, + и «Сбросить» для активной вкладки.
    Zoom {
        step: crate::tabs::zoom::Zoom,
    },
    /// Окошко масштаба показалось (где оно и радиус углов, CSS px) или спряталось (`null`):
    /// под ним в странице прорезается вырез.
    ZoomBubble {
        rect: Option<Rect>,
        #[serde(default)]
        radius: f64,
    },
    CloseOtherTabs {
        id: u64,
    },
    CloseTabsRight {
        id: u64,
    },
    /// Перенести закладки и историю из другого браузера.
    Import {
        browser: crate::storage::import::Browser,
    },
    /// Установить в систему и зарегистрировать как браузер.
    Install,
    /// Открыть «Приложения по умолчанию» в параметрах Windows.
    OpenDefaultApps,
    /// Загрузки: интерфейс знает только номер, путь к файлу хранит Rust.
    DownloadOpen {
        id: u64,
    },
    DownloadShow {
        id: u64,
    },
    DownloadCancel {
        id: u64,
    },
    /// «Сохранить всё равно» для файла опасного типа («Удалить» — это DownloadCancel).
    DownloadKeep {
        id: u64,
    },
    DownloadRemove {
        id: u64,
    },
    DownloadsClear,
    DownloadsFolder,
    /// «Проверить» в настройках.
    UpdateCheck,
    /// «Обновить»: скачать новую версию и перезапуститься.
    UpdateInstall,
    /// «Что нового»: изменения в доступном обновлении или в этой версии.
    UpdateNotes {
        which: crate::update::NotesWhich,
    },
    /// Кнопка перевода в адресной строке: перевести активную вкладку.
    Translate,
    /// «Показать оригинал» в окошке перевода.
    TranslateOriginal,
    /// Язык перевода, «Всегда переводить» и «Предлагать перевод».
    TranslatePrefs {
        prefs: crate::translate::Prefs,
    },
    /// Ответ в окошке «Открыть приложение?». Что открывать, знает только Rust.
    AppLink {
        id: u64,
        open: bool,
        #[serde(default)]
        always: bool,
    },
    /// Ответ в окошке «Сайт хочет…».
    Permission {
        id: u64,
        answer: PermAnswer,
    },
    /// Настройки → Безопасность: решение сайта по одному разрешению; `null` — спрашивать.
    SitePermission {
        site: String,
        kind: crate::storage::permissions::Perm,
        decision: Option<crate::storage::permissions::Decision>,
    },
    /// «Забыть сайт»: все его разрешения снова под вопросом.
    ForgetSite {
        site: String,
    },
    /// Блокировать рекламу на сайте активной вкладки (`on`) или нет; страница
    /// перезагружается.
    AdblockSite {
        on: bool,
    },
    /// Сайт — в список «не блокировать» (вписали в настройках) или из него.
    AdblockAllow {
        site: String,
    },
    AdblockForget {
        site: String,
    },
    /// Проверить списки фильтров сейчас.
    AdblockUpdate,
    /// «Не спрашивать» для всех сайтов (`block`) или снова спрашивать.
    PermissionDefault {
        kind: crate::storage::permissions::Perm,
        block: bool,
    },
    /// Убрать «Всегда разрешать» у ссылки на приложение.
    ForgetAppLink {
        site: String,
        scheme: String,
    },
    /// Открыли «Сообщить о проблеме»: сколько журнала уйдёт вместе с отчётом.
    ReportInfo,
    /// «Отправить»: что не работает и приложить ли журнал (его читает Rust, не интерфейс),
    /// и выбранные файлы — их места и имена (сами файлы уже пришли `report_file`).
    Report {
        text: String,
        log: bool,
        #[serde(default)]
        files: Vec<ReportAttachment>,
    },
    /// «Понятно» под ответом разработчика: эти ответы прочитаны.
    AnswersSeen {
        ids: Vec<String>,
    },
    /// Кусок выбранного файла: `slot` — место файла в форме, `offset` — сколько его
    /// байт уже пришло, `data` — следующие байты в base64.
    ReportFile {
        slot: usize,
        offset: u64,
        data: String,
    },
    /// Файл убрали из формы.
    ReportFileDrop {
        slot: usize,
    },
    /// Включить или выключить плагин.
    PluginToggle {
        id: String,
        on: bool,
    },
    /// Новые значения настроек плагина (только изменённые или все).
    PluginSettings {
        id: String,
        values: Map<String, Value>,
    },
    /// Нажали кнопку плагина на панели.
    PluginAction {
        id: String,
    },
    /// «Обновить»: перечитать папку плагинов.
    PluginsReload,
    /// «Папка плагинов»: открыть её в проводнике.
    PluginsFolder,
    /// Удалить плагин (папку `folder` в папке плагинов) вместе с его данными.
    PluginRemove {
        folder: String,
    },
    /// Открыли «Магазин»: загрузить каталог.
    MarketOpen,
    /// «Установить» или «Обновить» плагин из магазина.
    MarketInstall {
        id: String,
    },
}

/// Файл к «Сообщить о проблеме».
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReportAttachment {
    pub slot: usize,
    pub name: String,
}

/// Что нажали в окошке разрешения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermAnswer {
    /// «Разрешить»: и запомнить.
    Allow,
    /// «Только сейчас»: до ухода со страницы.
    Once,
    /// «Запретить»: и запомнить.
    Block,
    /// Закрыли окошко: в этот раз нет, но спросить снова.
    Dismiss,
}

/// Разбирает и проверяет сообщение интерфейса.
pub fn parse(raw: &str) -> Result<UiCmd, IpcError> {
    if raw.len() > MAX_MESSAGE {
        return Err(IpcError::TooLarge(raw.len()));
    }
    let cmd: UiCmd = serde_json::from_str(raw)?;
    validate(&cmd)?;
    Ok(cmd)
}

fn validate(cmd: &UiCmd) -> Result<(), IpcError> {
    let short = |s: &str, max: usize, what: &'static str| {
        if s.len() <= max {
            Ok(())
        } else {
            Err(IpcError::Invalid(what))
        }
    };
    match cmd {
        UiCmd::Layout { content, ai } => {
            content.validate()?;
            if let Some(ai) = ai {
                ai.validate()?;
            }
        }
        UiCmd::IslandDock { dock: Some(dock) } => dock.validate()?,
        UiCmd::Navigate { input } => short(input, MAX_INPUT, "input")?,
        UiCmd::NewTab { input: Some(input) } => short(input, MAX_INPUT, "input")?,
        UiCmd::Ai { query: Some(q), .. } => short(q, MAX_INPUT, "query")?,
        UiCmd::Suggest { q } | UiCmd::Find { q, .. } => short(q, 2048, "query")?,
        UiCmd::HideSite { url, .. } => short(url, 2048, "site")?,
        UiCmd::History { q, offset, limit } => {
            short(q, 2048, "query")?;
            if !(1..=MAX_HISTORY_PAGE).contains(limit) || *offset > 1_000_000 {
                return Err(IpcError::Invalid("history"));
            }
        }
        UiCmd::HistoryOpen { url, .. } | UiCmd::HistoryDelete { url } => {
            short(url, MAX_INPUT, "history_url")?;
        }
        UiCmd::Hotkey { action } => short(action, 32, "hotkey")?,
        UiCmd::ThemeColors {
            window,
            page,
            scheme,
        } => {
            if parse_hex(window).is_none() || parse_hex(page).is_none() {
                return Err(IpcError::Invalid("color"));
            }
            if !matches!(scheme.as_str(), "" | "dark" | "light" | "auto") {
                return Err(IpcError::Invalid("scheme"));
            }
        }
        UiCmd::SaveSettings { settings } => {
            let size = serde_json::to_string(&settings.appearance)?.len();
            if size > MAX_APPEARANCE {
                return Err(IpcError::Invalid("appearance"));
            }
            if let Some(proxy) = &settings.proxy {
                short(proxy, 512, "proxy")?;
            }
            short(
                &settings.search_custom,
                crate::tabs::omnibox::MAX_TEMPLATE,
                "search",
            )?;
            // С запасом на пробелы по краям: точную проверку делают настройки.
            if let Some(ua) = &settings.user_agent_custom {
                short(ua, 2 * crate::tabs::identity::MAX_AGENT, "user_agent")?;
            }
            if let Some(referer) = &settings.referer_custom {
                short(referer, 2 * crate::tabs::identity::MAX_REFERER, "referer")?;
            }
        }
        UiCmd::TranslatePrefs { prefs } if !prefs.is_valid() => {
            return Err(IpcError::Invalid("translate"));
        }
        UiCmd::SetVolume { volume, .. } if *volume > crate::tabs::volume::FULL => {
            return Err(IpcError::Invalid("volume"));
        }
        UiCmd::ZoomBubble { rect, radius } => {
            if let Some(rect) = rect {
                rect.validate()?;
            }
            if !(0.0..=64.0).contains(radius) {
                return Err(IpcError::Invalid("radius"));
            }
        }
        UiCmd::SitePermission {
            site,
            kind,
            decision,
        } => {
            use crate::storage::permissions::{Decision, Perm, valid_site};
            if !valid_site(site) || !kind.remembered() || *kind == Perm::Unknown {
                return Err(IpcError::Invalid("permission"));
            }
            if *decision == Some(Decision::Unknown) {
                return Err(IpcError::Invalid("decision"));
            }
        }
        UiCmd::AdblockAllow { site } | UiCmd::AdblockForget { site } => {
            short(site, 2048, "site")?;
        }
        UiCmd::ForgetSite { site } | UiCmd::ForgetAppLink { site, .. } => {
            short(site, 300, "site")?;
            if let UiCmd::ForgetAppLink { scheme, .. } = cmd {
                short(scheme, 64, "scheme")?;
            }
        }
        UiCmd::PermissionDefault { kind, .. } if !kind.remembered() => {
            return Err(IpcError::Invalid("permission"));
        }
        UiCmd::Report { text, files, .. } => {
            short(text, crate::reports::MAX_USER_TEXT, "report")?;
            if text.trim().is_empty() {
                return Err(IpcError::Invalid("report_empty"));
            }
            if files.len() > crate::reports::MAX_FILES {
                return Err(IpcError::Invalid("report_files"));
            }
            let mut slots = std::collections::HashSet::new();
            for file in files {
                short(&file.name, 1024, "report_file")?;
                if file.slot >= crate::reports::SLOTS || !slots.insert(file.slot) {
                    return Err(IpcError::Invalid("report_file"));
                }
            }
        }
        UiCmd::ReportFile { slot, offset, data } => {
            // base64 от куска в MAX_CHUNK байт.
            short(
                data,
                crate::reports::MAX_CHUNK.div_ceil(3) * 4,
                "report_file",
            )?;
            if *slot >= crate::reports::SLOTS || *offset > 1 << 32 {
                return Err(IpcError::Invalid("report_file"));
            }
        }
        UiCmd::ReportFileDrop { slot } if *slot >= crate::reports::SLOTS => {
            return Err(IpcError::Invalid("report_file"));
        }
        UiCmd::PluginToggle { id, .. } | UiCmd::PluginAction { id } => short(id, 64, "plugin")?,
        UiCmd::PluginSettings { id, values } => {
            short(id, 64, "plugin")?;
            if values.len() > 64 {
                return Err(IpcError::Invalid("plugin"));
            }
        }
        UiCmd::PluginRemove { folder } => short(folder, 255, "plugin")?,
        UiCmd::MarketInstall { id } => short(id, 64, "plugin")?,
        UiCmd::AnswersSeen { ids } => {
            if ids.len() > 32 {
                return Err(IpcError::Invalid("ids"));
            }
            for id in ids {
                short(id, 64, "ids")?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// `#rrggbb` → RGBA.
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8, u8)> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8, 255))
}

// ---------- Rust → интерфейс ----------

#[derive(Debug, Serialize)]
pub struct TabView<'a> {
    pub id: u64,
    pub title: &'a str,
    pub url: &'a str,
    pub loading: bool,
    pub sleeping: bool,
    /// Страница просыпается: вместо неё видна заглушка.
    pub waking: bool,
    pub ai: bool,
    pub bookmarked: bool,
    /// «Не усыплять» у самой вкладки (кнопка в адресной строке).
    pub no_sleep: bool,
    /// Сайт вкладки в списке «не усыплять»: она не засыпает, даже без своего флага.
    pub site_no_sleep: bool,
    /// Страница играет звук (значок динамика на вкладке).
    pub audible: bool,
    /// Звук вкладки выключен (перечёркнутый динамик).
    pub muted: bool,
    /// Громкость вкладки, 0–100.
    pub volume: u8,
    /// Масштаб страницы в процентах (100 — без масштаба).
    pub zoom: u16,
    /// Заблокировано рекламы на странице.
    pub blocked: u32,
    /// Реклама на сайте вкладки блокируется (не выключена вовсе и не для этого сайта).
    pub ads_blocked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Site {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct Suggestion {
    pub title: String,
    pub url: String,
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
pub struct DownloadView {
    pub id: u64,
    pub name: String,
    pub url: String,
    pub received: u64,
    pub total: Option<u64>,
    pub status: crate::downloads::Status,
    pub error: String,
    pub at: u64,
    /// Файл ещё на месте (его могли удалить или перенести).
    pub exists: bool,
    /// Файл опасного типа ждёт решения: сохранить или удалить.
    pub ask: bool,
    /// Ждёт решения программа под видом обычного файла (она уже скачана).
    pub disguised: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToUi<'a> {
    State {
        tabs: Vec<TabView<'a>>,
        active: Option<u64>,
        ai_open: bool,
        can_restore: bool,
        /// Плитки новой вкладки; `None`, пока активна обычная страница.
        top_sites: Option<Vec<Site>>,
    },
    Settings {
        settings: &'a crate::settings::Settings,
    },
    /// Список сайтов «не усыплять» поменял Rust (меню вкладки или чистка вписанного).
    NoSleepSites {
        sites: &'a [String],
    },
    Suggestions {
        q: &'a str,
        items: Vec<Suggestion>,
        /// Прошлые запросы из истории, начинающиеся с набранного.
        searches: Vec<String>,
    },
    /// Подсказки поисковой системы на набранный `q` (могут прийти позже `Suggestions`).
    Completions {
        q: &'a str,
        items: &'a [String],
    },
    Hotkey {
        action: String,
    },
    Find {
        found: bool,
    },
    Toast {
        text: &'a str,
    },
    Window {
        maximized: bool,
    },
    System {
        status: crate::system::Status,
    },
    Imported {
        ok: bool,
        browser: crate::storage::import::Browser,
        name: &'static str,
        profile: &'a str,
        bookmarks: usize,
        history: usize,
        error: &'a str,
    },
    Installed {
        ok: bool,
        error: &'a str,
    },
    Downloads {
        items: Vec<DownloadView>,
        can_cancel: bool,
    },
    DownloadProgress {
        id: u64,
        received: u64,
        total: Option<u64>,
    },
    /// Что играет во вкладке (для островка); `None` — страница не ответила.
    Media {
        id: u64,
        info: Option<crate::tabs::media::Info>,
    },
    /// Видео вкладки показано из чёлки (`on`) или уже убрано; `height` — высота острова
    /// с видео (логические px), под неё интерфейс освобождает место.
    Peek {
        id: u64,
        on: bool,
        height: f64,
    },
    Update(crate::update::View),
    /// Блокировка рекламы: настройки, списки, счётчик за всё время.
    Adblock(crate::adblock::View<'a>),
    /// Заблокировано на странице вкладки (и всего).
    Blocked {
        id: u64,
        count: u32,
        total: u64,
    },
    /// Прокси вкладок: кнопка на панели и раздел «Сеть».
    Proxy(crate::net::View<'a>),
    /// Список изменений для «Что нового»; `error` — не удалось скачать.
    UpdateNotes {
        which: crate::update::NotesWhich,
        from: Option<String>,
        to: String,
        /// Страница релиза `to` — «На GitHub».
        page: String,
        sections: Vec<crate::update::NotesSection>,
        error: String,
    },
    Translate(crate::translate::View<'a>),
    /// Снимок страницы под окном (или «убрать снимок»).
    Snapshot(crate::tabs::snapshot::View),
    /// Активную вкладку настроек закрыли: убрать лист с анимацией и ответить `settings_left`.
    SettingsLeave,
    /// Масштаб активной вкладки поменяли: показать окошко масштаба.
    Zoom {
        id: u64,
        /// В процентах.
        zoom: u16,
    },
    /// Сайт просит открыть приложение; `id = 0` — закрыть окошко.
    AppLink {
        id: u64,
        /// Сайт для показа (`example.com`), пустой — неизвестен.
        site: &'a str,
        /// Название программы, пустое — не нашли.
        app: &'a str,
        scheme: &'a str,
        link: &'a str,
        /// Показать «Всегда разрешать для этого сайта».
        remember: bool,
    },
    /// Сайт просит разрешения; `id = 0` — закрыть окошко.
    PermissionAsk {
        id: u64,
        /// Кто просит, для показа (`meet.google.com`); пустой — страница без адреса.
        site: &'a str,
        /// Сайт вкладки, если просит встроенный в неё чужой фрейм.
        page: &'a str,
        kinds: Vec<crate::storage::permissions::Perm>,
        /// Решение запомнится (у страницы есть свой адрес).
        remember: bool,
    },
    /// Разрешения сайтов и «Всегда разрешать» ссылок — для Настроек → Безопасность
    /// и кнопки сайта в адресной строке.
    Permissions {
        sites: &'a [crate::storage::permissions::Site],
        defaults: &'a std::collections::BTreeMap<
            crate::storage::permissions::Perm,
            crate::storage::permissions::Decision,
        >,
        app_links: &'a [crate::storage::app_links::Entry],
    },
    /// Сколько байт журнала уйдёт с «Сообщить о проблеме».
    ReportInfo {
        log: usize,
    },
    /// «Сообщить о проблеме» отправлен (`done`: форму можно очистить) или нет.
    Reported {
        done: bool,
    },
    /// Свои сообщения разработчику и ответы на них, новые сверху.
    Answers {
        items: Vec<crate::reports::answers::View>,
        /// Пришёл ответ, которого ещё не видели (не при запуске): сказать об этом.
        fresh: bool,
    },
    /// Ответ на кусок файла: сколько байт файла уже на диске или почему он не принят.
    ReportFile {
        slot: usize,
        size: u64,
        error: Option<&'a str>,
    },
    /// Записи для страницы истории: ответ на `history` с теми же `q` и `offset`.
    History {
        q: &'a str,
        offset: usize,
        items: Vec<&'a crate::storage::history::Entry>,
        /// Сколько записей подходит под запрос и есть ли они дальше.
        found: usize,
        more: bool,
        /// Сколько адресов во всей истории.
        total: usize,
    },
    /// Ответ на `history_count`.
    HistoryCount {
        since: u64,
        count: usize,
    },
    /// Замер для диспетчера задач: ответ на `tasks`.
    Tasks(crate::taskman::View),
    /// Плагины для «Настройки → Плагины» и их кнопки на панели.
    Plugins(crate::plugins::List<'a>),
    /// Магазин плагинов.
    Market(crate::plugins::market::View<'a>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_commands() {
        assert!(matches!(
            parse(r#"{"cmd":"plugin_toggle","id":"hello","on":true}"#),
            Ok(UiCmd::PluginToggle { on: true, .. })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"plugin_settings","id":"hello","values":{"speed":300}}"#),
            Ok(UiCmd::PluginSettings { .. })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"plugins_reload"}"#),
            Ok(UiCmd::PluginsReload)
        ));
        assert!(matches!(
            parse(r#"{"cmd":"plugins_folder"}"#),
            Ok(UiCmd::PluginsFolder)
        ));
        assert!(matches!(
            parse(r#"{"cmd":"plugin_remove","folder":"hello"}"#),
            Ok(UiCmd::PluginRemove { .. })
        ));
        let long = format!(r#"{{"cmd":"plugin_action","id":"{}"}}"#, "a".repeat(65));
        assert!(parse(&long).is_err());
        let many: Vec<String> = (0..65).map(|i| format!(r#""k{i}":1"#)).collect();
        let too_many = format!(
            r#"{{"cmd":"plugin_settings","id":"x","values":{{{}}}}}"#,
            many.join(",")
        );
        assert!(parse(&too_many).is_err());
    }

    #[test]
    fn parses_import() {
        let cmd = parse(r#"{"cmd":"import","browser":"opera_gx"}"#);
        assert!(matches!(
            cmd,
            Ok(UiCmd::Import {
                browser: crate::storage::import::Browser::OperaGx
            })
        ));
        assert!(parse(r#"{"cmd":"import","browser":"netscape"}"#).is_err());
        assert!(parse(r#"{"cmd":"import_chrome"}"#).is_err());
    }

    #[test]
    fn parses_navigate() {
        let cmd = parse(r#"{"cmd":"navigate","input":"rust"}"#);
        assert!(matches!(cmd, Ok(UiCmd::Navigate { input }) if input == "rust"));
    }

    #[test]
    fn rejects_unknown_and_bad_values() {
        assert!(parse(r#"{"cmd":"rm_rf"}"#).is_err());
        // Путь к файлу интерфейс передать не может — только номер загрузки.
        assert!(parse(r#"{"cmd":"download_open","path":"C:\Windows"}"#).is_err());
        assert!(matches!(
            parse(r#"{"cmd":"download_open","id":3}"#),
            Ok(UiCmd::DownloadOpen { id: 3 })
        ));
        // Откуда качать обновление, решает Rust: у команды нет полей.
        assert!(matches!(
            parse(r#"{"cmd":"update_install"}"#),
            Ok(UiCmd::UpdateInstall)
        ));
        assert!(matches!(
            parse(r#"{"cmd":"update_notes","which":"next"}"#),
            Ok(UiCmd::UpdateNotes {
                which: crate::update::NotesWhich::Next
            })
        ));
        assert!(parse(r#"{"cmd":"update_notes","which":"https://example.com"}"#).is_err());
        assert!(
            parse(r#"{"cmd":"layout","content":{"x":0,"y":0,"w":-5,"h":10},"ai":null}"#).is_err()
        );
        assert!(parse(r##"{"cmd":"theme_colors","window":"red","page":"#000000"}"##).is_err());
        let huge = format!(
            r#"{{"cmd":"navigate","input":"{}"}}"#,
            "a".repeat(MAX_INPUT + 1)
        );
        assert!(parse(&huge).is_err());
    }

    #[test]
    fn settings_tab_messages() {
        assert!(matches!(
            parse(r#"{"cmd":"open_settings"}"#),
            Ok(UiCmd::OpenSettings)
        ));
        assert!(matches!(
            parse(r#"{"cmd":"settings_left"}"#),
            Ok(UiCmd::SettingsLeft)
        ));
        let out = serde_json::to_string(&ToUi::SettingsLeave).unwrap_or_default();
        assert_eq!(out, r#"{"type":"settings_leave"}"#);
    }

    #[test]
    fn parses_task_manager_commands() {
        assert!(matches!(
            parse(r#"{"cmd":"open_tasks"}"#),
            Ok(UiCmd::OpenTasks)
        ));
        assert!(matches!(parse(r#"{"cmd":"tasks"}"#), Ok(UiCmd::Tasks)));
        assert!(matches!(
            parse(r#"{"cmd":"tasks_sleep","id":4}"#),
            Ok(UiCmd::TasksSleep { id: 4 })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"tasks_sleep_all"}"#),
            Ok(UiCmd::TasksSleepAll)
        ));
        assert!(parse(r#"{"cmd":"tasks_sleep"}"#).is_err());
        assert!(parse(r#"{"cmd":"tasks_sleep","id":-1}"#).is_err());
    }

    #[test]
    fn task_manager_view_serializes() {
        use crate::taskman::{Part, PartUse, TabUse, Usage, View};
        let usage = Usage {
            memory: 5 << 20,
            cpu: 1.5,
            procs: 2,
        };
        let view = View {
            per_tab: true,
            tabs: vec![TabUse {
                id: 3,
                usage,
                shared: false,
            }],
            parts: vec![PartUse {
                part: Part::Gpu,
                usage,
            }],
            total: usage,
            cpu_ready: true,
            ram_total: 16 << 30,
            ram_free: 8 << 30,
            cpus: 8,
        };
        let json = serde_json::to_string(&ToUi::Tasks(view)).unwrap_or_default();
        assert!(json.starts_with(r#"{"type":"tasks","per_tab":true,"tabs":[{"id":3,"memory":5242880,"cpu":1.5,"procs":2,"shared":false}],"parts":[{"part":"gpu","memory""#), "{json}");
        assert!(json.contains(r#""cpus":8"#));
    }

    #[test]
    fn parses_history_page_commands() {
        assert!(matches!(
            parse(r#"{"cmd":"open_history"}"#),
            Ok(UiCmd::OpenHistory)
        ));
        assert!(matches!(
            parse(r#"{"cmd":"history","q":"wry","offset":200,"limit":200}"#),
            Ok(UiCmd::History { q, offset: 200, limit: 200 }) if q == "wry"
        ));
        // Без запроса и сдвига — первая страница всей истории.
        assert!(matches!(
            parse(r#"{"cmd":"history","limit":50}"#),
            Ok(UiCmd::History {
                offset: 0,
                limit: 50,
                ..
            })
        ));
        // Всю историю разом не отдаём: страница — от 1 до MAX_HISTORY_PAGE записей.
        assert!(parse(r#"{"cmd":"history","limit":0}"#).is_err());
        assert!(parse(r#"{"cmd":"history","limit":5000}"#).is_err());
        assert!(parse(r#"{"cmd":"history","offset":-1,"limit":10}"#).is_err());
        assert!(parse(r#"{"cmd":"history"}"#).is_err());
        let long = format!(
            r#"{{"cmd":"history","q":"{}","limit":10}}"#,
            "a".repeat(3000)
        );
        assert!(parse(&long).is_err());

        assert!(matches!(
            parse(r#"{"cmd":"history_open","url":"https://a.example/","background":true}"#),
            Ok(UiCmd::HistoryOpen { url, background: true }) if url == "https://a.example/"
        ));
        assert!(matches!(
            parse(r#"{"cmd":"history_open","url":"https://a.example/"}"#),
            Ok(UiCmd::HistoryOpen {
                background: false,
                ..
            })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"history_delete","url":"https://a.example/"}"#),
            Ok(UiCmd::HistoryDelete { url }) if url == "https://a.example/"
        ));
        assert!(parse(r#"{"cmd":"history_delete"}"#).is_err());
        let huge = format!(
            r#"{{"cmd":"history_delete","url":"https://a.example/{}"}}"#,
            "a".repeat(MAX_INPUT)
        );
        assert!(parse(&huge).is_err());

        assert!(matches!(
            parse(r#"{"cmd":"history_clear","since":0}"#),
            Ok(UiCmd::HistoryClear { since: 0 })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"history_count","since":1764100000}"#),
            Ok(UiCmd::HistoryCount {
                since: 1_764_100_000
            })
        ));
        // Срок — только число секунд: ни строки, ни дроби, ни «всё» без поля.
        assert!(parse(r#"{"cmd":"history_clear","since":"all"}"#).is_err());
        assert!(parse(r#"{"cmd":"history_clear","since":1.5}"#).is_err());
        assert!(parse(r#"{"cmd":"history_clear"}"#).is_err());
        assert!(matches!(
            parse(r#"{"cmd":"hotkey","action":"history"}"#),
            Ok(UiCmd::Hotkey { action }) if crate::ui::hotkeys::Hotkey::parse(&action).is_some()
        ));

        let entry = crate::storage::history::Entry {
            url: "https://a.example/".into(),
            title: "A".into(),
            visits: 2,
            last: 100,
        };
        let out = serde_json::to_string(&ToUi::History {
            q: "a",
            offset: 0,
            items: vec![&entry],
            found: 1,
            more: false,
            total: 3,
        })
        .unwrap_or_default();
        assert_eq!(
            out,
            r#"{"type":"history","q":"a","offset":0,"items":[{"url":"https://a.example/","title":"A","visits":2,"last":100}],"found":1,"more":false,"total":3}"#
        );
        let out =
            serde_json::to_string(&ToUi::HistoryCount { since: 5, count: 2 }).unwrap_or_default();
        assert_eq!(out, r#"{"type":"history_count","since":5,"count":2}"#);
    }

    #[test]
    fn parses_hide_site() {
        assert!(matches!(
            parse(r#"{"cmd":"hide_site","url":"https://github.com","hidden":true}"#),
            Ok(UiCmd::HideSite { url, hidden: true }) if url == "https://github.com"
        ));
        assert!(parse(r#"{"cmd":"hide_site","url":"https://github.com"}"#).is_err());
        let long = format!(
            r#"{{"cmd":"hide_site","url":"{}","hidden":true}}"#,
            "a".repeat(3000)
        );
        assert!(parse(&long).is_err());
    }

    #[test]
    fn parses_toggle_no_sleep() {
        assert!(matches!(
            parse(r#"{"cmd":"toggle_no_sleep","id":7}"#),
            Ok(UiCmd::ToggleNoSleep { id: 7 })
        ));
        assert!(parse(r#"{"cmd":"toggle_no_sleep"}"#).is_err());
        assert!(matches!(
            parse(r#"{"cmd":"toggle_site_no_sleep","id":7}"#),
            Ok(UiCmd::ToggleSiteNoSleep { id: 7 })
        ));
        assert!(parse(r#"{"cmd":"toggle_site_no_sleep"}"#).is_err());
    }

    #[test]
    fn no_sleep_sites_setting_is_optional() {
        let sites = |extra: &str| match parse(&format!(
            r#"{{"cmd":"save_settings","settings":{{"appearance":{{}},"double_search":true,"proxy":"","sleep_minutes":5,"live_tabs":3{extra}}}}}"#
        )) {
            Ok(UiCmd::SaveSettings { settings }) => Some(settings.no_sleep_sites),
            _ => None,
        };
        assert_eq!(sites(""), Some(None));
        assert_eq!(
            sites(r#","no_sleep_sites":["youtube.com"]"#),
            Some(Some(vec!["youtube.com".to_string()]))
        );
        // Не список — сохраняется всё остальное, а сайты не трогаются.
        assert_eq!(sites(r#","no_sleep_sites":"youtube.com""#), Some(None));
    }

    #[test]
    fn parses_media() {
        use crate::tabs::media::Action;
        assert!(matches!(
            parse(r#"{"cmd":"media","id":3}"#),
            Ok(UiCmd::Media {
                id: 3,
                action: None
            })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"media","id":3,"action":"next"}"#),
            Ok(UiCmd::Media {
                action: Some(Action::Next),
                ..
            })
        ));
        // Только известные кнопки.
        assert!(parse(r#"{"cmd":"media","id":3,"action":"eval"}"#).is_err());
        assert!(parse(r#"{"cmd":"media"}"#).is_err());
    }

    #[test]
    fn parses_island_dock() {
        assert!(matches!(
            parse(r#"{"cmd":"island_dock","dock":{"cx":960,"top":30,"width":400}}"#),
            Ok(UiCmd::IslandDock { dock: Some(_) })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"island_dock"}"#),
            Ok(UiCmd::IslandDock { dock: None })
        ));
        // Ширина видео в разумных пределах, координаты — числа.
        assert!(parse(r#"{"cmd":"island_dock","dock":{"cx":960,"top":30,"width":5}}"#).is_err());
        assert!(parse(r#"{"cmd":"island_dock","dock":{"cx":-5,"top":30,"width":400}}"#).is_err());
        // Карточка в боковой панели: верх чёлки не у края окна, но не ниже острова.
        assert!(matches!(
            parse(
                r#"{"cmd":"island_dock","dock":{"cx":120,"top":120,"width":228,"notch_top":86}}"#
            ),
            Ok(UiCmd::IslandDock { dock: Some(_) })
        ));
        assert!(
            parse(r#"{"cmd":"island_dock","dock":{"cx":120,"top":80,"width":228,"notch_top":86}}"#)
                .is_err()
        );
        // Ширина чёлки без видео и скорость анимаций; без них — как раньше (скорость 1).
        match parse(
            r#"{"cmd":"island_dock","dock":{"cx":960,"top":22,"width":400,"from":240,"speed":0}}"#,
        ) {
            Ok(UiCmd::IslandDock { dock: Some(d) }) => assert_eq!((d.from, d.speed), (240.0, 0.0)),
            other => panic!("{other:?}"),
        }
        match parse(r#"{"cmd":"island_dock","dock":{"cx":960,"top":22,"width":400}}"#) {
            Ok(UiCmd::IslandDock { dock: Some(d) }) => assert_eq!((d.from, d.speed), (0.0, 1.0)),
            other => panic!("{other:?}"),
        }
        assert!(
            parse(r#"{"cmd":"island_dock","dock":{"cx":960,"top":22,"width":400,"speed":9}}"#)
                .is_err()
        );
        assert!(
            parse(r#"{"cmd":"island_dock","dock":{"cx":960,"top":22,"width":400,"from":-1}}"#)
                .is_err()
        );
    }

    #[test]
    fn parses_set_volume() {
        assert!(matches!(
            parse(r#"{"cmd":"set_volume","id":4,"volume":35}"#),
            Ok(UiCmd::SetVolume { id: 4, volume: 35 })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"set_volume","id":4,"volume":0}"#),
            Ok(UiCmd::SetVolume { volume: 0, .. })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"set_volume","id":4,"volume":100}"#),
            Ok(UiCmd::SetVolume { volume: 100, .. })
        ));
        // Громче 100 % не бывает; дробь, минус, строка и пропуск — ошибка.
        assert!(parse(r#"{"cmd":"set_volume","id":4,"volume":101}"#).is_err());
        assert!(parse(r#"{"cmd":"set_volume","id":4,"volume":300}"#).is_err());
        assert!(parse(r#"{"cmd":"set_volume","id":4,"volume":50.5}"#).is_err());
        assert!(parse(r#"{"cmd":"set_volume","id":4,"volume":-5}"#).is_err());
        assert!(parse(r#"{"cmd":"set_volume","id":4,"volume":"50"}"#).is_err());
        assert!(parse(r#"{"cmd":"set_volume","id":4}"#).is_err());
        assert!(parse(r#"{"cmd":"set_volume","volume":50}"#).is_err());
    }

    #[test]
    fn tab_menu_commands_take_only_an_id() {
        assert!(matches!(
            parse(r#"{"cmd":"reload_tab","id":1}"#),
            Ok(UiCmd::ReloadTab { id: 1 })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"duplicate_tab","id":2}"#),
            Ok(UiCmd::DuplicateTab { id: 2 })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"toggle_mute","id":3}"#),
            Ok(UiCmd::ToggleMute { id: 3 })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"close_other_tabs","id":4}"#),
            Ok(UiCmd::CloseOtherTabs { id: 4 })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"close_tabs_right","id":5}"#),
            Ok(UiCmd::CloseTabsRight { id: 5 })
        ));
        // Без номера, с чужим номером вместо числа или с адресом — отказ.
        assert!(parse(r#"{"cmd":"toggle_mute"}"#).is_err());
        assert!(parse(r#"{"cmd":"close_tabs_right","id":-1}"#).is_err());
        assert!(parse(r#"{"cmd":"duplicate_tab","id":"7"}"#).is_err());
        assert!(parse(r#"{"cmd":"reload_tab","url":"view-source:https://a/"}"#).is_err());
        // Действия страницы из интерфейса — только известные имена сочетаний.
        assert!(matches!(
            parse(r#"{"cmd":"hotkey","action":"view_source"}"#),
            Ok(UiCmd::Hotkey { action }) if crate::ui::hotkeys::Hotkey::parse(&action).is_some()
        ));
    }

    #[test]
    fn translate_prefs_are_checked() {
        let ok =
            r#"{"cmd":"translate_prefs","prefs":{"to":"de","always":["en","fr"],"offer":false}}"#;
        assert!(matches!(parse(ok), Ok(UiCmd::TranslatePrefs { prefs }) if prefs.to == "de"));
        // Язык перевода — только из списка, коды — без мусора.
        let bad_to = r#"{"cmd":"translate_prefs","prefs":{"to":"xx","always":[],"offer":true}}"#;
        assert!(parse(bad_to).is_err());
        let bad_code =
            r#"{"cmd":"translate_prefs","prefs":{"to":"ru","always":["en\"><img>"],"offer":true}}"#;
        assert!(parse(bad_code).is_err());
        // Что и куда переводить, интерфейс не передаёт: только «переведи активную».
        assert!(matches!(
            parse(r#"{"cmd":"translate"}"#),
            Ok(UiCmd::Translate)
        ));
    }

    #[test]
    fn parses_search_settings() {
        use crate::tabs::omnibox::Engine;
        let save = |extra: &str| {
            parse(&format!(
                r#"{{"cmd":"save_settings","settings":{{"appearance":{{}},"double_search":true,"proxy":"","sleep_minutes":5,"live_tabs":3{extra}}}}}"#
            ))
        };
        let engine = |extra: &str| match save(extra) {
            Ok(UiCmd::SaveSettings { settings }) => Some(settings.search_engine),
            _ => None,
        };
        assert_eq!(engine(r#","search_engine":"bing""#), Some(Engine::Bing));
        // Старый интерфейс без поля и незнакомая система — DuckDuckGo.
        assert_eq!(engine(""), Some(Engine::DuckDuckGo));
        assert_eq!(
            engine(r#","search_engine":"altavista""#),
            Some(Engine::DuckDuckGo)
        );
        let long = format!(
            r#","search_engine":"custom","search_custom":"https://a.b/?q=%s{}""#,
            "x".repeat(crate::tabs::omnibox::MAX_TEMPLATE)
        );
        assert!(save(&long).is_err());
    }

    #[test]
    fn parses_zoom_commands() {
        use crate::tabs::zoom::Zoom;
        assert!(matches!(
            parse(r#"{"cmd":"zoom","step":"in"}"#),
            Ok(UiCmd::Zoom { step: Zoom::In })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"zoom","step":"reset"}"#),
            Ok(UiCmd::Zoom { step: Zoom::Reset })
        ));
        // Масштаб задаёт только Rust: число вместо ступени — отказ.
        assert!(parse(r#"{"cmd":"zoom","step":"5"}"#).is_err());
        assert!(parse(r#"{"cmd":"zoom","level":5}"#).is_err());
        let bubble =
            parse(r#"{"cmd":"zoom_bubble","rect":{"x":600,"y":44,"w":220,"h":46},"radius":12}"#);
        assert!(
            matches!(bubble, Ok(UiCmd::ZoomBubble { rect: Some(r), radius }) if r.w == 220.0 && radius == 12.0)
        );
        assert!(matches!(
            parse(r#"{"cmd":"zoom_bubble","rect":null}"#),
            Ok(UiCmd::ZoomBubble { rect: None, .. })
        ));
        assert!(
            parse(r#"{"cmd":"zoom_bubble","rect":{"x":0,"y":0,"w":-1,"h":4},"radius":12}"#)
                .is_err()
        );
        assert!(parse(r#"{"cmd":"zoom_bubble","rect":null,"radius":1000}"#).is_err());
        let out = serde_json::to_string(&ToUi::Zoom { id: 3, zoom: 125 }).unwrap_or_default();
        assert_eq!(out, r#"{"type":"zoom","id":3,"zoom":125}"#);
    }

    #[test]
    fn parses_permission_commands() {
        use crate::storage::permissions::{Decision, Perm};
        assert!(matches!(
            parse(r#"{"cmd":"permission","id":7,"answer":"once"}"#),
            Ok(UiCmd::Permission {
                id: 7,
                answer: PermAnswer::Once
            })
        ));
        assert!(parse(r#"{"cmd":"permission","id":7,"answer":"maybe"}"#).is_err());
        assert!(matches!(
            parse(r#"{"cmd":"site_permission","site":"https://meet.example.com","kind":"camera","decision":"block"}"#),
            Ok(UiCmd::SitePermission {
                site,
                kind: Perm::Camera,
                decision: Some(Decision::Block)
            }) if site == "https://meet.example.com"
        ));
        assert!(matches!(
            parse(
                r#"{"cmd":"site_permission","site":"https://a.example","kind":"camera","decision":null}"#
            ),
            Ok(UiCmd::SitePermission { decision: None, .. })
        ));
        // Не origin, показ экрана, неизвестный вид или решение — отказ.
        for bad in [
            r#"{"cmd":"site_permission","site":"javascript:1","kind":"camera","decision":"allow"}"#,
            r#"{"cmd":"site_permission","site":"https://a.example/x","kind":"camera","decision":"allow"}"#,
            r#"{"cmd":"site_permission","site":"https://a.example","kind":"screen","decision":"allow"}"#,
            r#"{"cmd":"site_permission","site":"https://a.example","kind":"teleport","decision":"allow"}"#,
            r#"{"cmd":"site_permission","site":"https://a.example","kind":"camera","decision":"maybe"}"#,
            r#"{"cmd":"permission_default","kind":"screen","block":true}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(matches!(
            parse(r#"{"cmd":"permission_default","kind":"notifications","block":true}"#),
            Ok(UiCmd::PermissionDefault {
                kind: Perm::Notifications,
                block: true
            })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"forget_app_link","site":"https://a.example","scheme":"tg"}"#),
            Ok(UiCmd::ForgetAppLink { .. })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"forget_site","site":"https://a.example"}"#),
            Ok(UiCmd::ForgetSite { .. })
        ));
    }

    #[test]
    fn parses_app_link_answer() {
        assert!(matches!(
            parse(r#"{"cmd":"app_link","id":4,"open":true,"always":true}"#),
            Ok(UiCmd::AppLink {
                id: 4,
                open: true,
                always: true
            })
        ));
        assert!(matches!(
            parse(r#"{"cmd":"app_link","id":4,"open":false}"#),
            Ok(UiCmd::AppLink { always: false, .. })
        ));
        assert!(parse(r#"{"cmd":"app_link","open":true}"#).is_err());
    }

    #[test]
    fn parses_bug_report() {
        assert!(matches!(
            parse(r#"{"cmd":"report","text":"вкладка не грузится","log":true}"#),
            Ok(UiCmd::Report { text, log: true, files }) if text == "вкладка не грузится" && files.is_empty()
        ));
        assert!(matches!(
            parse(r#"{"cmd":"report_info"}"#),
            Ok(UiCmd::ReportInfo)
        ));
        // Журнал читает Rust: интерфейс передаёт только «приложить или нет».
        assert!(parse(r#"{"cmd":"report","text":"x"}"#).is_err());
        assert!(parse(r#"{"cmd":"report","text":"x","log":"C:\\axiom.log"}"#).is_err());
        let empty = parse(r#"{"cmd":"report","text":"  \n ","log":false}"#);
        assert!(matches!(&empty, Err(e) if e.user_text() == Some("Опишите, что не работает")));
        let long = format!(
            r#"{{"cmd":"report","text":"{}","log":false}}"#,
            "a".repeat(crate::reports::MAX_USER_TEXT + 1)
        );
        assert!(matches!(parse(&long), Err(e) if e.user_text() == Some("Слишком длинный текст")));
        let out = serde_json::to_string(&ToUi::Reported { done: true }).unwrap_or_default();
        assert_eq!(out, r#"{"type":"reported","done":true}"#);
    }

    #[test]
    fn parses_report_files() {
        assert!(matches!(
            parse(r#"{"cmd":"report","text":"x","log":false,"files":[{"slot":3,"name":"снимок.png"},{"slot":0,"name":"a.mp4"}]}"#),
            Ok(UiCmd::Report { files, .. }) if files == [
                ReportAttachment { slot: 3, name: "снимок.png".into() },
                ReportAttachment { slot: 0, name: "a.mp4".into() },
            ]
        ));
        // Одно место дважды, место за пределами и путь вместо имени не проходят.
        assert!(parse(r#"{"cmd":"report","text":"x","log":false,"files":[{"slot":1,"name":"a"},{"slot":1,"name":"b"}]}"#).is_err());
        assert!(
            parse(r#"{"cmd":"report","text":"x","log":false,"files":[{"slot":64,"name":"a"}]}"#)
                .is_err()
        );
        assert!(
            parse(r#"{"cmd":"report","text":"x","log":false,"files":[{"slot":1,"path":"C:\\a"}]}"#)
                .is_err()
        );
        let many: Vec<String> = (0..=crate::reports::MAX_FILES)
            .map(|i| format!(r#"{{"slot":{i},"name":"f"}}"#))
            .collect();
        let many = format!(
            r#"{{"cmd":"report","text":"x","log":false,"files":[{}]}}"#,
            many.join(",")
        );
        assert!(parse(&many).is_err());

        assert!(matches!(
            parse(r#"{"cmd":"report_file","slot":2,"offset":0,"data":"iVBORw0KGgo="}"#),
            Ok(UiCmd::ReportFile { slot: 2, offset: 0, data }) if data == "iVBORw0KGgo="
        ));
        // Полный кусок в base64 влезает и в проверку, и в предел сообщения.
        let full = format!(
            r#"{{"cmd":"report_file","slot":63,"offset":1048576,"data":"{}"}}"#,
            "A".repeat(crate::reports::MAX_CHUNK.div_ceil(3) * 4)
        );
        assert!(parse(&full).is_ok());
        let over = format!(
            r#"{{"cmd":"report_file","slot":1,"offset":0,"data":"{}"}}"#,
            "A".repeat(crate::reports::MAX_CHUNK.div_ceil(3) * 4 + 4)
        );
        assert!(parse(&over).is_err());
        assert!(parse(r#"{"cmd":"report_file","slot":64,"offset":0,"data":""}"#).is_err());
        assert!(matches!(
            parse(r#"{"cmd":"report_file_drop","slot":5}"#),
            Ok(UiCmd::ReportFileDrop { slot: 5 })
        ));
        assert!(parse(r#"{"cmd":"report_file_drop","slot":99}"#).is_err());
        let out = serde_json::to_string(&ToUi::ReportFile {
            slot: 2,
            size: 10,
            error: None,
        })
        .unwrap_or_default();
        assert_eq!(
            out,
            r#"{"type":"report_file","slot":2,"size":10,"error":null}"#
        );
    }

    #[test]
    fn crash_reports_setting_is_optional() {
        let save = |extra: &str| match parse(&format!(
            r#"{{"cmd":"save_settings","settings":{{"appearance":{{}},"double_search":true,"proxy":"","sleep_minutes":5,"live_tabs":3{extra}}}}}"#
        )) {
            Ok(UiCmd::SaveSettings { settings }) => Some(settings.crash_reports),
            _ => None,
        };
        assert_eq!(save(""), Some(None));
        assert_eq!(save(r#","crash_reports":false"#), Some(Some(false)));
        let mut s = crate::settings::Settings::default();
        assert!(s.crash_reports, "по умолчанию включено");
        if let Ok(UiCmd::SaveSettings { settings }) = parse(
            r#"{"cmd":"save_settings","settings":{"appearance":{},"double_search":true,"proxy":"","sleep_minutes":5,"live_tabs":3,"crash_reports":false}}"#,
        ) {
            s.apply(settings);
        }
        assert!(!s.crash_reports);
    }

    #[test]
    fn usage_stats_setting_is_optional() {
        let parse_in = |extra: &str| match parse(&format!(
            r#"{{"cmd":"save_settings","settings":{{"appearance":{{}},"double_search":true,"proxy":"","sleep_minutes":5,"live_tabs":3{extra}}}}}"#
        )) {
            Ok(UiCmd::SaveSettings { settings }) => Some(settings),
            _ => None,
        };
        let mut s = crate::settings::Settings::default();
        assert!(s.usage_stats, "по умолчанию включено");
        if let Some(settings) = parse_in(r#","usage_stats":false"#) {
            s.apply(settings);
        }
        assert!(!s.usage_stats);
        // Интерфейс без переключателя поле не шлёт — выключенное так и остаётся.
        if let Some(settings) = parse_in("") {
            s.apply(settings);
        }
        assert!(!s.usage_stats);
        if let Some(settings) = parse_in(r#","usage_stats":true"#) {
            s.apply(settings);
        }
        assert!(s.usage_stats);
    }

    #[test]
    fn new_ai_chat_setting_is_optional() {
        let save = |extra: &str| match parse(&format!(
            r#"{{"cmd":"save_settings","settings":{{"appearance":{{}},"double_search":true,"proxy":"","sleep_minutes":5,"live_tabs":3{extra}}}}}"#
        )) {
            Ok(UiCmd::SaveSettings { settings }) => Some(settings.ai_new_chat),
            _ => None,
        };
        assert_eq!(save(""), Some(None));
        assert_eq!(save(r#","ai_new_chat":false"#), Some(Some(false)));
        assert_eq!(save(r#","ai_new_chat":null"#), Some(None));
        assert_eq!(save(r#","ai_new_chat":"нет""#), None);
    }

    #[test]
    fn proxy_button_and_address() {
        assert!(matches!(
            parse(r#"{"cmd":"proxy","on":true}"#),
            Ok(UiCmd::Proxy { on: true })
        ));
        assert!(parse(r#"{"cmd":"proxy"}"#).is_err());
        let save = |proxy: &str| {
            parse(&format!(
                r#"{{"cmd":"save_settings","settings":{{"appearance":{{}},"double_search":true,"sleep_minutes":5,"live_tabs":3{proxy}}}}}"#
            ))
        };
        // Интерфейс без поля адреса — не менять; прежнее поле ai_proxy просто не читается.
        assert!(
            matches!(save(""), Ok(UiCmd::SaveSettings { settings }) if settings.proxy.is_none())
        );
        assert!(matches!(
            save(r#","ai_proxy":"socks5://a:1""#),
            Ok(UiCmd::SaveSettings { settings }) if settings.proxy.is_none()
        ));
        let long = format!(r#","proxy":"socks5://{}:1""#, "a".repeat(600));
        assert!(matches!(save(&long), Err(IpcError::Invalid("proxy"))));
    }

    #[test]
    fn parses_hex() {
        assert_eq!(parse_hex("#1a2B3c"), Some((0x1a, 0x2b, 0x3c, 255)));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("123456"), None);
    }
}
