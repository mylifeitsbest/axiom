//! Прокси для всех вкладок и панели ИИ — например, WARP через `usque socks`.
//!
//! Адрес — в «Настройки → Сеть» (или в переменной `AXIOM_PROXY` на один запуск),
//! включается и выключается кнопкой на панели. Движок ходит через свой релей на
//! 127.0.0.1 (см. [`relay`]), поэтому кнопка работает сразу, без перезапуска.
//!
//! Релей поднимается, только если адрес задан к запуску Axiom: без адреса движок
//! ходит напрямую, как раньше, и релея нет вовсе. Самый первый адрес движку на ходу
//! уже не подсунуть (WebView2) — он заработает после перезапуска.
//!
//! Локальная сеть и сам компьютер (`localhost`, 192.168.…, 10.…) идут мимо прокси,
//! как у WARP: роутер и домашний сервер открываются и с включённым прокси.

pub mod address;
pub mod relay;

use std::{
    net::SocketAddr,
    sync::{Mutex, OnceLock, PoisonError},
};

use serde::Serialize;
use tao::event_loop::EventLoopProxy;

pub use relay::Health;
use relay::{Relay, Route};

use crate::events::UserEvent;

/// Адрес релея, с которым запущен движок; `None` — движок ходит напрямую.
/// Задаётся один раз, до первого webview: у всех webview профиля он обязан
/// совпадать (WebView2 не создаст webview с другим `--proxy-server`).
static ENGINE: OnceLock<Option<SocketAddr>> = OnceLock::new();

/// Релей, через который ходит движок (SOCKS5), — для аргументов WebView2 и WebKitGTK.
pub fn engine_proxy() -> Option<SocketAddr> {
    ENGINE.get().copied().flatten()
}

/// Мимо прокси: сам компьютер и локальные сети (как в WARP), включая CGNAT
/// (Tailscale). Chromium и GLib понимают одинаковую запись.
const BYPASS: &[&str] = &[
    "localhost",
    "*.localhost",
    "*.local",
    "127.0.0.0/8",
    "::1",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "100.64.0.0/10",
    "fc00::/7",
    "fe80::/10",
];

/// Аргументы WebView2 для прокси движка: релей и обход локальной сети.
/// `<local>` — имена без точки (`router`, `nas`).
pub fn browser_args() -> Option<String> {
    let relay = engine_proxy()?;
    Some(format!(
        "--proxy-server=socks5://{relay} --proxy-bypass-list=<local>;{}",
        BYPASS.join(";")
    ))
}

/// WebKitGTK: прокси ставится на окружение один раз, для всех его webview, — через
/// первый webview (интерфейс), до первой вкладки. wry умеет только без списка
/// исключений, а без него и localhost ушёл бы в прокси.
#[cfg(target_os = "linux")]
pub fn apply_to(webview: &wry::WebView) {
    use webkit2gtk::{
        NetworkProxyMode, NetworkProxySettings, WebContextExt as _, WebViewExt as _,
        WebsiteDataManagerExt as _,
    };
    use wry::WebViewExtUnix as _;
    let Some(relay) = engine_proxy() else { return };
    let uri = format!("socks5://{relay}");
    let Some(context) = webview.webview().context() else {
        return;
    };
    if let Some(manager) = context.website_data_manager() {
        let mut settings = NetworkProxySettings::new(Some(uri.as_str()), BYPASS);
        manager.set_network_proxy_settings(NetworkProxyMode::Custom, Some(&mut settings));
    }
}

/// Прокси вкладок: адрес, включён ли он и релей, через который ходит движок.
pub struct TabProxy {
    relay: Option<Relay>,
    /// Адрес из `AXIOM_PROXY`: на этот запуск важнее настройки и не сохраняется.
    env: bool,
    /// Адрес, по которому ходит релей, когда прокси включён.
    addr: String,
    route: Option<Route>,
    on: bool,
}

/// Состояние для кнопки на панели и раздела «Сеть».
#[derive(Debug, Serialize)]
pub struct View<'a> {
    /// Адрес задан — кнопка на панели видна.
    pub shown: bool,
    pub on: bool,
    /// `up`, `down`, `checking`, `off` или `restart` (адрес задан после запуска:
    /// движок ещё без релея).
    pub state: &'static str,
    pub addr: &'a str,
    /// Действует `AXIOM_PROXY`, а не адрес из настроек.
    pub env: bool,
}

impl TabProxy {
    /// До первого webview: решает, с каким прокси запустится движок.
    pub fn start(saved: &str, saved_on: bool, events: &EventLoopProxy<UserEvent>) -> Self {
        let env = std::env::var("AXIOM_PROXY")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .filter(|raw| match address::parse(raw) {
                Ok(Some(_)) => true,
                _ => {
                    eprintln!("AXIOM_PROXY не подошёл ({raw}): прокси из настроек");
                    false
                }
            });
        let (addr, on) = match &env {
            Some(raw) => (raw.clone(), true),
            None => (saved.to_string(), saved_on),
        };
        let route = address::parse(&addr).ok().flatten();
        let relay = route.as_ref().and_then(|route| {
            let first = if on { route.clone() } else { Route::Direct };
            let events = Mutex::new(events.clone());
            let notify = move |health| {
                let events = events.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = events.send_event(UserEvent::Proxy(health));
            };
            match Relay::start(first, notify) {
                Ok(relay) => Some(relay),
                Err(err) => {
                    eprintln!("прокси: релей не запустился, вкладки идут напрямую: {err}");
                    None
                }
            }
        });
        let _ = ENGINE.set(relay.as_ref().map(Relay::addr));
        let proxy = Self {
            relay,
            env: env.is_some(),
            addr,
            route,
            on,
        };
        proxy.check();
        proxy
    }

    /// Трафик сейчас идёт через прокси человека.
    pub fn active(&self) -> bool {
        self.on && self.relay.is_some() && self.route.is_some()
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Адрес задан переменной окружения: настройки его не меняют и не сохраняют.
    pub fn env_override(&self) -> bool {
        self.env
    }

    /// Нужен перезапуск: адрес есть, а движок запущен без релея.
    pub fn needs_restart(&self) -> bool {
        self.route.is_some() && self.relay.is_none()
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Включает или выключает прокси. `false` — ничего не поменялось.
    pub fn set_on(&mut self, on: bool) -> bool {
        if self.on == on || self.route.is_none() {
            return false;
        }
        self.on = on;
        self.apply();
        true
    }

    /// Новый адрес из настроек (уже проверенный). Пока действует `AXIOM_PROXY`,
    /// настройка ждёт следующего запуска.
    pub fn set_addr(&mut self, raw: &str, on: bool) {
        if self.env {
            return;
        }
        self.addr = raw.to_string();
        self.route = address::parse(raw).ok().flatten();
        self.on = on && self.route.is_some();
        self.apply();
    }

    fn apply(&self) {
        let Some(relay) = &self.relay else { return };
        let route = match (&self.route, self.on) {
            (Some(route), true) => route.clone(),
            _ => Route::Direct,
        };
        relay.set_route(route);
        self.check();
    }

    /// Проверить в фоне, отвечает ли прокси (итог придёт событием `Proxy`).
    pub fn check(&self) {
        if self.active()
            && let Some(relay) = &self.relay
        {
            relay.check();
        }
    }

    pub fn view(&self) -> View<'_> {
        let state = match &self.relay {
            _ if self.route.is_none() => "off",
            None => "restart",
            Some(_) if !self.on => "off",
            Some(relay) => match relay.health() {
                Health::Up => "up",
                Health::Down => "down",
                Health::Unknown => "checking",
            },
        };
        View {
            shown: self.route.is_some(),
            on: self.on,
            state,
            addr: &self.addr,
            env: self.env,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_list_is_one_token() {
        // В аргументах WebView2 список — одно слово: пробел разорвал бы его.
        assert!(BYPASS.iter().all(|h| !h.contains([' ', ';', '"'])));
    }
}
