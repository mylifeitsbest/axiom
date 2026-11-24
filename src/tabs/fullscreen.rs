//! Полный экран: видео по кнопке сайта (Fullscreen API) и F11.
//!
//! Сам движок разворачивает элемент только на размер своего webview, а окно
//! остаётся прежним — видео просто заполняет область вкладки. Поэтому ловим
//! событие движка, а окно и раскладку меняет `App` (app.rs): окно без рамки на
//! весь монитор, webview страницы на всё окно, остальные webview спрятаны.
//!
//! WebView2: `ContainsFullScreenElementChanged`; Esc из полного экрана выводит сам
//! движок. Сайту он разрешает полный экран только при включённом `msWebOOUI`
//! (см. `webview::browser_args`). WebKitGTK: сигналы `enter-fullscreen` /
//! `leave-fullscreen`, окно он разворачивает сам, мы дополнительно растягиваем webview.

use tao::event_loop::EventLoopProxy;
use wry::WebView;

use super::TabId;
use crate::{events::UserEvent, ui::hotkeys::Hotkey};

/// Чья страница развернула элемент на весь экран.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Tab(TabId),
    /// Панель ИИ.
    Ai,
}

/// Что сейчас на весь экран.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct State {
    /// Страница развернула элемент (видео) сама.
    pub page: Option<Owner>,
    /// F11: страница на весь экран без интерфейса браузера.
    pub browser: bool,
}

impl State {
    pub fn is_on(self) -> bool {
        self.page.is_some() || self.browser
    }

    /// Выйти ли из полного экрана перед этим сочетанием клавиш.
    ///
    /// Видео сворачивается от любого сочетания (как в Chrome: Ctrl+T, Ctrl+Tab),
    /// кроме звука и масштаба. В режиме F11 вкладки переключаются как обычно, а выходим,
    /// только когда нужен сам интерфейс: адресная строка, панели, новая вкладка.
    pub fn leaves_on(self, hotkey: Hotkey) -> bool {
        match hotkey {
            _ if hotkey.keeps_fullscreen() => false,
            _ if self.page.is_some() => true,
            _ => self.browser && (hotkey.is_ui() || hotkey == Hotkey::NewTab),
        }
    }

    /// Esc выводит из режима F11. Из полного экрана самой страницы выводит движок:
    /// он знает, не перехватила ли страница Esc (игры, удалённый рабочий стол).
    pub fn leaves_on_escape(self) -> bool {
        self.browser && self.page.is_none()
    }
}

/// Скрипт выхода из полного экрана: наш выход (переключили вкладку, F11)
/// должен свернуть и сам элемент, иначе страница считала бы, что всё ещё на весь экран.
const EXIT: &str = "(()=>{const d=document;\
if(d.fullscreenElement)d.exitFullscreen().catch(()=>{});\
else if(d.webkitFullscreenElement)d.webkitExitFullscreen()})()";

pub fn exit_page(webview: &WebView) {
    let _ = webview.evaluate_script(EXIT);
}

/// Сообщает в главный цикл, когда страница входит в полный экран и выходит из него.
#[cfg(windows)]
pub fn install(webview: &WebView, owner: Owner, events: EventLoopProxy<UserEvent>) {
    use webview2_com::ContainsFullScreenElementChangedEventHandler;
    use windows_core::BOOL;
    use wry::WebViewExtWindows;

    let handler = ContainsFullScreenElementChangedEventHandler::create(Box::new(move |core, _| {
        let Some(core) = core else { return Ok(()) };
        let mut on = BOOL::default();
        // SAFETY: core — живое ядро webview, указатель на локальную переменную.
        unsafe { core.ContainsFullScreenElement(&mut on)? };
        let _ = events.send_event(UserEvent::Fullscreen {
            owner,
            on: on.as_bool(),
        });
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: ядро webview живёт столько же, сколько сам webview.
    let result = unsafe {
        webview
            .webview()
            .add_ContainsFullScreenElementChanged(&handler, &mut token)
    };
    if let Err(err) = result {
        eprintln!("полный экран для страниц недоступен: {err}");
    }
}

/// WebKitGTK: крейта webkit2gtk среди зависимостей нет, поэтому подключаемся
/// к сигналам по имени через GObject.
#[cfg(target_os = "linux")]
pub fn install(webview: &WebView, owner: Owner, events: EventLoopProxy<UserEvent>) {
    use gtk::glib::prelude::*;
    use wry::WebViewExtUnix;

    let view = webview.webview();
    for (signal, on) in [("enter-fullscreen", true), ("leave-fullscreen", false)] {
        let events = events.clone();
        view.connect_local(signal, false, move |_| {
            let _ = events.send_event(UserEvent::Fullscreen { owner, on });
            // false: дальше сработает обработчик WebKitGTK — он сам разворачивает окно.
            Some(false.to_value())
        });
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn install(_webview: &WebView, _owner: Owner, _events: EventLoopProxy<UserEvent>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_leaves_on_any_hotkey() {
        let video = State {
            page: Some(Owner::Tab(1)),
            browser: false,
        };
        assert!(video.leaves_on(Hotkey::NextTab));
        assert!(video.leaves_on(Hotkey::NewTab));
        assert!(video.leaves_on(Hotkey::FocusOmnibox));
        // F11 и Esc обрабатываются отдельно.
        assert!(!video.leaves_on(Hotkey::Fullscreen));
        assert!(!video.leaves_on(Hotkey::Escape));
        assert!(!video.leaves_on_escape());
        // Звук и масштаб меняют, не выходя из просмотра.
        assert!(!video.leaves_on(Hotkey::Mute));
        assert!(!video.leaves_on(Hotkey::ZoomOut));
    }

    #[test]
    fn f11_keeps_tab_switching() {
        let f11 = State {
            page: None,
            browser: true,
        };
        assert!(!f11.leaves_on(Hotkey::NextTab));
        assert!(!f11.leaves_on(Hotkey::Reload));
        assert!(f11.leaves_on(Hotkey::NewTab));
        assert!(f11.leaves_on(Hotkey::Settings));
        assert!(f11.leaves_on_escape());
        let off = State::default();
        assert!(!off.is_on());
        assert!(!off.leaves_on(Hotkey::NewTab));
        assert!(!off.leaves_on_escape());
    }
}
