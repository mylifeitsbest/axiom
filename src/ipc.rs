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
    pub double_search: bool,
    pub ai_proxy: String,
    pub sleep_minutes: u32,
    #[serde(default)]
    pub onboarded: bool,
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
    Suggest {
        q: String,
    },
    SaveSettings {
        settings: SettingsIn,
    },
    ThemeColors {
        window: String,
        page: String,
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
    ImportChrome,
    /// Установить в систему и зарегистрировать как браузер.
    Install,
    /// Открыть «Приложения по умолчанию» в параметрах Windows.
    OpenDefaultApps,
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
        UiCmd::Suggest { q } | UiCmd::Find { q, .. } => short(q, 512, "query")?,
        UiCmd::Hotkey { action } => short(action, 32, "hotkey")?,
        UiCmd::ThemeColors { window, page } => {
            if parse_hex(window).is_none() || parse_hex(page).is_none() {
                return Err(IpcError::Invalid("color"));
            }
        }
        UiCmd::SaveSettings { settings } => {
            let size = serde_json::to_string(&settings.appearance)?.len();
            if size > MAX_APPEARANCE {
                return Err(IpcError::Invalid("appearance"));
            }
            short(&settings.ai_proxy, 512, "proxy")?;
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
}

#[derive(Debug, Serialize)]
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
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToUi<'a> {
    State {
        tabs: Vec<TabView<'a>>,
        active: Option<u64>,
        ai_open: bool,
        ai_proxy: bool,
        can_restore: bool,
        top_sites: Vec<Site>,
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
        profile: &'a str,
        bookmarks: usize,
        history: usize,
        error: &'a str,
    },
    Installed {
        ok: bool,
        error: &'a str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_navigate() {
        let cmd = parse(r#"{"cmd":"navigate","input":"rust"}"#);
        assert!(matches!(cmd, Ok(UiCmd::Navigate { input }) if input == "rust"));
    }

    #[test]
    fn rejects_unknown_and_bad_values() {
        assert!(parse(r#"{"cmd":"rm_rf"}"#).is_err());
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
    fn parses_hex() {
        assert_eq!(parse_hex("#1a2B3c"), Some((0x1a, 0x2b, 0x3c, 255)));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("123456"), None);
    }
}
