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
            Self::Invalid("input" | "query") => Some("Слишком длинный текст"),
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
    pub ai_proxy: String,
    pub sleep_minutes: u32,
    pub live_tabs: u32,
    #[serde(default)]
    pub onboarded: bool,
    #[serde(default)]
    pub force_dark: bool,
    #[serde(default = "yes")]
    pub check_updates: bool,
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
    /// Лист закрытых настроек плавно ушёл: можно показывать страницу под ним.
    SettingsLeft,
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
    /// Крестик на плитке новой вкладки (`hidden: true`) и «Вернуть» (`false`).
    HideSite {
        url: String,
        hidden: bool,
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
    /// Кнопка перевода в адресной строке: перевести активную вкладку.
    Translate,
    /// «Показать оригинал» в окошке перевода.
    TranslateOriginal,
    /// Язык перевода, «Всегда переводить» и «Предлагать перевод».
    TranslatePrefs {
        prefs: crate::translate::Prefs,
    },
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
        UiCmd::Navigate { input } => short(input, MAX_INPUT, "input")?,
        UiCmd::NewTab { input: Some(input) } => short(input, MAX_INPUT, "input")?,
        UiCmd::Ai { query: Some(q), .. } => short(q, MAX_INPUT, "query")?,
        UiCmd::Suggest { q } | UiCmd::Find { q, .. } => short(q, 2048, "query")?,
        UiCmd::HideSite { url, .. } => short(url, 2048, "site")?,
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
            short(&settings.ai_proxy, 512, "proxy")?;
            short(
                &settings.search_custom,
                crate::tabs::omnibox::MAX_TEMPLATE,
                "search",
            )?;
        }
        UiCmd::TranslatePrefs { prefs } if !prefs.is_valid() => {
            return Err(IpcError::Invalid("translate"));
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
    pub ai: bool,
    pub bookmarked: bool,
    pub no_sleep: bool,
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
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToUi<'a> {
    State {
        tabs: Vec<TabView<'a>>,
        active: Option<u64>,
        ai_open: bool,
        ai_proxy: bool,
        can_restore: bool,
        /// Плитки новой вкладки; `None`, пока активна обычная страница.
        top_sites: Option<Vec<Site>>,
    },
    Settings {
        settings: &'a crate::settings::Settings,
    },
    Suggestions {
        q: &'a str,
        items: Vec<Suggestion>,
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
    Update(crate::update::View),
    Translate(crate::translate::View<'a>),
    /// Снимок страницы под окном (или «убрать снимок»).
    Snapshot(crate::tabs::snapshot::View),
    /// Активную вкладку настроек закрыли: убрать лист с анимацией и ответить `settings_left`.
    SettingsLeave,
}

#[cfg(test)]
mod tests {
    use super::*;

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
                r#"{{"cmd":"save_settings","settings":{{"appearance":{{}},"double_search":true,"ai_proxy":"","sleep_minutes":5,"live_tabs":3{extra}}}}}"#
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
    fn parses_hex() {
        assert_eq!(parse_hex("#1a2B3c"), Some((0x1a, 0x2b, 0x3c, 255)));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("123456"), None);
    }
}
