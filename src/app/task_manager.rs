//! Диспетчер задач (`axiom://tasks`, Shift+Esc). Своего webview у вкладки нет:
//! страницу рисует интерфейс (`ui/tasks.js`) и раз в секунду, пока она на экране,
//! просит свежий замер. Тот же замер нужен карточке вкладки под курсором (память
//! вкладки). Здесь — замер и кнопки «Усыпить».

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use super::App;
use crate::{
    events::UserEvent,
    ipc::ToUi,
    tabs::{TASKS, TASKS_TITLE, TabId},
    taskman::{Owner, Sampler, engine},
};

/// Движок не ответил за это время — спрашиваем заново.
const ANSWER_LIMIT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct State {
    sampler: Sampler,
    /// Номер последнего замера: ответы на прежние не считаются.
    serial: u64,
    waiting: Option<Waiting>,
    /// Вкладки, которые человек усыпил сам: их не держат «не усыплять» и звук.
    pub(super) manual_sleep: HashSet<TabId>,
}

struct Waiting {
    at: Instant,
    /// Сколько окружений ещё не ответило.
    left: usize,
    got: Vec<(u32, Vec<engine::EngineProc>)>,
}

impl App {
    /// Shift+Esc и пункт меню: диспетчер один — есть, так переходим на него,
    /// нет — открываем рядом с активной. Shift+Esc на самом диспетчере закрывает
    /// его и возвращает туда, откуда открыли.
    pub(super) fn toggle_tasks(&mut self) {
        match self.tabs.tasks() {
            Some(id) if self.tabs.active == Some(id) => self.close_tab_with(id, false),
            _ => self.open_tasks(),
        }
    }

    pub(super) fn open_tasks(&mut self) {
        if let Some(id) = self.tabs.tasks() {
            self.activate(id);
            return;
        }
        let from = self.tabs.active;
        let id = self.tabs.add(TASKS.into(), TASKS_TITLE.into(), false, None);
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.opener = from;
        }
        self.activate(id);
    }

    /// Интерфейс просит замер: диспетчер на экране или карточка вкладки под курсором.
    /// Сначала движок говорит, чьи процессы (асинхронно, по окружению на каждый
    /// главный процесс), потом — память и процессор у системы.
    pub(super) fn tasks_request(&mut self) {
        if self
            .tasks
            .waiting
            .as_ref()
            .is_some_and(|w| w.at.elapsed() < ANSWER_LIMIT)
        {
            return;
        }
        self.tasks.serial += 1;
        let serial = self.tasks.serial;
        // По одному webview на окружение: у панели ИИ с прокси оно своё.
        let mut asked: Vec<u32> = Vec::new();
        for webview in self.live_webviews().map(|(_, w)| w) {
            let Some(browser) = engine::browser_pid(webview) else {
                continue;
            };
            if asked.contains(&browser) {
                continue;
            }
            let events = self.events.clone();
            let sent = engine::query(webview, move |procs| {
                let _ = events.send_event(UserEvent::TaskProcs {
                    serial,
                    browser,
                    procs,
                });
            });
            if sent {
                asked.push(browser);
            }
        }
        if asked.is_empty() {
            self.tasks.waiting = None;
            self.tasks_finish(Vec::new());
            return;
        }
        self.tasks.waiting = Some(Waiting {
            at: Instant::now(),
            left: asked.len(),
            got: Vec::new(),
        });
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn on_task_procs(
        &mut self,
        serial: u64,
        browser: u32,
        procs: Option<Vec<engine::EngineProc>>,
    ) {
        if serial != self.tasks.serial {
            return;
        }
        let Some(waiting) = self.tasks.waiting.as_mut() else {
            return;
        };
        if let Some(procs) = procs {
            waiting.got.push((browser, procs));
        }
        waiting.left = waiting.left.saturating_sub(1);
        if waiting.left > 0 {
            return;
        }
        let got = self.tasks.waiting.take().map(|w| w.got).unwrap_or_default();
        self.tasks_finish(got);
    }

    fn tasks_finish(&mut self, engine: Vec<(u32, Vec<engine::EngineProc>)>) {
        // Чья страница в каком главном фрейме: вкладки, интерфейс, панель ИИ.
        let mut owners: HashMap<(u32, u32), Owner> = HashMap::new();
        for (owner, webview) in self.live_webviews() {
            if let (Some(browser), Some(frame)) =
                (engine::browser_pid(webview), engine::main_frame(webview))
            {
                owners.insert((browser, frame), owner);
            }
        }
        let tree = crate::taskman::os::tree(std::process::id());
        if tree.is_empty() {
            return;
        }
        let mut view = self
            .tasks
            .sampler
            .measure(&tree, &engine, &owners, Instant::now());
        if let Some((free, total)) = crate::system::memory::ram() {
            view.ram_free = free;
            view.ram_total = total;
        }
        self.send(&ToUi::Tasks(view));
    }

    /// Webview, у которых есть страница: интерфейс, панель ИИ и вкладки.
    fn live_webviews(&self) -> impl Iterator<Item = (Owner, &wry::WebView)> {
        std::iter::once((Owner::Ui, &self.ui))
            .chain(self.ai.webview.iter().map(|w| (Owner::Ai, w)))
            .chain(
                self.tabs
                    .iter()
                    .filter_map(|t| Some((Owner::Tab(t.id), t.webview.as_ref()?))),
            )
    }

    /// «Усыпить» у вкладки: человек просит сам — «не усыплять» и звук не мешают.
    /// Не усыпляем только скачивающую (загрузка оборвётся) и с недописанным текстом.
    pub(super) fn tasks_sleep(&mut self, id: TabId) {
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        if tab.webview.is_none() || tab.is_internal() || self.tabs.active == Some(id) {
            return;
        }
        if self.downloads.is_busy(id) {
            self.send(&ToUi::Toast {
                text: "Во вкладке идёт скачивание — она не уснёт, пока оно не закончится",
            });
            return;
        }
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.sleep_requested = None;
            tab.keep_alive = false;
        }
        self.tasks.manual_sleep.insert(id);
        self.request_sleep(id);
    }

    /// «Усыпить фоновые»: те же вкладки, что усыпил бы таймер (без звука,
    /// без «не усыплять», без скачиваний).
    pub(super) fn tasks_sleep_all(&mut self) {
        let ids: Vec<TabId> = self
            .sleep_candidates()
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        if ids.is_empty() {
            self.send(&ToUi::Toast {
                text: "Усыплять нечего: фоновые вкладки уже спят или их нельзя усыпить",
            });
            return;
        }
        for &id in &ids {
            self.tasks.manual_sleep.insert(id);
            self.request_sleep(id);
        }
    }
}
