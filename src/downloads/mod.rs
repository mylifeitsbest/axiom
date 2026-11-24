//! Загрузки файлов: список с прогрессом и действия «открыть», «показать в папке».
//!
//! Файлы сохраняются в системную папку «Загрузки». WebView2 и WebKitGTK
//! присылают события загрузки в колбэках, те превращаются в [`DownloadEvent`]
//! и попадают в главный цикл. Список последних загрузок хранится в профиле.

#[cfg(target_os = "linux")]
mod webkit;
#[cfg(windows)]
mod webview2;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
pub use self::webkit::attach;
#[cfg(windows)]
pub use self::webview2::{cancel, install, keep};
use crate::{
    storage::{Paths, now_secs, read_json, write_json},
    tabs::TabId,
};

const FILE: &str = "downloads.json";
/// Сколько последних загрузок помнить.
const MAX_ITEMS: usize = 100;
/// Длинные адреса (особенно `data:`) не храним целиком.
const MAX_URL: usize = 2048;

/// Отменить загрузку умеет только WebView2: wry не отдаёт объект загрузки WebKitGTK.
pub const CAN_CANCEL: bool = cfg!(windows);

pub type DownloadId = u64;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_id() -> DownloadId {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Событие загрузки из колбэка webview.
#[derive(Debug)]
pub enum DownloadEvent {
    Started {
        id: DownloadId,
        url: String,
        path: PathBuf,
        total: Option<u64>,
        /// Вкладка, из которой начали загрузку (известна только на Windows).
        tab: Option<TabId>,
    },
    // Прогресс и отмену сейчас присылает только WebView2 (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Progress {
        id: DownloadId,
        received: u64,
        total: Option<u64>,
    },
    /// Файл опасного типа: WebView2 ждёт, сохранить его или удалить (`waiting`),
    /// или пользователь уже решил сохранить.
    #[cfg_attr(not(windows), allow(dead_code))]
    Decision { id: DownloadId, waiting: bool },
    Finished {
        id: DownloadId,
        outcome: Outcome,
        /// Итоговый путь, если он известен (WebView2 может его уточнить).
        path: Option<PathBuf>,
        received: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    #[cfg_attr(not(windows), allow(dead_code))]
    Cancelled,
    /// Пользователь отказался сохранять файл опасного типа.
    #[cfg_attr(not(windows), allow(dead_code))]
    Discarded,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Done,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    #[serde(skip)]
    pub id: DownloadId,
    pub url: String,
    pub path: PathBuf,
    #[serde(default)]
    pub received: u64,
    #[serde(default)]
    pub total: Option<u64>,
    pub status: Status,
    #[serde(default)]
    pub error: String,
    /// Когда началась загрузка (секунды Unix).
    #[serde(default)]
    pub at: u64,
    #[serde(skip)]
    pub tab: Option<TabId>,
    /// Ждёт решения: сохранить файл опасного типа или удалить.
    #[serde(skip)]
    pub ask: bool,
}

impl Download {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "файл".into())
    }
}

/// Список загрузок, новые — первыми.
#[derive(Default)]
pub struct Downloads {
    items: Vec<Download>,
    dirty: bool,
}

impl Downloads {
    pub fn load(paths: &Paths) -> Self {
        let mut items: Vec<Download> = read_json(&paths.root.join(FILE)).unwrap_or_default();
        items.truncate(MAX_ITEMS);
        for item in &mut items {
            item.id = next_id();
            // Браузер закрыли посреди загрузки: WebView2 её уже не продолжит.
            if item.status == Status::Active {
                item.status = Status::Failed;
                item.error = "Прервано: браузер был закрыт".into();
            }
        }
        Self {
            items,
            dirty: false,
        }
    }

    pub fn save_if_dirty(&mut self, paths: &Paths) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        write_json(&paths.root.join(FILE), &self.items)?;
        self.dirty = false;
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Download> {
        self.items.iter()
    }

    pub fn get(&self, id: DownloadId) -> Option<&Download> {
        self.items.iter().find(|d| d.id == id)
    }

    /// Применяет событие. Возвращает загрузку, если изменился её статус
    /// (началась или закончилась) — об этом стоит сказать пользователю.
    pub fn apply(&mut self, event: DownloadEvent) -> Option<&Download> {
        match event {
            DownloadEvent::Started {
                id,
                url,
                path,
                total,
                tab,
            } => {
                self.items.insert(
                    0,
                    Download {
                        id,
                        url: short_url(&url),
                        path,
                        received: 0,
                        total,
                        status: Status::Active,
                        error: String::new(),
                        at: now_secs(),
                        tab,
                        ask: false,
                    },
                );
                self.trim();
                self.dirty = true;
                self.items.first()
            }
            DownloadEvent::Progress {
                id,
                received,
                total,
            } => {
                let item = self.active_mut(id)?;
                item.received = received;
                if total.is_some() {
                    item.total = total;
                }
                None
            }
            DownloadEvent::Decision { id, waiting } => {
                let item = self.active_mut(id)?;
                item.ask = waiting;
                // О том, что нужно решение, стоит сказать; о самом решении — нет.
                if waiting { Some(&*item) } else { None }
            }
            DownloadEvent::Finished {
                id,
                outcome,
                path,
                received,
            } => {
                let item = self.active_mut(id)?;
                if let Some(path) = path.filter(|p| !p.as_os_str().is_empty()) {
                    item.path = path;
                }
                if let Some(received) = received {
                    item.received = received;
                }
                item.ask = false;
                match outcome {
                    Outcome::Done => {
                        item.status = Status::Done;
                        // Размер без Content-Length становится известен только в конце.
                        if item.total.is_none_or(|t| t < item.received) {
                            item.total = Some(item.received);
                        }
                    }
                    Outcome::Cancelled => item.status = Status::Cancelled,
                    Outcome::Discarded => {
                        item.status = Status::Cancelled;
                        item.error = "Удалено".into();
                    }
                    Outcome::Failed(reason) => {
                        item.status = Status::Failed;
                        item.error = reason;
                    }
                }
                let id = item.id;
                self.dirty = true;
                self.get(id)
            }
        }
    }

    fn active_mut(&mut self, id: DownloadId) -> Option<&mut Download> {
        self.items
            .iter_mut()
            .find(|d| d.id == id && d.status == Status::Active)
    }

    /// Убирает загрузку из списка (сам файл остаётся). Идущие загрузки не трогаем.
    pub fn remove(&mut self, id: DownloadId) -> bool {
        let before = self.items.len();
        self.items
            .retain(|d| d.id != id || d.status == Status::Active);
        let removed = self.items.len() != before;
        self.dirty |= removed;
        removed
    }

    /// Очищает список от завершённых загрузок.
    pub fn clear(&mut self) {
        let before = self.items.len();
        self.items.retain(|d| d.status == Status::Active);
        self.dirty |= self.items.len() != before;
    }

    /// Вкладка что-то скачивает — её нельзя усыплять: вместе с webview прервётся загрузка.
    pub fn is_busy(&self, tab: TabId) -> bool {
        self.items
            .iter()
            .any(|d| d.status == Status::Active && d.tab == Some(tab))
    }

    /// Идёт загрузка, у которой неизвестна вкладка (панель ИИ, Linux).
    pub fn busy_untracked(&self) -> bool {
        self.items
            .iter()
            .any(|d| d.status == Status::Active && d.tab.is_none())
    }

    fn trim(&mut self) {
        while self.items.len() > MAX_ITEMS {
            match self.items.iter().rposition(|d| d.status != Status::Active) {
                Some(i) => {
                    self.items.remove(i);
                }
                None => break,
            }
        }
    }
}

fn short_url(url: &str) -> String {
    if url.starts_with("data:") {
        return "data:".into();
    }
    if url.len() <= MAX_URL {
        return url.to_string();
    }
    let mut end = MAX_URL;
    while !url.is_char_boundary(end) {
        end -= 1;
    }
    url[..end].to_string()
}

/// `файл.zip` → `файл (1).zip`, если такой файл уже есть.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Системная папка «Загрузки».
pub fn folder() -> Option<PathBuf> {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(Path::to_path_buf))
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join("Downloads")))
}

/// Открывает файл программой по умолчанию.
pub fn open(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("файл удалён или перемещён");
    }
    #[cfg(windows)]
    let command = {
        let mut c = Command::new("explorer.exe");
        c.arg(path);
        c
    };
    #[cfg(not(windows))]
    let command = {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    spawn(command)
}

/// Показывает файл в проводнике (на Linux — открывает его папку).
pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("explorer.exe");
        if path.exists() {
            // Explorer ждёт кавычки после запятой; в путях Windows кавычек не бывает.
            let mut arg = OsString::from("/select,\"");
            arg.push(path);
            arg.push("\"");
            c.raw_arg(arg);
        } else {
            c.arg(existing_dir(path)?);
        }
        c
    };
    #[cfg(not(windows))]
    let command = {
        let mut c = Command::new("xdg-open");
        c.arg(existing_dir(path)?);
        c
    };
    spawn(command)
}

fn existing_dir(path: &Path) -> Result<OsString> {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().context("у файла нет папки")?
    };
    if !dir.is_dir() {
        bail!("папка {} не найдена", dir.display());
    }
    Ok(dir.as_os_str().to_owned())
}

/// Запускает программу и не ждёт её (на Linux ждём в фоне, чтобы не копить зомби).
fn spawn(mut command: Command) -> Result<()> {
    let mut child = command
        .spawn()
        .with_context(|| format!("не удалось запустить {:?}", command.get_program()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(not(windows))]
pub fn cancel(_id: DownloadId) -> bool {
    false
}

#[cfg(not(windows))]
pub fn keep(_id: DownloadId) {}

/// На Windows загрузки ловятся нативным событием после создания webview (см. [`install`]).
#[cfg(not(target_os = "linux"))]
pub fn attach<'a>(
    builder: wry::WebViewBuilder<'a>,
    _events: &tao::event_loop::EventLoopProxy<crate::events::UserEvent>,
) -> wry::WebViewBuilder<'a> {
    builder
}

#[cfg(not(windows))]
pub fn install(
    _webview: &wry::WebView,
    _tab: Option<TabId>,
    _events: tao::event_loop::EventLoopProxy<crate::events::UserEvent>,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(store: &mut Downloads, tab: Option<TabId>) -> DownloadId {
        let id = next_id();
        store.apply(DownloadEvent::Started {
            id,
            url: "https://example.com/a.zip".into(),
            path: PathBuf::from("a.zip"),
            total: None,
            tab,
        });
        id
    }

    #[test]
    fn lifecycle() {
        let mut store = Downloads::default();
        let id = started(&mut store, Some(7));
        assert!(store.is_busy(7));
        assert!(
            store
                .apply(DownloadEvent::Progress {
                    id,
                    received: 10,
                    total: Some(40),
                })
                .is_none()
        );
        // Идущую загрузку из списка не убрать.
        assert!(!store.remove(id));
        let done = store.apply(DownloadEvent::Finished {
            id,
            outcome: Outcome::Done,
            path: None,
            received: Some(40),
        });
        assert_eq!(done.map(|d| d.status), Some(Status::Done));
        assert!(!store.is_busy(7));
        // Повторное событие о завершении (WebKitGTK шлёт по одному на каждый webview) игнорируется.
        assert!(
            store
                .apply(DownloadEvent::Finished {
                    id,
                    outcome: Outcome::Cancelled,
                    path: None,
                    received: None,
                })
                .is_none()
        );
        assert!(store.remove(id));
        assert_eq!(store.iter().count(), 0);
    }

    #[test]
    fn unknown_size_becomes_known() {
        let mut store = Downloads::default();
        let id = started(&mut store, None);
        store.apply(DownloadEvent::Finished {
            id,
            outcome: Outcome::Done,
            path: Some(PathBuf::from("b.zip")),
            received: Some(1234),
        });
        let item = store.get(id);
        assert_eq!(item.and_then(|d| d.total), Some(1234));
        assert_eq!(item.map(Download::name).as_deref(), Some("b.zip"));
    }

    #[test]
    fn clear_keeps_active() {
        let mut store = Downloads::default();
        let a = started(&mut store, None);
        let b = started(&mut store, None);
        store.apply(DownloadEvent::Finished {
            id: a,
            outcome: Outcome::Failed("нет сети".into()),
            path: None,
            received: None,
        });
        store.clear();
        let left: Vec<_> = store.iter().map(|d| d.id).collect();
        assert_eq!(left, vec![b]);
    }

    #[test]
    fn dangerous_file_waits_for_decision() {
        let mut store = Downloads::default();
        let kept = started(&mut store, Some(3));
        let dropped = started(&mut store, Some(3));
        for id in [kept, dropped] {
            let asked = store.apply(DownloadEvent::Decision { id, waiting: true });
            assert!(asked.is_some_and(|d| d.ask && d.status == Status::Active));
        }
        // Пока ждёт решения, вкладку не усыпить, а загрузку не убрать из списка.
        assert!(store.is_busy(3));
        assert!(!store.remove(kept));
        // «Сохранить всё равно»: снова обычная загрузка, говорить об этом незачем.
        assert!(
            store
                .apply(DownloadEvent::Decision {
                    id: kept,
                    waiting: false
                })
                .is_none()
        );
        assert_eq!(store.get(kept).map(|d| d.ask), Some(false));
        // «Удалить»: отменена, с пометкой «Удалено».
        let gone = store.apply(DownloadEvent::Finished {
            id: dropped,
            outcome: Outcome::Discarded,
            path: None,
            received: None,
        });
        assert!(
            gone.is_some_and(|d| d.status == Status::Cancelled && !d.ask && d.error == "Удалено")
        );
    }

    #[test]
    fn short_urls() {
        assert_eq!(short_url("data:text/plain,hello"), "data:");
        let long = format!("https://e.com/{}", "я".repeat(MAX_URL));
        assert!(short_url(&long).len() <= MAX_URL);
    }

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("axiom-dl-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("report.pdf");
        assert_eq!(unique_path(&file), file);
        let _ = std::fs::write(&file, b"x");
        assert_eq!(unique_path(&file), dir.join("report (1).pdf"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
