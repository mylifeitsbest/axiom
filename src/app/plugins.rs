//! Плагины в браузере: «Настройки → Плагины», кнопки плагинов на панели и
//! запросы их скриптов из вкладок. Что плагину можно, решается здесь: страница
//! (и сам скрипт плагина) может прислать что угодно.

use serde_json::{Map, Value};

use super::App;
use crate::{
    downloads,
    events::UserEvent,
    ipc::ToUi,
    plugins::{
        self,
        api::{self, Op, Request},
        bridge,
        manifest::Permission,
        market,
    },
    tabs::TabId,
};

impl App {
    pub(super) fn push_plugins(&self) {
        self.send(&ToUi::Plugins(self.plugins.view(&self.market.status)));
    }

    fn push_market(&self) {
        let m = &self.market;
        let items = m
            .items
            .iter()
            .flatten()
            .map(|item| {
                let local = self.plugins.get(&item.id);
                market::ItemView {
                    item,
                    installed: local.map(|p| p.manifest.version.as_str()),
                    from_store: local.is_some_and(|p| p.from_store.is_some()),
                }
            })
            .collect();
        self.send(&ToUi::Market(market::View {
            loaded: m.items.is_some(),
            loading: m.loading,
            error: m.error.as_deref(),
            items,
            busy: &m.busy,
        }));
    }

    /// Открыли «Магазин»: каталог (каждый раз заново — там могли появиться новые)
    /// и что с плагинами, которые публиковали отсюда.
    pub(super) fn market_open(&mut self) {
        if !self.market.loading {
            self.market.loading = true;
            self.market.error = None;
            market::load_index(&self.events);
            market::load_status(&self.paths.root, &self.events);
        }
        self.push_market();
    }

    pub(super) fn market_install(&mut self, id: &str) {
        if self.market.busy.iter().any(|b| b == id) {
            return;
        }
        let Some(item) = self.market.item(id) else {
            return;
        };
        if self.plugins.get(id).is_some_and(|p| p.from_store.is_none()) {
            self.toast_text(&format!(
                "У вас уже есть свой плагин «{id}» — переименуйте или удалите его папку"
            ));
            return;
        }
        market::download(item, &self.events);
        self.market.set_busy(id, true);
        self.push_market();
    }

    pub(super) fn plugin_publish(&mut self, id: &str, about: market::Publish) {
        let Some(plugin) = self.plugins.get(id) else {
            return;
        };
        if plugin.from_store.is_some() {
            self.toast_text(
                "Этот плагин поставлен из магазина — опубликовать его может только автор",
            );
            return;
        }
        if self.market.busy.iter().any(|b| b == id) {
            return;
        }
        let dir = self.plugins.dir().join(&plugin.folder);
        let cut = |s: &str, n: usize| s.trim().chars().take(n).collect::<String>();
        let about = market::Publish {
            author: cut(&about.author, market::MAX_AUTHOR),
            contact: cut(&about.contact, market::MAX_CONTACT),
            note: cut(&about.note, market::MAX_NOTE),
        };
        match market::publish(&self.paths.root, &dir, id, about, &self.events) {
            Ok(()) => {
                self.market.set_busy(id, true);
                self.push_plugins();
                self.push_market();
                self.toast_text("Плагин отправляется в магазин…");
            }
            Err(err) => self.toast_text(&format!("Плагин не отправлен: {err}")),
        }
    }

    pub(super) fn on_market(&mut self, event: market::Event) {
        match event {
            market::Event::Index(result) => {
                self.market.loading = false;
                match result {
                    Ok(items) => {
                        self.market.items = Some(items);
                        self.market.error = None;
                    }
                    Err(err) => self.market.error = Some(err),
                }
                self.push_market();
            }
            market::Event::Status(status) => {
                self.market.status = status;
                self.push_plugins();
            }
            market::Event::Downloaded { id, result } => {
                self.market.set_busy(&id, false);
                let dir = self.plugins.dir();
                let installed = result.and_then(|bundle| {
                    market::install(&dir, &id, &bundle).map(|fresh| (fresh, bundle.version))
                });
                match installed {
                    Ok((fresh, version)) => {
                        self.plugins.rescan();
                        // Поставили — значит, хотят пользоваться: включаем сразу.
                        // Обновление оставляет плагин таким, каким он был.
                        let enabled = if fresh {
                            self.plugins.set_enabled(&id, true)
                        } else {
                            Ok(false)
                        };
                        self.plugins_changed();
                        let name = self.plugin_name(&id);
                        let text = match enabled {
                            Err(err) => format!("«{name}» поставлен, но не включился: {err}"),
                            Ok(_) if fresh => format!(
                                "«{name}» установлен — работает на страницах, которые вы откроете или обновите"
                            ),
                            Ok(_) => format!("«{name}» обновлён до {version} — обновите страницу"),
                        };
                        self.toast_text(&text);
                    }
                    Err(err) => self.toast_text(&format!("Не удалось поставить плагин: {err}")),
                }
                self.push_market();
            }
            market::Event::Published { id, result } => {
                self.market.set_busy(&id, false);
                match result {
                    Ok(version) => {
                        self.market.status.insert(
                            id.clone(),
                            market::Status {
                                version,
                                status: "pending".into(),
                                reason: String::new(),
                                published: self
                                    .market
                                    .status
                                    .get(&id)
                                    .and_then(|s| s.published.clone()),
                            },
                        );
                        let name = self.plugin_name(&id);
                        self.toast_text(&format!(
                            "«{name}» отправлен на проверку — после неё он появится в магазине"
                        ));
                    }
                    Err(err) => self.toast_text(&format!("Плагин не отправлен: {err}")),
                }
                self.push_plugins();
                self.push_market();
            }
        }
    }

    /// Набор или код плагинов поменялся: всем живым вкладкам — новые скрипты.
    fn plugins_changed(&mut self) {
        let scripts = self.plugins.scripts();
        for tab in self.tabs.iter() {
            if let Some(webview) = &tab.webview {
                bridge::apply(webview, tab.id, &self.events, scripts.clone());
            }
        }
        // Страниц выключенных и удалённых плагинов больше не ждём.
        self.plugin_pages
            .retain(|(_, id), _| scripts.iter().any(|s| &s.plugin == id));
        self.push_plugins();
    }

    fn plugin_name(&self, id: &str) -> String {
        self.plugins
            .get(id)
            .map_or_else(|| id.to_owned(), |p| p.manifest.name.clone())
    }

    fn toast_text(&self, text: &str) {
        self.send(&ToUi::Toast { text });
    }

    pub(super) fn plugin_toggle(&mut self, id: &str, on: bool) {
        match self.plugins.set_enabled(id, on) {
            Ok(true) => {}
            Ok(false) => {
                self.push_plugins();
                return;
            }
            Err(err) => {
                // Файлы плагина поменяли после загрузки, и теперь он не читается.
                self.plugins_changed();
                self.toast_text(&format!("Плагин не включён: {err}"));
                return;
            }
        }
        self.plugins_changed();
        let name = self.plugin_name(id);
        let text = if on {
            format!("«{name}» включён — заработает на страницах, которые вы откроете или обновите")
        } else {
            format!("«{name}» выключен — на открытых страницах он работает, пока их не обновят")
        };
        self.toast_text(&text);
    }

    pub(super) fn plugin_settings(&mut self, id: &str, incoming: &Map<String, Value>) {
        let Some(values) = self.plugins.set_values(id, incoming) else {
            return;
        };
        self.plugins_changed();
        // Страницам, где плагин уже работает, — новые значения сразу (`axiom.on('settings')`).
        let message = api::event("settings", Some(&values));
        let pages: Vec<(TabId, Option<i64>)> = self
            .plugin_pages
            .iter()
            .filter(|((_, p), _)| p == id)
            .map(|((tab, _), context)| (*tab, *context))
            .collect();
        for (tab, context) in pages {
            self.plugin_send(tab, id, context, &message, |_| {});
        }
    }

    pub(super) fn plugin_action(&mut self, id: &str) {
        if self.plugins.active(id).is_none() {
            return;
        }
        let Some(tab) = self.tabs.active else {
            return;
        };
        let context = self.plugin_pages.get(&(tab, id.to_owned())).copied();
        let events = self.events.clone();
        let plugin = id.to_owned();
        let sent = context.is_some()
            && self.plugin_send(
                tab,
                id,
                context.flatten(),
                &api::event("action", None),
                move |ok| {
                    if !ok {
                        let _ = events.send_event(UserEvent::PluginMissed { tab, plugin });
                    }
                },
            );
        if !sent {
            self.plugin_missed(tab, id);
        }
    }

    /// Кнопку плагина нажали, а скрипта плагина на странице нет (или он не слушает кнопку).
    pub(super) fn plugin_missed(&self, tab: TabId, id: &str) {
        if self.tabs.active != Some(tab) {
            return;
        }
        let name = self.plugin_name(id);
        self.toast_text(&format!("«{name}» не работает на этой странице"));
    }

    pub(super) fn plugins_reload(&mut self) {
        self.plugins.rescan();
        self.plugins_changed();
        self.toast_text("Плагины перечитаны — обновите страницу, чтобы увидеть изменения");
    }

    pub(super) fn plugins_folder(&self) {
        let dir = self.plugins.dir();
        if let Err(err) = crate::storage::private_dir(&dir)
            .map_err(anyhow::Error::from)
            .and_then(|()| downloads::open(&dir))
        {
            self.toast_error(&err);
        }
    }

    pub(super) fn plugin_remove(&mut self, folder: &str) {
        match self.plugins.remove(folder) {
            Ok(()) => {
                self.plugins_changed();
                self.toast_text("Плагин удалён");
            }
            Err(err) => {
                self.toast_text(&err);
                self.push_plugins();
            }
        }
    }

    /// Запрос скрипта плагина. Ответ уходит туда же, откуда пришёл запрос.
    pub(super) fn on_plugin(&mut self, msg: plugins::Message) {
        let plugins::Message {
            tab,
            plugin,
            context,
            raw,
        } = msg;
        if self.plugins.active(&plugin).is_none()
            || self.tabs.get(tab).is_none_or(|t| t.webview.is_none())
        {
            return;
        }
        let Request { id, op } = match Request::parse(&raw) {
            Ok(request) => request,
            Err(err) => {
                eprintln!("плагин {plugin}: {err}");
                return;
            }
        };
        let result = match op {
            Op::Hello {} => {
                self.plugin_pages.insert((tab, plugin), context);
                return;
            }
            Op::StorageGet { keys } => Ok(self.plugins.store(&plugin).get(keys.as_deref())),
            Op::StorageSet { items } => {
                self.plugins.store(&plugin).set(items).map(|()| Value::Null)
            }
            Op::StorageRemove { keys } => self
                .plugins
                .store(&plugin)
                .remove(&keys)
                .map(|()| Value::Null),
            Op::StorageClear {} => self.plugins.store(&plugin).clear().map(|()| Value::Null),
            Op::Notify { text } => self.plugin_notify(&plugin, &text),
            Op::TabsOpen { url, background } => self.plugin_open_tab(&plugin, tab, url, background),
        };
        if let Some(id) = id {
            self.plugin_send(tab, &plugin, context, &api::reply(id, result), |_| {});
        }
    }

    fn plugin_notify(&mut self, plugin: &str, text: &str) -> Result<Value, String> {
        self.plugin_may(plugin, Permission::Notify)?;
        let text = text.trim();
        if text.is_empty() {
            return Err("пустое сообщение".into());
        }
        let short: String = text.chars().take(api::MAX_NOTIFY).collect();
        let name = self.plugin_name(plugin);
        self.toast_text(&format!("{name}: {short}"));
        Ok(Value::Null)
    }

    fn plugin_open_tab(
        &mut self,
        plugin: &str,
        from: TabId,
        url: String,
        background: bool,
    ) -> Result<Value, String> {
        let page = url::Url::parse(&url)
            .ok()
            .filter(|u| url.len() <= api::MAX_URL && matches!(u.scheme(), "http" | "https"))
            .ok_or("axiom.tabs.open: только адреса http и https")?;
        self.plugin_may(plugin, Permission::Tabs)?;
        let ai = self.tabs.get(from).is_some_and(|t| t.ai);
        self.add_page_tab(page.into(), ai, Some(from), background);
        Ok(Value::Null)
    }

    /// Разрешение есть в манифесте и плагин не превысил частоту.
    fn plugin_may(&mut self, plugin: &str, perm: Permission) -> Result<(), String> {
        let allowed = self
            .plugins
            .get(plugin)
            .is_some_and(|p| p.manifest.allows(perm));
        if !allowed {
            let name = match perm {
                Permission::Notify => "notify",
                Permission::Tabs => "tabs",
            };
            return Err(format!("нужно разрешение \"{name}\" в plugin.json"));
        }
        if !self.plugins.allow(plugin, perm) {
            return Err("слишком часто".into());
        }
        Ok(())
    }

    /// Сообщение скрипту плагина во вкладке. `false` — отдать некуда (нет страницы).
    fn plugin_send(
        &self,
        tab: TabId,
        plugin: &str,
        context: Option<i64>,
        message: &str,
        done: impl FnOnce(bool) + 'static,
    ) -> bool {
        let scripts = self.plugins.scripts();
        let (Some(webview), Some(script)) = (
            self.tabs.get(tab).and_then(|t| t.webview.as_ref()),
            scripts.iter().find(|s| s.plugin == plugin),
        ) else {
            return false;
        };
        bridge::deliver(webview, script, context, message, done);
        true
    }

    /// Вкладки без webview (закрытые, уснувшие) плагинам больше не нужны.
    pub(super) fn prune_plugin_pages(&mut self) {
        let live = |id: TabId| self.tabs.get(id).is_some_and(|t| t.webview.is_some());
        bridge::retain(live);
        let alive: std::collections::HashSet<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.webview.is_some())
            .map(|t| t.id)
            .collect();
        self.plugin_pages.retain(|(tab, _), _| alive.contains(tab));
    }
}
