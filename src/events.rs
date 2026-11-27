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
    /// Запрос скрипта плагина из вкладки (`plugins::bridge`).
    Plugin(crate::plugins::Message),
    /// Кнопку плагина нажали, а на странице активной вкладки его нет.
    PluginMissed {
        tab: TabId,
        plugin: String,
    },
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
    /// Панель ИИ не пустила к себе страницу не из AI Mode (переход отменён):
    /// если по ней кликнули, она откроется во вкладке (см. `ai_panel::pages`).
    AiLeft(String),
    /// Страница ответила, как открывать её ссылку: в фоне (средний клик, Ctrl+клик) или впереди.
    PageTab {
        url: String,
        ai: bool,
        from: Option<TabId>,
        background: bool,
    },
    /// Масштаб страницы: WebView2 поменял его сам или Ctrl+колёсико на Linux.
    TabZoom(TabId, crate::tabs::zoom::Event),
    /// Страница вкладки начала или перестала играть звук.
    TabAudio {
        id: TabId,
        playing: bool,
    },
    /// Страница ответила, что играет (`tabs::media::QUERY`), — для островка.
    TabMedia {
        id: TabId,
        raw: String,
    },
    /// Страница вывела видео поверх себя для островка; `raw` — его размер (`tabs::mini`).
    MiniReady {
        id: TabId,
        raw: String,
    },
    /// Кадр выезда (`open`) или уезда острова с видео.
    MiniFrame {
        id: TabId,
        step: u8,
        open: bool,
    },
    /// Мышь ушла с чёлки и острова (или показ остановили).
    MiniEnded(TabId),
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
    /// Импорт из другого браузера завершился (выполняется в фоновом потоке).
    Imported(
        crate::storage::import::Browser,
        Result<crate::storage::import::Imported, String>,
    ),
    /// Перевод страницы: ответ страницы, перевод от Google или таймер.
    Translate(crate::translate::Event),
    /// Окно так и не получило `ready` от интерфейса — показываем его принудительно.
    ShowFallback,
    /// Снимок страницы под окном интерфейса готов или не успел.
    Snapshot(crate::tabs::snapshot::Event),
    /// Запасной таймер: интерфейс так и не сообщил, что лист закрытых настроек ушёл.
    SettingsLeft,
    /// Просыпающаяся вкладка: пора спросить страницу, нарисовалась ли она, или ответ.
    Waking(TabId, crate::tabs::waking::Event),
    /// Ссылка на приложение (`mailto:`, `tg:`…): запрос страницы или итог запуска.
    AppLink(crate::tabs::external::Event),
    /// Сайт просит микрофон, камеру, место, уведомления…: решить или спросить.
    Permission(crate::tabs::permissions::Request),
    /// Панель ИИ ушла на другую страницу: её запросы разрешений устарели.
    PermissionPageLeft(crate::tabs::TabId),
    /// Решения, которые WebView2 запомнил сам (до того, как Axiom стал отвечать).
    #[cfg_attr(not(windows), allow(dead_code))]
    PermissionsSaved(Vec<crate::tabs::permissions::Saved>),
    /// Переход по ссылке с метками слежки отменён: загрузить чистый адрес.
    /// Только WebView2: на Linux переходы внутри вкладки не переписываются.
    #[cfg_attr(not(windows), allow(dead_code))]
    Untracked {
        id: TabId,
        url: String,
    },
    /// «Сообщить о проблеме»: отчёт отправлен, отложен или не принят (фоновый поток).
    Reported(crate::reports::Outcome),
    /// Поисковая система ответила подсказками на набранный текст (`suggest`).
    Completions(crate::suggest::Answer),
    /// Свои сообщения разработчику изменились: новое, пришёл ответ, ответ прочитан.
    Answers,
    /// Linux: процесс попросили завершиться (SIGTERM при выключении компьютера, Ctrl+C
    /// в терминале) — закрыться как по крестику, сохранив вкладки.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Quit,
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
