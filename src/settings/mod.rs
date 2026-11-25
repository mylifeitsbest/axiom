//! Настройки браузера. Внешний вид хранится как непрозрачный JSON:
//! его схему знает только интерфейс, Rust лишь ограничивает размер.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    ipc::SettingsIn,
    storage,
    tabs::omnibox::{self, Engine, Search},
};

pub const FILE: &str = "settings.json";
const MAX_SLEEP_MINUTES: u32 = 24 * 60;
const MAX_LIVE_TABS: u32 = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemeColors {
    pub window: String,
    pub page: String,
    /// Что сайты видят в `prefers-color-scheme`: dark, light или auto (как в системе).
    pub scheme: String,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            window: "#141418".into(),
            page: "#19191e".into(),
            scheme: "dark".into(),
        }
    }
}

impl ThemeColors {
    pub fn theme(&self) -> wry::Theme {
        match self.scheme.as_str() {
            "light" => wry::Theme::Light,
            "auto" => wry::Theme::Auto,
            _ => wry::Theme::Dark,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Тема, раскладка, шрифты, свой CSS — всё, что настраивает интерфейс.
    pub appearance: Map<String, Value>,
    /// Поисковая система адресной строки.
    #[serde(deserialize_with = "omnibox::lenient_engine")]
    pub search_engine: Engine,
    /// Свой адрес поиска с `%s` на месте запроса (для `custom`). Хранится,
    /// даже когда выбрана другая система, чтобы к нему можно было вернуться.
    pub search_custom: String,
    /// Запрос из адресной строки уходит и в поисковую систему, и в AI Mode.
    pub double_search: bool,
    /// Прокси только для панели ИИ: `http://host:port` или `socks5://host:port`.
    pub ai_proxy: String,
    /// Через сколько минут неактивности усыплять вкладку (0 — никогда).
    pub sleep_minutes: u32,
    /// Сколько фоновых вкладок держать загруженными (0 — без ограничения).
    /// Лишние, самые давние, засыпают сразу.
    pub live_tabs: u32,
    /// Цвета темы, чтобы новые webview не мигали белым.
    pub colors: ThemeColors,
    /// Пройдено ли приветствие первого запуска.
    pub onboarded: bool,
    /// Принудительно затемнять сайты без своей тёмной темы (WebView2, после перезапуска).
    pub force_dark: bool,
    /// Узнавать у GitHub о новых версиях (раз в несколько часов).
    pub check_updates: bool,
    /// Следить и за бета-версиями (каждый коммит в main, до выпуска версии).
    /// `None` — ещё не решали: бета-версия при первом запуске включает их сама,
    /// и копия остаётся в этом канале, даже если потом поставят обычный выпуск.
    pub beta: Option<bool>,
    /// Перевод страниц: язык, автоперевод, предлагать ли перевод.
    pub translate: crate::translate::Prefs,
    /// Убирать метки слежки (`utm_*`, `fbclid`, `gclid`…) из открываемых адресов.
    pub strip_tracking: bool,
    /// Отправлять разработчику отчёт, если Axiom вылетит (см. `reports`).
    pub crash_reports: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Map::new(),
            search_engine: Engine::DuckDuckGo,
            search_custom: String::new(),
            double_search: true,
            ai_proxy: String::new(),
            sleep_minutes: 5,
            live_tabs: 3,
            colors: ThemeColors::default(),
            onboarded: false,
            force_dark: false,
            check_updates: true,
            beta: None,
            translate: crate::translate::Prefs::default(),
            strip_tracking: true,
            crash_reports: true,
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
        self.search_engine = incoming.search_engine;
        self.search_custom = incoming.search_custom.trim().to_string();
        self.fix_search();
        self.double_search = incoming.double_search;
        self.ai_proxy = incoming.ai_proxy.trim().to_string();
        self.sleep_minutes = incoming.sleep_minutes.min(MAX_SLEEP_MINUTES);
        self.live_tabs = incoming.live_tabs.min(MAX_LIVE_TABS);
        self.onboarded |= incoming.onboarded;
        self.force_dark = incoming.force_dark;
        self.check_updates = incoming.check_updates;
        // Интерфейс без этого переключателя поле не шлёт — тогда не трогаем.
        if let Some(on) = incoming.strip_tracking {
            self.strip_tracking = on;
        }
        if let Some(on) = incoming.crash_reports {
            self.crash_reports = on;
        }
        // Переключатель есть не у всех: у кого его нет, приходит null.
        if let Some(on) = incoming.beta {
            self.beta = Some(on);
        }
    }

    /// Следить ли за бета-версиями. Первый запуск бета-версии
    /// включает их сам; выключенные вручную так и остаются выключенными.
    pub fn beta_channel(&mut self, beta_build: bool) -> bool {
        if self.beta.is_none() && beta_build {
            self.beta = Some(true);
        }
        self.beta == Some(true)
    }

    /// Куда уходит поиск из адресной строки.
    pub fn search(&self) -> Search<'_> {
        Search {
            engine: self.search_engine,
            custom: &self.search_custom,
        }
    }

    /// Поисковый запрос, если `url` — страница результатов любой системы или своего адреса.
    pub fn query(&self, url: &str) -> Option<String> {
        omnibox::search_query(url, &self.search_custom)
    }

    /// Свой адрес — только http(s) и с `%s`; без годного адреса — DuckDuckGo.
    fn fix_search(&mut self) {
        if !omnibox::valid_template(&self.search_custom) {
            self.search_custom.clear();
        }
        if self.search_engine == Engine::Custom && self.search_custom.is_empty() {
            self.search_engine = Engine::DuckDuckGo;
        }
    }

    fn sanitized(mut self) -> Self {
        self.fix_search();
        self.sleep_minutes = self.sleep_minutes.min(MAX_SLEEP_MINUTES);
        self.live_tabs = self.live_tabs.min(MAX_LIVE_TABS);
        self.translate = self.translate.sanitized();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming(engine: &str, custom: &str) -> serde_json::Result<SettingsIn> {
        serde_json::from_value(serde_json::json!({
            "appearance": {},
            "double_search": true,
            "ai_proxy": "",
            "sleep_minutes": 5,
            "live_tabs": 3,
            "search_engine": engine,
            "search_custom": custom,
        }))
    }

    #[test]
    fn applies_search_engine() -> serde_json::Result<()> {
        let mut s = Settings::default();
        s.apply(incoming("yandex", "")?);
        assert_eq!(s.search_engine, Engine::Yandex);

        s.apply(incoming("custom", " https://example.com/?q=%s ")?);
        assert_eq!(s.search_engine, Engine::Custom);
        assert_eq!(s.search().custom, "https://example.com/?q=%s");
        assert_eq!(
            s.query("https://example.com/?q=rust").as_deref(),
            Some("rust")
        );

        // Свой адрес без %s или не http(s) — откат на DuckDuckGo.
        for bad in ["https://example.com/", "javascript:alert(%s)", ""] {
            s.apply(incoming("custom", bad)?);
            assert_eq!(s.search_engine, Engine::DuckDuckGo, "{bad}");
            assert!(s.search_custom.is_empty());
        }
        Ok(())
    }

    #[test]
    fn unknown_engine_keeps_other_settings() -> serde_json::Result<()> {
        let file =
            r#"{"search_engine":"altavista","search_custom":"ftp://x/%s","sleep_minutes":30}"#;
        let s = serde_json::from_str::<Settings>(file)?.sanitized();
        assert_eq!(s.search_engine, Engine::DuckDuckGo);
        assert!(s.search_custom.is_empty());
        assert_eq!(s.sleep_minutes, 30);
        // Старый файл без поисковой системы.
        let old = serde_json::from_str::<Settings>(r#"{"double_search":false}"#)?;
        assert_eq!(old.search_engine, Engine::DuckDuckGo);
        Ok(())
    }
}
