//! Какую панель ИИ видно на какой вкладке.
//!
//! Двойной поиск открывает панель для своей вкладки, и вкладка запоминает запрос.
//! На вкладке со своим запросом открытая панель всегда показывает его ответ, ничего
//! не перезагружая, если он уже на экране. Панель двойного поиска на новой вкладке
//! или на обычном сайте прячется. Панель, которую человек открыл сам (Ctrl+K,
//! кнопка, «Спросить ИИ»), на вкладках без запроса остаётся как была.

/// Кто открыл панель.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Панель закрыта: переключение вкладок её не открывает.
    #[default]
    Closed,
    /// Открыл человек: панель общая и закрывается только им самим.
    Manual,
    /// Открыл двойной поиск: панель видна на вкладках со своим запросом.
    Search,
}

/// Что сделать с панелью.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Keep,
    /// Спрятать до возврата на вкладку с запросом.
    Hide,
    /// Показать; `Some` — сначала загрузить этот запрос.
    Show(Option<String>),
    /// Закрыть: переключение вкладок её больше не откроет.
    Close,
}

/// Стала активной вкладка со своим запросом `query` (`None` — новая вкладка,
/// обычный сайт). `shown` — запрос, который сейчас загружен в панели.
pub fn on_activate(mode: Mode, query: Option<&str>, shown: Option<&str>) -> Step {
    match (mode, query) {
        (Mode::Closed, _) => Step::Keep,
        // У вкладки свой поиск — в панели его ответ, кто бы панель ни открыл.
        (_, Some(q)) => Step::Show(load(q, shown)),
        (Mode::Search, None) => Step::Hide,
        (Mode::Manual, None) => Step::Keep,
    }
}

/// Закрыли вкладку с запросом `query`: если панель показывала именно его, она
/// закрывается вместе с вкладкой, а не переходит к соседней.
pub fn on_close(mode: Mode, query: Option<&str>, shown: Option<&str>) -> Step {
    match (mode, query) {
        (Mode::Closed, _) => Step::Keep,
        (_, Some(q)) if shown == Some(q) => Step::Close,
        _ => Step::Keep,
    }
}

/// Двойной поиск во вкладке с новым запросом `query`. `active` — вкладка уже
/// на экране (новую вкладку панель покажет, когда та откроется).
/// Панель, открытая человеком, остаётся общей, но запрос в неё всё равно уходит.
pub fn on_search(mode: Mode, query: &str, shown: Option<&str>, active: bool) -> (Mode, Step) {
    match mode {
        Mode::Manual => (Mode::Manual, Step::Show(load(query, shown))),
        _ if active => (Mode::Search, Step::Show(load(query, shown))),
        _ => (Mode::Search, Step::Keep),
    }
}

/// Человек открыл панель сам. `open` — панель уже на экране.
pub fn on_open(mode: Mode, open: bool, query: Option<&str>, shown: Option<&str>) -> (Mode, Step) {
    // Интерфейс вслед за двойным поиском присылает тот же запрос: он уже в панели.
    if open && query.is_some() && query == shown {
        return (mode, Step::Keep);
    }
    (Mode::Manual, Step::Show(query.and_then(|q| load(q, shown))))
}

/// Запрос грузим, только если панель показывает другой.
fn load(query: &str, shown: Option<&str>) -> Option<String> {
    (shown != Some(query)).then(|| query.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(q: &str) -> Step {
        Step::Show(Some(q.into()))
    }

    #[test]
    fn search_panel_follows_tabs() {
        // Новая вкладка или обычный сайт: панель прячется.
        assert_eq!(on_activate(Mode::Search, None, Some("a")), Step::Hide);
        // Вернулись на вкладку с тем же запросом: показываем без перезагрузки.
        assert_eq!(
            on_activate(Mode::Search, Some("a"), Some("a")),
            Step::Show(None)
        );
        // У вкладки свой запрос: он и загружается.
        assert_eq!(on_activate(Mode::Search, Some("b"), Some("a")), show("b"));
        // Панель уснула и потеряла ответ: загружаем заново.
        assert_eq!(on_activate(Mode::Search, Some("a"), None), show("a"));
    }

    #[test]
    fn closed_panel_ignores_tabs() {
        for query in [None, Some("a"), Some("b")] {
            assert_eq!(on_activate(Mode::Closed, query, Some("a")), Step::Keep);
            assert_eq!(on_close(Mode::Closed, query, Some("a")), Step::Keep);
        }
    }

    #[test]
    fn manual_panel_still_shows_the_tab_search() {
        // Видео 25.11: панель открыли кнопкой, и вкладки с поиском её больше не меняли.
        assert_eq!(on_activate(Mode::Manual, Some("b"), Some("a")), show("b"));
        assert_eq!(
            on_activate(Mode::Manual, Some("a"), Some("a")),
            Step::Show(None)
        );
        // Без своего поиска ручная панель остаётся как была.
        assert_eq!(on_activate(Mode::Manual, None, Some("a")), Step::Keep);
        // Закрыли вкладку, чей ответ в панели, — панель закрывается и здесь.
        assert_eq!(on_close(Mode::Manual, Some("a"), Some("a")), Step::Close);
        assert_eq!(on_close(Mode::Manual, Some("b"), Some("a")), Step::Keep);
        assert_eq!(on_close(Mode::Manual, None, Some("a")), Step::Keep);
    }

    #[test]
    fn closing_search_tab_closes_its_panel() {
        assert_eq!(on_close(Mode::Search, Some("a"), Some("a")), Step::Close);
        // Закрыли другую вкладку или вкладку без запроса — панель не трогаем.
        assert_eq!(on_close(Mode::Search, Some("b"), Some("a")), Step::Keep);
        assert_eq!(on_close(Mode::Search, None, Some("a")), Step::Keep);
        // После закрытия соседняя вкладка с поиском панель не открывает.
        assert_eq!(on_activate(Mode::Closed, Some("b"), Some("a")), Step::Keep);
    }

    #[test]
    fn search_opens_bound_panel() {
        assert_eq!(
            on_search(Mode::Closed, "a", None, true),
            (Mode::Search, show("a"))
        );
        assert_eq!(
            on_search(Mode::Search, "b", Some("a"), true),
            (Mode::Search, show("b"))
        );
        // Новая вкладка ещё не на экране: панель покажет её открытие.
        assert_eq!(
            on_search(Mode::Closed, "a", None, false),
            (Mode::Search, Step::Keep)
        );
        assert_eq!(
            on_activate(Mode::Search, Some("a"), None),
            show("a"),
            "открытие новой вкладки показывает её запрос"
        );
    }

    #[test]
    fn search_keeps_manual_panel_shared() {
        assert_eq!(
            on_search(Mode::Manual, "a", Some("x"), false),
            (Mode::Manual, show("a"))
        );
        assert_eq!(
            on_search(Mode::Manual, "a", Some("a"), true),
            (Mode::Manual, Step::Show(None))
        );
    }

    #[test]
    fn user_open_is_manual() {
        // Ctrl+K на вкладке без запроса — общая панель с прежним содержимым.
        assert_eq!(
            on_open(Mode::Search, false, None, Some("a")),
            (Mode::Manual, Step::Show(None))
        );
        assert_eq!(
            on_open(Mode::Closed, false, Some("b"), Some("a")),
            (Mode::Manual, show("b"))
        );
        // Тот же запрос в закрытой панели: показываем, не спрашивая заново.
        assert_eq!(
            on_open(Mode::Closed, false, Some("a"), Some("a")),
            (Mode::Manual, Step::Show(None))
        );
    }

    #[test]
    fn echo_of_double_search_keeps_mode() {
        assert_eq!(
            on_open(Mode::Search, true, Some("a"), Some("a")),
            (Mode::Search, Step::Keep)
        );
        // Другой запрос в открытой панели — это уже вопрос человека.
        assert_eq!(
            on_open(Mode::Search, true, Some("b"), Some("a")),
            (Mode::Manual, show("b"))
        );
    }
}
