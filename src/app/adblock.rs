//! Блокировка рекламы: события блокировщика, кнопка в адресной строке и раздел
//! настроек (см. `crate::adblock`).

use super::App;
use crate::{adblock::Event, ipc::ToUi, tabs::sleep};

impl App {
    pub(super) fn on_adblock(&mut self, event: Event) {
        if matches!(event, Event::Counted) {
            self.adblock.counted();
            self.push_blocked();
            return;
        }
        if self.adblock.on_event(event) {
            self.push_adblock();
        }
    }

    /// Счётчик активной вкладки (остальные приходят с состоянием вкладок).
    pub(super) fn push_blocked(&self) {
        let Some(id) = self.tabs.active else {
            return;
        };
        self.send(&ToUi::Blocked {
            id,
            count: crate::adblock::blocked(id),
            total: self.adblock.view(&self.settings.adblock()).total,
        });
    }

    pub(super) fn push_adblock(&self) {
        let prefs = self.settings.adblock();
        self.send(&ToUi::Adblock(self.adblock.view(&prefs)));
    }

    /// Настройки блокировщика поменялись (переключатели, список сайтов).
    pub(super) fn sync_adblock(&mut self) {
        if self.adblock.set_prefs(&self.settings.adblock()) {
            self.push_state();
        }
        self.push_adblock();
    }

    /// Кнопка в адресной строке: блокировать на сайте активной вкладки или нет.
    /// Страница перезагружается — иначе изменение видно только на следующей.
    /// Включили — без кэша: реклама, загруженная, пока сайт был разрешён, лежит в
    /// кэше памяти страницы, и оттуда она взялась бы без запроса, мимо блокировщика.
    pub(super) fn adblock_site(&mut self, on: bool) {
        let Some(tab) = self.tabs.active_tab() else {
            return;
        };
        let (id, url) = (tab.id, tab.url.clone());
        let listed = sleep::site_kept(&self.settings.adblock_sites, &url);
        if on == listed {
            match sleep::toggle_site(&mut self.settings.adblock_sites, &url) {
                Some(sleep::SiteToggle::Full) => {
                    self.send(&ToUi::Toast {
                        text: "Список сайтов без блокировки полон",
                    });
                    return;
                }
                Some(_) => {}
                None => return,
            }
        }
        // Блокировку выключили вовсе — кнопка «Блокировать» включает её снова.
        if on && !self.settings.adblock {
            self.settings.adblock = true;
            self.send(&ToUi::Settings {
                settings: &self.settings,
            });
        }
        self.persist_settings();
        self.sync_adblock();
        self.reload_tab_with(id, on);
    }

    /// Сайт из настроек — в список «не блокировать» или из него.
    pub(super) fn adblock_list(&mut self, site: &str, add: bool) {
        let before = self.settings.adblock_sites.clone();
        if add {
            let Some(site) = sleep::site(site) else {
                self.send(&ToUi::Toast {
                    text: "Это не похоже на адрес сайта",
                });
                return;
            };
            let mut sites = self.settings.adblock_sites.clone();
            sites.push(site);
            self.settings.adblock_sites = sleep::clean_sites(&sites);
        } else {
            self.settings.adblock_sites.retain(|s| s != site);
        }
        if self.settings.adblock_sites != before {
            self.persist_settings();
        }
        self.sync_adblock();
    }

    pub(super) fn adblock_update(&mut self) {
        if !self.adblock.check() {
            return;
        }
        self.push_adblock();
    }
}
