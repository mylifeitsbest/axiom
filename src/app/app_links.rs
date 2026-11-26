//! Ссылки на приложения и чистые адреса: связывает `tabs::external`
//! и `tabs::tracking` с вкладками, окошком интерфейса и профилем.

use std::time::{Duration, Instant};

use url::Url;

use super::App;
use crate::{
    events::UserEvent,
    ipc::ToUi,
    storage::app_links::AppLinks,
    tabs::{
        TabId,
        external::{self, Event, Gesture, Request, Source},
        identity, script, tracking,
    },
};

/// Сколько молчим о «нет программы для ссылки», чтобы скрипт не завалил подсказками.
const NO_APP_QUIET: Duration = Duration::from_secs(5);
/// После «Отмена» на окошке, которое открылось без клика, вкладка какое-то время не спрашивает.
const MUTE_AFTER_CANCEL: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct State {
    links: Option<AppLinks>,
    pending: Option<Pending>,
    next_id: u64,
    no_app_at: Option<Instant>,
    muted: Option<(TabId, Instant)>,
}

/// Окошко «Открыть приложение?», которое ждёт ответа.
struct Pending {
    id: u64,
    tab: TabId,
    /// Ссылка для запуска — уже проверенная [`external::classify`].
    link: String,
    /// Origin сайта для «Всегда разрешать»; `None` — запомнить нельзя.
    site: Option<String>,
    /// Перед запросом на странице был настоящий клик.
    clicked: bool,
}

impl App {
    /// Включает или выключает чистку переходов по ссылкам по настройке.
    pub(super) fn sync_tracking(&self) {
        tracking::set_enabled(self.settings.strip_tracking);
    }

    /// Подмена User-Agent и Referer: новый выбор — сразу всем живым вкладкам.
    pub(super) fn sync_identity(&self) {
        if !identity::set(self.settings.identity()) {
            return;
        }
        for webview in self.tabs.iter().filter_map(|t| t.webview.as_ref()) {
            identity::apply(webview);
        }
        identity::apply_ui(&self.ui);
    }

    /// Адрес для вкладки без меток слежки (если это включено).
    pub(super) fn untracked(&self, url: String) -> String {
        if self.settings.strip_tracking {
            tracking::clean(&url).unwrap_or(url)
        } else {
            url
        }
    }

    /// Переход по ссылке с метками отменён во вкладке: грузим чистый адрес.
    pub(super) fn load_untracked(&mut self, id: TabId, url: String) {
        if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
            let _ = webview.load_url(&url);
        }
    }

    /// Ввод адресной строки — ссылка на приложение: открываем сразу и не ищем.
    /// `false` — это не ссылка на приложение.
    pub(super) fn open_typed_app(&mut self, input: &str) -> bool {
        match external::typed(input) {
            None => false,
            Some(Ok(link)) => {
                external::launch(link, self.events.clone());
                true
            }
            Some(Err(scheme)) => {
                let text = format!("Ссылки {scheme}: заблокированы");
                self.send(&ToUi::Toast { text: &text });
                true
            }
        }
    }

    pub(super) fn on_app_link(&mut self, event: Event) {
        match event {
            Event::Request(request) => self.check_app_link(request),
            Event::Checked { request, raw } => {
                self.decide_app_link(request, clicked_just_now(&raw));
            }
            Event::NoApp { tab, scheme } => {
                let quiet = self
                    .app_links
                    .no_app_at
                    .is_some_and(|at| at.elapsed() < NO_APP_QUIET);
                if self.tabs.active == Some(tab) && !quiet {
                    self.app_links.no_app_at = Some(Instant::now());
                    let text = format!("Нет программы для ссылок {scheme}:");
                    self.send(&ToUi::Toast { text: &text });
                }
            }
            Event::Failed { scheme } => {
                let text = format!("Не удалось открыть ссылку {scheme}:");
                self.send(&ToUi::Toast { text: &text });
            }
        }
    }

    /// Первый шаг: отсеиваем лишнее и спрашиваем страницу, был ли настоящий клик.
    fn check_app_link(&mut self, request: Request) {
        // Ещё раз здесь: запрос мог прийти не из обработчика навигации (фрейм, новое окно).
        if external::classify(&request.url, Source::Site).is_none() {
            eprintln!(
                "ссылка заблокирована: {}:",
                external::scheme_of(&request.url)
            );
            return;
        }
        if request.gesture == Gesture::No {
            // Без клика сайт приложение не открывает — и не спрашивает.
            eprintln!("ссылка без клика: {}:", external::scheme_of(&request.url));
            return;
        }
        // Спрашивает только вкладка, которую видно, и одно окошко за раз.
        if self.tabs.active != Some(request.tab) || self.app_links.pending.is_some() {
            return;
        }
        let Some(webview) = self.tabs.get(request.tab).and_then(|t| t.webview.as_ref()) else {
            return;
        };
        let events = self.events.clone();
        let asked = {
            let request = request.clone();
            script::ask(webview, external::INPUT_PROBE, move |raw| {
                let request = request.clone();
                let _ = events.send_event(UserEvent::AppLink(Event::Checked { request, raw }));
            })
        };
        if !asked {
            self.decide_app_link(request, false);
        }
    }

    /// Второй шаг: открыть сразу («Всегда разрешать» и настоящий клик) или спросить.
    /// Без клика окошко тоже возможно (клик во фрейме страница не видит), но без
    /// «Всегда разрешать» — и открывает программу всё равно только кнопка.
    fn decide_app_link(&mut self, request: Request, clicked: bool) {
        let Some(link) = external::classify(&request.url, Source::Site) else {
            return;
        };
        if self.tabs.active != Some(request.tab) || self.app_links.pending.is_some() {
            return;
        }
        let Some(tab_url) = self.tabs.get(request.tab).map(|t| t.url.clone()) else {
            return;
        };
        let muted = self
            .app_links
            .muted
            .is_some_and(|(t, at)| t == request.tab && at.elapsed() < MUTE_AFTER_CANCEL);
        if muted && !clicked {
            return;
        }
        let scheme = external::scheme_of(&link).to_string();
        let Some(handler) = external::handler(&scheme) else {
            let text = format!("Нет программы для ссылок {scheme}:");
            self.send(&ToUi::Toast { text: &text });
            return;
        };
        // Запомнить «Всегда разрешать» можно только для сайта, который назвал движок.
        // Адрес вкладки — не обязательно тот, кто просит: это мог быть чужой фрейм.
        let site = request.origin.as_deref().and_then(web_origin);
        // В окошке — кто просит, а если движок не сказал, то страница во вкладке.
        let shown = site.clone().or_else(|| web_origin(&tab_url));
        if clicked
            && let Some(site) = &site
            && self.link_perms().allows(site, &scheme)
        {
            external::launch(link, self.events.clone());
            return;
        }
        // Поверх видео на весь экран окошко было бы не видно.
        if self.fullscreen.is_on() {
            self.leave_fullscreen();
        }
        self.app_links.next_id += 1;
        let id = self.app_links.next_id;
        let shown_site = shown.as_deref().map(display_host).unwrap_or_default();
        let shown_link: String = link.chars().take(300).collect();
        let remember = clicked && site.is_some();
        self.send(&ToUi::AppLink {
            id,
            site: &shown_site,
            app: &handler.name,
            scheme: &scheme,
            link: &shown_link,
            remember,
        });
        // Чтобы Esc дошёл до окошка, а не до страницы.
        let _ = self.ui.focus();
        self.app_links.pending = Some(Pending {
            id,
            tab: request.tab,
            link,
            site: site.filter(|_| remember),
            clicked,
        });
    }

    /// «Открыть» или «Отмена» в окошке.
    pub(super) fn answer_app_link(&mut self, id: u64, open: bool, always: bool) {
        let Some(pending) = self.app_links.pending.take_if(|p| p.id == id) else {
            return;
        };
        if let Some(webview) = self.tabs.get(pending.tab).and_then(|t| t.webview.as_ref()) {
            let _ = webview.focus();
        }
        if !open {
            if !pending.clicked {
                self.app_links.muted = Some((pending.tab, Instant::now()));
            }
            return;
        }
        if self.tabs.get(pending.tab).is_none() {
            return;
        }
        if always && let Some(site) = &pending.site {
            let scheme = external::scheme_of(&pending.link).to_string();
            self.link_perms().allow(site, &scheme);
            if let Some(links) = &self.app_links.links
                && let Err(err) = links.save(&self.paths)
            {
                eprintln!("разрешения ссылок не сохранены: {err:#}");
            }
        }
        external::launch(pending.link, self.events.clone());
    }

    /// Открыли другую вкладку: окошко прежней больше не к месту.
    pub(super) fn close_app_link_unless(&mut self, active: TabId) {
        if self
            .app_links
            .pending
            .take_if(|p| p.tab != active)
            .is_some()
        {
            self.send(&ToUi::AppLink {
                id: 0,
                site: "",
                app: "",
                scheme: "",
                link: "",
                remember: false,
            });
        }
    }

    /// Разрешения читаются с диска при первой ссылке, а не при запуске.
    fn link_perms(&mut self) -> &mut AppLinks {
        self.app_links
            .links
            .get_or_insert_with(|| AppLinks::load(&self.paths))
    }
}

/// Ответ страницы на `INPUT_PROBE` — число миллисекунд с последнего клика (или -1).
fn clicked_just_now(raw: &str) -> bool {
    let ago = raw.trim().parse::<f64>().unwrap_or(-1.0);
    (0.0..=external::INPUT_FRESH_MS).contains(&ago)
}

/// `https://example.com:8443` — origin веб-страницы; остальное запомнить нельзя.
fn web_origin(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

/// `example.com` или `127.0.0.1:8765` для окошка.
fn display_host(origin: &str) -> String {
    Url::parse(origin)
        .ok()
        .and_then(|u| {
            let host = u.host_str()?.to_string();
            Some(match u.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(
            web_origin("https://Example.com/a?b#c").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            web_origin("http://127.0.0.1:8765/t.html").as_deref(),
            Some("http://127.0.0.1:8765")
        );
        assert_eq!(web_origin("file:///C:/a.html"), None);
        assert_eq!(web_origin("null"), None);
        assert_eq!(display_host("https://example.com"), "example.com");
        assert_eq!(display_host("http://127.0.0.1:8765"), "127.0.0.1:8765");
        // Адрес фрейма, открывшего окно, тоже сводится к его сайту.
        assert_eq!(
            web_origin("https://widget.example/frame.html?x=1").as_deref(),
            Some("https://widget.example")
        );
    }

    #[test]
    fn fresh_click_is_within_a_second() {
        assert!(clicked_just_now("0"));
        assert!(clicked_just_now(" 420.5 "));
        assert!(clicked_just_now("1000"));
        assert!(!clicked_just_now("1500"));
        assert!(!clicked_just_now("2999"));
        assert!(!clicked_just_now("-1"));
        assert!(!clicked_just_now("null"));
    }
}
