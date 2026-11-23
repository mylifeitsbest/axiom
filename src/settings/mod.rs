//! Настройки браузера. Внешний вид хранится как непрозрачный JSON:
//! его схему знает только интерфейс, Rust лишь ограничивает размер.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{ipc::SettingsIn, storage};

pub const FILE: &str = "settings.json";
const MAX_SLEEP_MINUTES: u32 = 24 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemeColors {
    pub window: String,
    pub page: String,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            window: "#141418".into(),
            page: "#19191e".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Тема, раскладка, шрифты, свой CSS — всё, что настраивает интерфейс.
    pub appearance: Map<String, Value>,
    /// Запрос из адресной строки уходит и в DuckDuckGo, и в AI Mode.
    pub double_search: bool,
    /// Прокси только для панели ИИ: `http://host:port` или `socks5://host:port`.
    pub ai_proxy: String,
    /// Через сколько минут неактивности усыплять вкладку (0 — никогда).
    pub sleep_minutes: u32,
    /// Цвета темы, чтобы новые webview не мигали белым.
    pub colors: ThemeColors,
    /// Пройдено ли приветствие первого запуска.
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Map::new(),
            double_search: true,
            ai_proxy: String::new(),
            sleep_minutes: 10,
            colors: ThemeColors::default(),
            onboarded: false,
        }
    }
}

impl Settings {
    pub fn load(paths: &storage::Paths) -> Self {
        storage::read_json::<Settings>(&paths.root.join(FILE))
            .map(Settings::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &storage::Paths) -> anyhow::Result<()> {
        storage::write_json(&paths.root.join(FILE), self)
    }

    /// Применяет присланные интерфейсом значения. Прокси проверяется отдельно.
    pub fn apply(&mut self, incoming: SettingsIn) {
        self.appearance = incoming.appearance;
        self.double_search = incoming.double_search;
        self.ai_proxy = incoming.ai_proxy.trim().to_string();
        self.sleep_minutes = incoming.sleep_minutes.min(MAX_SLEEP_MINUTES);
        self.onboarded |= incoming.onboarded;
    }

    fn sanitized(mut self) -> Self {
        self.sleep_minutes = self.sleep_minutes.min(MAX_SLEEP_MINUTES);
        if crate::ai_panel::proxy::parse(&self.ai_proxy).is_err() {
            self.ai_proxy.clear();
        }
        if crate::ipc::parse_hex(&self.colors.window).is_none()
            || crate::ipc::parse_hex(&self.colors.page).is_none()
        {
            self.colors = ThemeColors::default();
        }
        self
    }
}
