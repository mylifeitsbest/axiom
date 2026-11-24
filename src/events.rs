//! События, которые приходят в главный цикл из колбэков webview и фоновых потоков.

use std::{thread, time::Duration};

use tao::event_loop::EventLoopProxy;

use crate::{
    downloads::DownloadEvent,
    tabs::{TabId, fullscreen::Owner, sleep::PageState},
    ui::hotkeys::Hotkey,
};

/// Интервал служебного таймера: усыпление вкладок, сохранение сессии и истории.
pub const TICK: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum UserEvent {
    /// Сырой JSON от служебного webview интерфейса (источник уже проверен).
    Ui(String),
    /// Горячая клавиша, нажатая внутри страницы или панели ИИ.
    Hotkey(Hotkey),
    TabTitle(TabId, String),
    TabLoad {
        id: TabId,
        started: bool,
        url: String,
    },
    /// Сайт попросил открыть новое окно (`target=_blank`, `window.open`).
    OpenTab {
        url: String,
        ai: bool,
        /// Вкладка-источник; `None` — панель ИИ.
        from: Option<TabId>,
    },
    /// Страница ответила перед усыплением: позиция прокрутки и нет ли недописанного текста.
    SleepChecked(TabId, PageState),
    FindResult(bool),
    /// Страница развернула элемент на весь экран (видео) или свернула его.
    Fullscreen {
        owner: Owner,
        on: bool,
    },
    /// Загрузка файла началась, продвинулась или закончилась.
    Download(DownloadEvent),
    /// Адреса от второго запуска `axiom.exe <url>` (клик по ссылке в другой программе).
    External(Vec<String>),
    /// Проверка или скачивание обновления продвинулись (фоновый поток).
    Update(crate::update::UpdateEvent),
    /// Установка в систему завершилась (фоновый поток: PowerShell для ярлыка — секунды).
    Installed(Result<(), String>),
    /// Импорт из Chrome завершился (выполняется в фоновом потоке).
    Imported(Result<crate::storage::chrome::Imported, String>),
    /// Окно так и не получило `ready` от интерфейса — показываем его принудительно.
    ShowFallback,
    Tick,
}

pub fn spawn_ticker(proxy: EventLoopProxy<UserEvent>) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        if proxy.send_event(UserEvent::ShowFallback).is_err() {
            return;
        }
        loop {
            thread::sleep(TICK);
            if proxy.send_event(UserEvent::Tick).is_err() {
                return;
            }
        }
    });
}
