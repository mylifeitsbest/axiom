//! Разрешения сайтов: решает по памяти (`storage::permissions`) или спрашивает
//! окошком под адресной строкой; Настройки → Безопасность меняют память.

use url::Url;

use super::App;
use crate::{
    ipc::{PermAnswer, ToUi},
    storage::permissions::{Decision, Perm, Permissions},
    tabs::{
        TabId,
        permissions::{self as engine, Request, Saved},
    },
};

#[derive(Default)]
pub(super) struct State {
    /// Читается с диска при первой надобности.
    store: Option<Permissions>,
    /// Запросы, которые ждут ответа человека (по всем вкладкам).
    asks: Vec<Request>,
    /// Окошко, открытое сейчас: для какой вкладки и какого сайта.
    shown: Option<Shown>,
    /// «Только сейчас»: до ухода вкладки со страницы.
    once: Vec<(TabId, Option<String>, Perm)>,
    next_id: u64,
}

struct Shown {
    id: u64,
    tab: TabId,
    site: Option<String>,
    kinds: Vec<Perm>,
}

impl App {
    /// Запрос страницы: ответить по памяти или спросить.
    pub(super) fn on_permission(&mut self, request: Request) {
        match self.decide_permission(&request) {
            Some(allow) => engine::resolve(request.id, allow),
            None => {
                self.permissions.asks.push(request);
                self.show_permission_ask();
            }
        }
    }

    /// `Some` — решено без вопроса, `None` — спросить.
    fn decide_permission(&mut self, request: &Request) -> Option<bool> {
        if !self.permission_page_alive(request.tab) {
            return Some(false);
        }
        // Названия микрофонов и камер — тому, кому они уже разрешены.
        if request.device_info {
            let granted = [Perm::Microphone, Perm::Camera].into_iter().any(|p| {
                self.permission_state(request.tab, request.site.as_deref(), p)
                    == Some(Decision::Allow)
            });
            return Some(granted);
        }
        let mut all_allowed = true;
        for &kind in &request.kinds {
            match self.permission_state(request.tab, request.site.as_deref(), kind) {
                Some(Decision::Block) => return Some(false),
                Some(Decision::Allow) => {}
                _ => all_allowed = false,
            }
        }
        all_allowed.then_some(true)
    }

    /// Решение для вкладки: «только сейчас», потом память.
    fn permission_state(&mut self, tab: TabId, site: Option<&str>, kind: Perm) -> Option<Decision> {
        if self
            .permissions
            .once
            .iter()
            .any(|(t, s, k)| *t == tab && s.as_deref() == site && *k == kind)
        {
            return Some(Decision::Allow);
        }
        site.and_then(|site| self.permission_store().get(site, kind))
    }

    /// Страница, которая спрашивает, ещё жива: вкладка не уснула, панель ИИ на месте.
    fn permission_page_alive(&self, tab: TabId) -> bool {
        if tab == engine::AI {
            return self.ai.webview.is_some();
        }
        self.tabs.get(tab).is_some_and(|t| t.webview.is_some())
    }

    /// Чей запрос сейчас можно показать: активной вкладки или открытой панели ИИ.
    fn permission_visible(&self, tab: TabId) -> bool {
        if tab == engine::AI {
            return self.ai.open && self.ai.webview.is_some();
        }
        self.tabs.active == Some(tab)
    }

    /// Показывает окошко для активной вкладки (или открытой панели ИИ), если она
    /// чего-то ждёт; окошко другой вкладки прячет (её запросы ждут, пока на неё не вернутся).
    pub(super) fn show_permission_ask(&mut self) {
        if let Some(shown) = &self.permissions.shown
            && !self.permission_visible(shown.tab)
        {
            self.hide_permission_ask();
        }
        let (active, site) = match &self.permissions.shown {
            Some(shown) => (shown.tab, shown.site.clone()),
            None => match self
                .permissions
                .asks
                .iter()
                .find(|a| self.permission_visible(a.tab))
            {
                Some(ask) => (ask.tab, ask.site.clone()),
                None => return,
            },
        };
        // Все виды, о которых сейчас просит этот сайт в этой вкладке (камера и
        // микрофон приходят двумя запросами подряд — окошко одно).
        let mut kinds = Vec::new();
        for ask in self
            .permissions
            .asks
            .iter()
            .filter(|a| a.tab == active && a.site == site)
        {
            for &kind in &ask.kinds {
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
        }
        if kinds.is_empty() {
            self.hide_permission_ask();
            return;
        }
        kinds.sort();
        if self
            .permissions
            .shown
            .as_ref()
            .is_some_and(|s| s.kinds == kinds)
        {
            return;
        }
        let id = match &self.permissions.shown {
            Some(shown) => shown.id,
            None => {
                self.permissions.next_id += 1;
                self.permissions.next_id
            }
        };
        let page = self
            .tabs
            .get(active)
            .and_then(|t| engine::origin_of(&t.url))
            .filter(|page| site.as_ref() != Some(page))
            .map(|page| display_host(&page))
            .unwrap_or_default();
        // Поверх видео на весь экран окошко было бы не видно.
        if self.fullscreen.is_on() {
            self.leave_fullscreen();
        }
        let shown_site = site.as_deref().map(display_host).unwrap_or_default();
        self.send(&ToUi::PermissionAsk {
            id,
            site: &shown_site,
            page: &page,
            kinds: kinds.clone(),
            remember: site.is_some(),
        });
        self.permissions.shown = Some(Shown {
            id,
            tab: active,
            site,
            kinds,
        });
    }

    fn hide_permission_ask(&mut self) {
        if self.permissions.shown.take().is_some() {
            self.send(&ToUi::PermissionAsk {
                id: 0,
                site: "",
                page: "",
                kinds: Vec::new(),
                remember: false,
            });
        }
    }

    /// Кнопка в окошке.
    pub(super) fn answer_permission(&mut self, id: u64, answer: PermAnswer) {
        let Some(shown) = self.permissions.shown.take_if(|s| s.id == id) else {
            return;
        };
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.permissions.asks)
            .into_iter()
            .partition(|a| a.tab == shown.tab && a.site == shown.site);
        self.permissions.asks = rest;
        // Без адреса запомнить нельзя: «Разрешить» — только на этот раз.
        let answer = match (answer, &shown.site) {
            (PermAnswer::Allow, None) => PermAnswer::Once,
            (PermAnswer::Block, None) => PermAnswer::Dismiss,
            (answer, _) => answer,
        };
        let mut changed = false;
        for &kind in &shown.kinds {
            let remembered = kind.remembered();
            match answer {
                PermAnswer::Allow | PermAnswer::Block if remembered => {
                    let decision = if answer == PermAnswer::Allow {
                        Decision::Allow
                    } else {
                        Decision::Block
                    };
                    if let Some(site) = &shown.site {
                        changed |= self.permission_store().set(site, kind, Some(decision));
                    }
                }
                // Показ экрана не запоминается и не повторяется: «Разрешить» — на этот раз.
                PermAnswer::Allow | PermAnswer::Once if kind != Perm::Screen => {
                    self.permissions
                        .once
                        .push((shown.tab, shown.site.clone(), kind));
                }
                _ => {}
            }
        }
        let allow = matches!(answer, PermAnswer::Allow | PermAnswer::Once);
        for ask in mine {
            // Пришёл, пока окошко было открыто, и человек его не видел — спросим ещё раз.
            if ask.kinds.iter().any(|k| !shown.kinds.contains(k)) {
                self.permissions.asks.push(ask);
                continue;
            }
            engine::resolve(ask.id, allow);
        }
        if changed {
            self.save_permissions();
        }
        let webview = if shown.tab == engine::AI {
            self.ai.webview.as_ref()
        } else {
            self.tabs.get(shown.tab).and_then(|t| t.webview.as_ref())
        };
        if let Some(webview) = webview {
            let _ = webview.focus();
        }
        self.show_permission_ask();
    }

    /// Вкладка ушла со страницы или закрылась: её запросы больше не к месту,
    /// «только сейчас» кончилось.
    pub(super) fn permissions_tab_gone(&mut self, tab: TabId) {
        let (gone, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.permissions.asks)
            .into_iter()
            .partition(|a| a.tab == tab);
        self.permissions.asks = rest;
        for ask in gone {
            engine::resolve(ask.id, false);
        }
        self.permissions.once.retain(|(t, _, _)| *t != tab);
        if self
            .permissions
            .shown
            .as_ref()
            .is_some_and(|s| s.tab == tab)
        {
            self.hide_permission_ask();
        }
    }

    /// Вкладки, которые уснули или пропали, ответа уже не ждут.
    pub(super) fn prune_permission_asks(&mut self) {
        let mut dead: Vec<TabId> = self
            .permissions
            .asks
            .iter()
            .map(|a| a.tab)
            .chain(self.permissions.once.iter().map(|(t, _, _)| *t))
            .filter(|&t| !self.permission_page_alive(t))
            .collect();
        dead.sort_unstable();
        dead.dedup();
        for tab in dead {
            self.permissions_tab_gone(tab);
        }
    }

    // ---------- Настройки → Безопасность ----------

    pub(super) fn set_site_permission(
        &mut self,
        site: &str,
        kind: Perm,
        decision: Option<Decision>,
    ) {
        if self.permission_store().set(site, kind, decision) {
            // Запрет снимает и «только сейчас».
            if decision != Some(Decision::Allow) {
                self.permissions
                    .once
                    .retain(|(_, s, k)| !(s.as_deref() == Some(site) && *k == kind));
            }
            self.save_permissions();
        }
    }

    pub(super) fn forget_site(&mut self, site: &str) {
        self.permissions
            .once
            .retain(|(_, s, _)| s.as_deref() != Some(site));
        if self.permission_store().forget(site) {
            self.save_permissions();
        }
    }

    pub(super) fn set_permission_default(&mut self, kind: Perm, block: bool) {
        let decision = block.then_some(Decision::Block);
        if self.permission_store().set_default(kind, decision) {
            self.save_permissions();
        }
    }

    pub(super) fn forget_app_link(&mut self, site: &str, scheme: &str) {
        if self.remove_app_link(site, scheme) {
            self.push_permissions();
        }
    }

    /// Что WebView2 помнил сам: переносим, но своё решение Axiom не перезаписываем.
    pub(super) fn on_permissions_saved(&mut self, saved: Vec<Saved>) {
        let mut changed = false;
        for item in saved {
            let store = self.permission_store();
            if store.own(&item.site, item.perm).is_none() {
                changed |= store.set(&item.site, item.perm, Some(item.decision));
            }
        }
        if changed {
            self.save_permissions();
        }
    }

    /// Списки для настроек и адресной строки.
    pub(super) fn push_permissions(&mut self) {
        self.permission_store();
        let links = self.app_link_entries();
        let Some(store) = &self.permissions.store else {
            return;
        };
        self.send(&ToUi::Permissions {
            sites: store.sites(),
            defaults: store.defaults(),
            app_links: &links,
        });
    }

    fn save_permissions(&mut self) {
        if let Some(store) = &self.permissions.store
            && let Err(err) = store.save(&self.paths)
        {
            eprintln!("разрешения сайтов не сохранены: {err:#}");
        }
        self.push_permissions();
    }

    fn permission_store(&mut self) -> &mut Permissions {
        self.permissions
            .store
            .get_or_insert_with(|| Permissions::load(&self.paths))
    }
}

/// `meet.google.com` или `127.0.0.1:8765` для окошка.
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
    fn hosts_for_the_prompt() {
        assert_eq!(display_host("https://meet.google.com"), "meet.google.com");
        assert_eq!(display_host("http://127.0.0.1:8765"), "127.0.0.1:8765");
        assert_eq!(display_host("garbage"), "");
    }
}
