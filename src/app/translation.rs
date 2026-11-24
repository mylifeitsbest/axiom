//! Перевод страниц: связывает состояние из `crate::translate` с вкладками,
//! интерфейсом и настройками. Все вызовы страницы и запросы к Google начинаются
//! отсюда; страница только отвечает.

use std::time::Instant;

use super::App;
use crate::{
    events::UserEvent,
    ipc::ToUi,
    tabs::TabId,
    translate::{
        self, Batch, Event, MAX_ERRORS, MAX_GLOBAL_PER_MINUTE, MAX_PROBES, MAX_RESUMES_PER_MINUTE,
        MAX_RUNNING, Page, Prefs, Probe, Stage, View, google::Output, lang,
    },
};

impl App {
    /// Кнопка 文A: перевести активную вкладку.
    pub(super) fn translate_active(&mut self) {
        if let Some(id) = self.tabs.active {
            self.start_translate(id);
        }
    }

    /// «Показать оригинал».
    pub(super) fn translate_original(&mut self) {
        if let Some(id) = self.tabs.active {
            self.revert_page(id, Stage::Foreign);
        }
    }

    /// Язык перевода, «Всегда переводить» и «Предлагать перевод» (окошко и настройки).
    pub(super) fn translate_prefs(&mut self, prefs: Prefs) {
        let old = std::mem::replace(&mut self.settings.translate, prefs.sanitized());
        self.persist_settings();
        let prefs = self.settings.translate.clone();
        // Непереведённые страницы на новом языке могут оказаться «своими» и наоборот.
        for page in self.translate.pages.values_mut() {
            if matches!(page.stage, Stage::Native | Stage::Foreign) && !page.lang.is_empty() {
                page.stage = if lang::same(&page.lang, &prefs.to) {
                    Stage::Native
                } else {
                    Stage::Foreign
                };
            }
        }
        if let Some(id) = self.tabs.active
            && let Some(page) = self.translate.pages.get(&id)
        {
            let (stage, lang) = (page.stage, page.lang.clone());
            let translating = matches!(stage, Stage::Busy | Stage::Done);
            if translating && old.to != prefs.to {
                // Открытую переведённую страницу сразу переводим на новый язык.
                if lang::same(&lang, &prefs.to) {
                    self.revert_page(id, Stage::Native);
                } else {
                    self.start_translate(id);
                }
            } else if stage == Stage::Foreign && prefs.auto(&lang) && !old.auto(&lang) {
                self.start_translate(id);
            }
        }
        self.push_translate_all();
    }

    /// Начало и конец загрузки страницы во вкладке (из `on_load`).
    pub(super) fn translate_load(&mut self, id: TabId, started: bool) {
        let Some(url) = self.tabs.get(id).map(|t| translate::document_url(&t.url)) else {
            return;
        };
        let epoch = self.translate.epoch();
        let tabs = &self.tabs;
        self.translate
            .pages
            .retain(|tab, _| tabs.get(*tab).is_some());
        let page = self
            .translate
            .pages
            .entry(id)
            .or_insert_with(|| Page::new(epoch, url.clone()));
        if started {
            if page.translating() && page.url == url {
                // F5, пробуждение вкладки или переход по якорю: решим, когда загрузится.
                page.resume = true;
                return;
            }
            *page = Page::new(epoch, url);
            self.push_translate(id);
            return;
        }
        if page.url != url {
            // Перенаправление или переход без события начала загрузки.
            if page.translating() {
                *page = Page::new(epoch, url);
            } else {
                page.url = url;
            }
        }
        if page.stage == Stage::Probing || page.resume {
            page.probes = 0;
            self.probe(id);
        }
    }

    /// Ответы страницы и фоновых потоков.
    pub(super) fn on_translate(&mut self, event: Event) {
        match event {
            Event::Probed { tab, epoch, raw } => self.on_probed(tab, epoch, &raw),
            Event::Taken {
                tab,
                epoch,
                raw,
                limit,
            } => self.on_taken(tab, epoch, &raw, limit),
            Event::Translated { tab, epoch, result } => self.on_translated(tab, epoch, result),
            Event::Poll { tab, epoch } => self.on_poll(tab, epoch),
        }
    }

    /// Состояние кнопки для всех вкладок: интерфейс загрузился или поменялись настройки.
    pub(super) fn push_translate_all(&self) {
        for id in self.translate.pages.keys() {
            self.push_translate(*id);
        }
    }

    fn probe(&mut self, id: TabId) {
        let Some(page) = self.translate.pages.get_mut(&id) else {
            return;
        };
        let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) else {
            return;
        };
        page.probes += 1;
        let (epoch, events) = (page.epoch, self.events.clone());
        let _ = webview.evaluate_script_with_callback(translate::PROBE, move |raw| {
            let _ = events.send_event(UserEvent::Translate(Event::Probed {
                tab: id,
                epoch,
                raw,
            }));
        });
    }

    fn on_probed(&mut self, id: TabId, epoch: u64, raw: &str) {
        let probe = Probe::parse(raw);
        let Some(page) = self.translate.current(id, epoch) else {
            return;
        };
        let resume = page.translating() && std::mem::take(&mut page.resume);
        if probe.active {
            if resume {
                // Тот же документ (переход по якорю): перевод на месте, продолжаем.
                return;
            }
            // Переведённая страница из кэша «назад-вперёд»: начинаем с оригинала.
            self.eval(id, translate::REVERT);
        }
        if resume {
            // Тот же адрес, но новый документ (F5, пробуждение): переводим снова,
            // если только страница не перезагружает себя по кругу.
            let Some(page) = self.translate.current(id, epoch) else {
                return;
            };
            let now = Instant::now();
            if page.resumes.allow(now, MAX_RESUMES_PER_MINUTE) {
                page.resumes.record(now);
                self.start_translate(id);
            } else {
                self.revert_page(id, Stage::Foreign);
            }
            return;
        }
        let Some(page) = self.translate.current(id, epoch) else {
            return;
        };
        let Some(found) = lang::detect(&probe.lang, &probe.text) else {
            // Сайт ещё рисует себя скриптом: спросим чуть позже.
            if page.probes < MAX_PROBES {
                self.schedule_poll(id);
            } else {
                page.stage = Stage::Native;
            }
            return;
        };
        let prefs = &self.settings.translate;
        page.lang = found;
        let auto = prefs.auto(&page.lang);
        page.stage = if lang::same(&page.lang, &prefs.to) {
            Stage::Native
        } else {
            Stage::Foreign
        };
        if page.stage == Stage::Foreign && auto {
            self.start_translate(id);
        } else {
            self.push_translate(id);
        }
    }

    fn start_translate(&mut self, id: TabId) {
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        let Some(webview) = tab.webview.as_ref() else {
            return;
        };
        let url = translate::document_url(&tab.url);
        let epoch = self.translate.epoch();
        let page = self
            .translate
            .pages
            .entry(id)
            .or_insert_with(|| Page::new(epoch, url.clone()));
        page.url = url;
        page.begin(epoch, &self.settings.translate.to);
        if webview.evaluate_script(&translate::start_script()).is_err() {
            page.stage = Stage::Foreign;
        }
        self.push_translate(id);
        if !self.pump(id) {
            self.schedule_poll(id);
        }
    }

    /// Просит у страницы следующую порцию, если ограничения позволяют.
    fn pump(&mut self, id: TabId) -> bool {
        let now = Instant::now();
        if self.translate.running >= MAX_RUNNING
            || !self.translate.recent.allow(now, MAX_GLOBAL_PER_MINUTE)
        {
            return false;
        }
        let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) else {
            return false;
        };
        let Some(page) = self.translate.pages.get_mut(&id) else {
            return false;
        };
        if page.translating() && page.limiter.left() == 0 {
            let first = !std::mem::replace(&mut page.capped, true);
            let finished = page.stage == Stage::Busy && page.in_flight == 0;
            if finished {
                page.stage = Stage::Done;
            }
            if first {
                self.send(&ToUi::Toast {
                    text: "Страница слишком большая: переведена не целиком",
                });
            }
            if finished {
                self.push_translate(id);
            }
            return false;
        }
        if page.errors >= MAX_ERRORS {
            return false;
        }
        let Some(limit) = page.next_limit(now) else {
            return false;
        };
        let (epoch, events) = (page.epoch, self.events.clone());
        let asked = webview
            .evaluate_script_with_callback(&translate::take_script(limit), move |raw| {
                let _ = events.send_event(UserEvent::Translate(Event::Taken {
                    tab: id,
                    epoch,
                    raw,
                    limit,
                }));
            })
            .is_ok();
        if asked {
            page.asking = Some(now);
        }
        asked
    }

    fn on_taken(&mut self, id: TabId, epoch: u64, raw: &str, limit: usize) {
        let Some(page) = self.translate.current(id, epoch) else {
            return;
        };
        page.asking = None;
        let batch = Batch::parse(raw, limit);
        if batch.groups.is_empty() {
            // Переводить больше нечего: дальше — только новый текст на странице.
            let finished = page.stage == Stage::Busy && page.in_flight == 0;
            if finished {
                page.stage = Stage::Done;
            }
            self.schedule_poll(id);
            if finished {
                self.push_translate(id);
            }
            return;
        }
        let now = Instant::now();
        page.limiter.record(now, batch.chars);
        page.in_flight += 1;
        let (to, events, more) = (page.to.clone(), self.events.clone(), batch.more);
        self.translate.running += 1;
        self.translate.recent.record(now);
        std::thread::spawn(move || {
            let result = translate::google::translate(&to, &batch.groups);
            let _ = events.send_event(UserEvent::Translate(Event::Translated {
                tab: id,
                epoch,
                result,
            }));
        });
        if more {
            self.pump(id);
        }
    }

    fn on_translated(&mut self, id: TabId, epoch: u64, result: Result<Output, String>) {
        self.translate.running = self.translate.running.saturating_sub(1);
        let Some(page) = self.translate.current(id, epoch) else {
            return;
        };
        page.in_flight = page.in_flight.saturating_sub(1);
        let idle = page.in_flight == 0;
        let out = match result {
            Ok(out) => out,
            Err(err) => {
                page.errors += 1;
                if page.first && page.stage == Stage::Busy {
                    // Ничего ещё не перевели: возвращаем как было и говорим почему.
                    self.revert_page(id, Stage::Foreign);
                    let text = format!("Не удалось перевести: {err}");
                    self.send(&ToUi::Toast { text: &text });
                } else if page.errors >= MAX_ERRORS {
                    let busy = page.stage == Stage::Busy;
                    page.stage = Stage::Done;
                    if busy {
                        self.push_translate(id);
                    }
                } else {
                    self.schedule_poll(id);
                }
                return;
            }
        };
        page.errors = 0;
        let first = std::mem::replace(&mut page.first, false);
        if first && let Some(found) = out.lang {
            if lang::same(&found, &page.to) {
                // Google вернул текст как есть: страница уже на нужном языке.
                page.lang = found;
                self.revert_page(id, Stage::Native);
                self.send(&ToUi::Toast {
                    text: "Страница уже на этом языке",
                });
                return;
            }
            if page.lang != found {
                page.lang = found;
                self.push_translate(id);
            }
        }
        if let Some(js) = translate::apply_script(&out.items) {
            self.eval(id, &js);
        }
        if !self.pump(id) && idle {
            self.schedule_poll(id);
        }
    }

    fn on_poll(&mut self, id: TabId, epoch: u64) {
        let active = self.tabs.active == Some(id);
        let alive = self.tabs.get(id).is_some_and(|t| t.webview.is_some());
        let Some(page) = self.translate.current(id, epoch) else {
            return;
        };
        page.polling = false;
        let (stage, errors, idle) = (page.stage, page.errors, page.in_flight == 0);
        // Вкладка уснула: после пробуждения страница загрузится и переведётся заново.
        if !alive {
            return;
        }
        match stage {
            Stage::Probing => self.probe(id),
            Stage::Busy | Stage::Done if errors < MAX_ERRORS => {
                // Фоновую переведённую вкладку не дёргаем: новый текст переведём, когда её откроют.
                if !active && stage == Stage::Done {
                    self.schedule_poll(id);
                    return;
                }
                if !self.pump(id) && idle {
                    self.schedule_poll(id);
                }
            }
            _ => {}
        }
    }

    fn schedule_poll(&mut self, id: TabId) {
        let Some(page) = self.translate.pages.get_mut(&id) else {
            return;
        };
        if std::mem::replace(&mut page.polling, true) {
            return;
        }
        let (epoch, events) = (page.epoch, self.events.clone());
        std::thread::spawn(move || {
            std::thread::sleep(translate::POLL);
            let _ = events.send_event(UserEvent::Translate(Event::Poll { tab: id, epoch }));
        });
    }

    /// Вернуть странице оригинал. Новое поколение: запоздалые ответы отбросятся.
    fn revert_page(&mut self, id: TabId, stage: Stage) {
        let epoch = self.translate.epoch();
        if let Some(page) = self.translate.pages.get_mut(&id) {
            page.stop(epoch, stage);
        }
        self.eval(id, translate::REVERT);
        self.push_translate(id);
    }

    fn eval(&self, id: TabId, js: &str) {
        if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
            let _ = webview.evaluate_script(js);
        }
    }

    fn push_translate(&self, id: TabId) {
        let Some(page) = self.translate.pages.get(&id) else {
            return;
        };
        let prefs = &self.settings.translate;
        let to = if page.translating() {
            &page.to
        } else {
            &prefs.to
        };
        self.send(&ToUi::Translate(View {
            id,
            phase: page.phase(prefs),
            lang: &page.lang,
            to,
        }));
    }
}
