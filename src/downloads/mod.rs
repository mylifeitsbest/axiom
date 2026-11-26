//! Загрузки файлов: список с прогрессом и действия «открыть», «показать в папке».
//!
//! Файлы сохраняются в системную папку «Загрузки». WebView2 и WebKitGTK
//! присылают события загрузки в колбэках, те превращаются в [`DownloadEvent`]
//! и попадают в главный цикл. Список последних загрузок хранится в профиле.
//!
//! Путь движку не доверяем: файл ложится только в «Загрузки» (см. [`place`]),
//! а файл опасного типа на Linux до решения человека лежит под временным
//! именем `….unconfirmed`, которое не запустить двойным щелчком.

#[cfg(target_os = "linux")]
mod webkit;
#[cfg(windows)]
mod webview2;

use std::{
    ffi::OsString,
    io::{Read, Seek, SeekFrom},
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
    /// Файл уже на диске и ждёт решения, оставить его или удалить: на Windows —
    /// безобидное расширение, а внутри программа (`program`, см. [`looks_like_program`]),
    /// на Linux — файл опасного типа под временным именем (см. [`place`]).
    Disguised {
        id: DownloadId,
        path: PathBuf,
        received: u64,
        program: bool,
    },
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
    /// Файл уже скачан и ждёт решения: программа под видом обычного файла
    /// или файл опасного типа под временным именем.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disguised: bool,
    /// Ждёт решения именно программа под видом обычного файла.
    #[serde(skip)]
    pub program: bool,
}

impl Download {
    /// Имя, под которым файл останется: без временного `.unconfirmed`.
    pub fn name(&self) -> String {
        confirmed(&self.path)
            .as_deref()
            .unwrap_or(&self.path)
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
            // Решения о файле не дождались до выхода. Программа под видом обычного
            // файла скачана целиком и остаётся, а файл опасного типа никто не
            // разрешил — его временную копию убираем.
            if item.status == Status::Active && item.disguised {
                item.disguised = false;
                if confirmed(&item.path).is_some() {
                    let _ = std::fs::remove_file(&item.path);
                    item.status = Status::Cancelled;
                    item.error = "Удалено: сохранить не разрешили".into();
                } else {
                    item.status = Status::Done;
                }
            }
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
                        disguised: false,
                        program: false,
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
            DownloadEvent::Disguised {
                id,
                path,
                received,
                program,
            } => {
                let item = self.active_mut(id)?;
                if !path.as_os_str().is_empty() {
                    item.path = path;
                }
                item.received = received;
                if item.total.is_none_or(|t| t < received) {
                    item.total = Some(received);
                }
                item.ask = true;
                item.disguised = true;
                item.program = program;
                self.dirty = true;
                self.get(id)
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
                item.disguised = false;
                item.program = false;
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

    /// «Сохранить всё равно» или «Удалить» для уже скачанного файла, который ждёт
    /// решения (см. [`DownloadEvent::Disguised`]). Решаем сами, без движка.
    /// Файл под временным именем при «Сохранить» получает своё. `None` — эта
    /// загрузка такого решения не ждёт.
    pub fn answer_disguised(&self, id: DownloadId, keep: bool) -> Option<DownloadEvent> {
        let item = self
            .items
            .iter()
            .find(|d| d.id == id && d.status == Status::Active && d.disguised)?;
        let mut path = None;
        let outcome = if keep {
            match confirmed(&item.path) {
                None => Outcome::Done,
                Some(target) => {
                    let target = unique_path(&target);
                    if std::fs::rename(&item.path, &target).is_ok() {
                        path = Some(target);
                        Outcome::Done
                    } else {
                        Outcome::Failed("Не удалось сохранить файл".into())
                    }
                }
            }
        } else if std::fs::remove_file(&item.path).is_ok() || !item.path.exists() {
            Outcome::Discarded
        } else {
            Outcome::Failed("Не удалось удалить файл".into())
        };
        Some(DownloadEvent::Finished {
            id,
            outcome,
            path,
            received: None,
        })
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

    /// Идёт загрузка, у которой неизвестна вкладка (панель ИИ, Linux). Файл,
    /// который уже скачан и ждёт решения, не в счёт.
    pub fn busy_untracked(&self) -> bool {
        self.items
            .iter()
            .any(|d| d.status == Status::Active && !d.disguised && d.tab.is_none())
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

/// Начало файла — программа: exe или dll Windows (`MZ` и заголовок `PE`),
/// программа Linux (ELF) или скрипт со строкой `#!/…`. Так видно `setup.exe`,
/// переименованный в `setup.txt` или `photo.jpg`, — по имени его не узнать.
/// Спрашиваем об этом только на Windows: на Linux у скачанного файла нет права
/// на запуск, а о типах файлов WebKitGTK не спрашивает вовсе.
///
/// Заголовок PE дальше `head` здесь не виден — его дочитывает [`program_inside`].
#[cfg_attr(not(windows), allow(dead_code))]
pub fn looks_like_program(head: &[u8]) -> bool {
    if head.starts_with(b"\x7fELF") {
        return true;
    }
    if let Some(rest) = head.strip_prefix(b"#!") {
        return rest
            .iter()
            .find(|b| !matches!(b, b' ' | b'\t'))
            .is_some_and(|&b| b == b'/');
    }
    pe_offset(head)
        .and_then(|offset| usize::try_from(offset).ok())
        .and_then(|offset| head.get(offset..offset.checked_add(4)?))
        .is_some_and(|sig| sig == PE_SIGNATURE)
}

#[cfg_attr(not(windows), allow(dead_code))]
const PE_SIGNATURE: &[u8; 4] = b"PE\0\0";

/// `MZ` в начале — смещение заголовка PE из поля e_lfanew старого заголовка DOS.
#[cfg_attr(not(windows), allow(dead_code))]
fn pe_offset(head: &[u8]) -> Option<u64> {
    if !head.starts_with(b"MZ") {
        return None;
    }
    let field: [u8; 4] = head.get(0x3C..0x40)?.try_into().ok()?;
    Some(u64::from(u32::from_le_bytes(field)))
}

/// Сколько байт начала файла читать для [`looks_like_program`]: заголовок PE
/// у обычных программ — в первых сотнях байт.
#[cfg_attr(not(windows), allow(dead_code))]
const HEAD: u64 = 4096;

/// Дальше этого заголовок PE не ищем. Windows читает его по 32-битному смещению
/// e_lfanew, то есть где угодно в файле: длинная заглушка DOS — и заголовок
/// уже за первыми 4 КБ. У настоящих программ он гораздо ближе; 16 МБ — с запасом,
/// а читаем оттуда всё равно только 4 байта.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_PE_OFFSET: u64 = 16 << 20;

/// Скачанный файл — программа (см. [`looks_like_program`]).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn program_inside(path: &Path) -> bool {
    std::fs::File::open(path).is_ok_and(program_in)
}

/// [`program_inside`] для любого источника с перемоткой.
#[cfg_attr(not(windows), allow(dead_code))]
fn program_in<R: Read + Seek>(mut file: R) -> bool {
    let mut head = Vec::new();
    if file.by_ref().take(HEAD).read_to_end(&mut head).is_err() {
        return false;
    }
    if looks_like_program(&head) {
        return true;
    }
    // Заголовок PE не поместился в прочитанное начало: дочитываем 4 байта по смещению.
    let Some(offset) = pe_offset(&head) else {
        return false;
    };
    if offset.saturating_add(4) <= head.len() as u64 || offset > MAX_PE_OFFSET {
        return false;
    }
    let mut sig = [0u8; 4];
    file.seek(SeekFrom::Start(offset)).is_ok()
        && file.read_exact(&mut sig).is_ok()
        && &sig == PE_SIGNATURE
}

/// Временное окончание файла опасного типа, пока человек не разрешил его
/// сохранить: под ним файл не откроется и не запустится двойным щелчком.
const UNCONFIRMED: &str = "unconfirmed";

/// Опасные на Linux типы файлов: ярлыки запуска `.desktop` (в них любая команда),
/// скрипты оболочек, программы и установщики, образы дисков (их подключают
/// двойным щелчком) и программы Windows — их запускает Wine или переносят на Windows.
#[cfg_attr(windows, allow(dead_code))]
const LINUX_RISKY: &str = "\
    appimage bash bat cmd com csh deb desktop dmg exe fish flatpak flatpakref flatpakrepo \
    img iso jar ksh lnk msi msix pif ps1 reg rpm run scr sh snap vbs vhd vhdx zsh";

/// Длиннее имя файла (в байтах) не делаем: в ext4 и большинстве систем предел — 255.
#[cfg_attr(windows, allow(dead_code))]
const MAX_NAME: usize = 200;

/// Расширение файла есть в списке `types` (через пробел, без точек).
pub fn has_type(path: &Path, types: &str) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    types
        .split_ascii_whitespace()
        .any(|t| t.eq_ignore_ascii_case(ext))
}

/// Куда положить файл, для которого движок предложил путь `suggested` (Linux).
///
/// WebKitGTK сообщает только имя, а wry, если в системе не настроена папка
/// «Загрузки» (`user-dirs.dirs`), клал файл в текущую папку процесса — домашнюю
/// или папку проекта, откуда запустили браузер. Поэтому место выбираем сами:
/// всегда в `dir`, от предложенного пути — только имя, без управляющих символов
/// и точки в начале (если «Загрузки» — домашняя папка, `.profile` не ляжет рядом
/// с настройками), занятое имя — с номером. Файл опасного типа до решения
/// человека лежит как `имя.unconfirmed` (см. [`confirmed`]).
#[cfg_attr(windows, allow(dead_code))]
pub fn place(dir: &Path, suggested: &Path) -> PathBuf {
    let name = clean_name(suggested);
    let held = has_type(Path::new(&name), LINUX_RISKY);
    let named = Path::new(&name);
    let stem = named
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = named
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let taken = |path: &Path| std::fs::symlink_metadata(path).is_ok();
    let free = (0..10_000)
        .map(|n| match n {
            0 => dir.join(&name),
            n => dir.join(format!("{stem} ({n}){ext}")),
        })
        .find(|path| !taken(path) && !(held && taken(&unconfirmed(path))))
        .unwrap_or_else(|| dir.join(&name));
    if held { unconfirmed(&free) } else { free }
}

/// Имя файла из предложенного пути: без папок, управляющих символов, точек
/// и пробелов по краям; длинное — укорочено с сохранением расширения.
#[cfg_attr(windows, allow(dead_code))]
fn clean_name(suggested: &Path) -> String {
    let raw = suggested
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let name = name
        .trim()
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    if name.is_empty() {
        return "download".into();
    }
    if name.len() <= MAX_NAME {
        return name.into();
    }
    let ext = Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .filter(|e| e.len() <= 16)
        .unwrap_or_default();
    let mut end = MAX_NAME - ext.len();
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ext}", &name[..end])
}

/// `файл.desktop` → `файл.desktop.unconfirmed`.
#[cfg_attr(windows, allow(dead_code))]
fn unconfirmed(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(UNCONFIRMED);
    PathBuf::from(name)
}

/// Файл под временным именем (см. [`place`]) → имя, которое он получит, когда
/// человек разрешит его сохранить. `None` — у файла уже своё имя.
pub fn confirmed(path: &Path) -> Option<PathBuf> {
    path.extension()
        .is_some_and(|ext| ext == UNCONFIRMED)
        .then(|| path.with_extension(""))
}

/// `файл.zip` → `файл (1).zip`, если такой файл уже есть.
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
    fn recognizes_programs() {
        // Настоящая программа Windows: MZ, e_lfanew = 0x80, там PE\0\0.
        let mut pe = vec![0u8; 0x100];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3C] = 0x80;
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        assert!(looks_like_program(&pe));
        // Сам exe тестов — тоже программа.
        let me = std::env::current_exe().unwrap_or_default();
        assert!(program_inside(&me));
        assert!(looks_like_program(b"\x7fELF\x02\x01\x01"));
        assert!(looks_like_program(b"#!/bin/sh\necho hi"));
        assert!(looks_like_program(b"#! /usr/bin/env python3\n"));

        // Текст, который начинается с MZ, и битые заголовки — нет.
        assert!(!looks_like_program(b"MZ - inventory notes"));
        let mut far = pe.clone();
        far[0x3C] = 0xFF;
        far[0x3D] = 0xFF;
        far[0x3E] = 0xFF;
        far[0x3F] = 0xFF;
        assert!(!looks_like_program(&far));
        assert!(!looks_like_program(b"#!important"));
        assert!(!looks_like_program(b"%PDF-1.7"));
        assert!(!looks_like_program(b"PK\x03\x04"));
        assert!(!looks_like_program(b""));
        assert!(!program_inside(Path::new("нет такого файла")));
    }

    /// Файл `len` байт: `MZ`, e_lfanew = `offset` и подпись PE там, если влезает.
    fn pe_at(offset: u32, len: usize) -> Vec<u8> {
        let mut file = vec![0u8; len];
        file[..2].copy_from_slice(b"MZ");
        file[0x3C..0x40].copy_from_slice(&offset.to_le_bytes());
        let at = offset as usize;
        if let Some(sig) = file.get_mut(at..at.saturating_add(4)) {
            sig.copy_from_slice(b"PE\0\0");
        }
        file
    }

    #[test]
    fn finds_pe_header_past_head() {
        use std::io::Cursor;
        // Заголовок в начале, на границе прочитанного и далеко за ним (длинная заглушка DOS).
        for offset in [0x80, 0xFFC, 0xFFE, 0x1000, 0x2000, 0x10_0000] {
            let file = pe_at(offset, offset as usize + 0x200);
            assert!(program_in(Cursor::new(&file)), "{offset:#x}");
        }
        // Ровно на пределе ещё ищем, дальше — уже нет.
        let edge = MAX_PE_OFFSET as u32;
        assert!(program_in(Cursor::new(pe_at(edge, edge as usize + 4))));
        assert!(!program_in(Cursor::new(pe_at(edge + 1, edge as usize + 5))));
        // Смещение 0xFFFFFFFF не переполняет ничего.
        assert!(!program_in(Cursor::new(pe_at(u32::MAX, 0x100))));

        // Смещение за концом файла, не та подпись, обрыв на середине подписи.
        assert!(!program_in(Cursor::new(pe_at(0x2000, 0x1800))));
        let mut wrong = pe_at(0x2000, 0x2200);
        wrong[0x2000..0x2004].copy_from_slice(b"NE\0\0");
        assert!(!program_in(Cursor::new(&wrong)));
        let cut = pe_at(0x2000, 0x2200);
        assert!(!program_in(Cursor::new(&cut[..0x2002])));
        // Без `MZ` смещение не читаем вовсе; ELF и скрипты — как раньше.
        let mut text = pe_at(0x2000, 0x2200);
        text[..2].copy_from_slice(b"MX");
        assert!(!program_in(Cursor::new(&text)));
        assert!(program_in(Cursor::new(b"\x7fELF\x02\x01\x01")));
        assert!(program_in(Cursor::new(b"#!/bin/sh\n")));
        assert!(!program_in(Cursor::new(b"MZ")));

        // И с диска: программа под видом картинки.
        let dir = std::env::temp_dir().join(format!("axiom-dl-pe-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("photo.jpg");
        let _ = std::fs::write(&file, pe_at(0x2000, 0x3000));
        let found = program_inside(&file);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found);
    }

    #[test]
    fn disguised_file_waits_for_answer() {
        let dir = std::env::temp_dir().join(format!("axiom-dl-disguised-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Downloads::default();
        let kept = started(&mut store, Some(5));
        let dropped = started(&mut store, Some(5));
        for id in [kept, dropped] {
            let file = dir.join(format!("{id}.txt"));
            let _ = std::fs::write(&file, b"MZ");
            let asked = store.apply(DownloadEvent::Disguised {
                id,
                path: file,
                received: 2,
                program: true,
            });
            assert!(asked.is_some_and(|d| d.ask && d.disguised && d.total == Some(2)));
        }
        // Пока ждёт решения, вкладку не усыпить.
        assert!(store.is_busy(5));
        // Обычная загрузка такого решения не ждёт.
        let plain = started(&mut store, None);
        assert!(store.answer_disguised(plain, true).is_none());

        let keep = store.answer_disguised(kept, true);
        let kept_item = keep.and_then(|e| store.apply(e).cloned());
        let drop = store.answer_disguised(dropped, false);
        let dropped_item = drop.and_then(|e| store.apply(e).cloned());
        let file_left = dir.join(format!("{dropped}.txt")).exists();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(kept_item.is_some_and(|d| d.status == Status::Done && !d.ask && !d.disguised));
        assert!(
            dropped_item.is_some_and(|d| d.status == Status::Cancelled && d.error == "Удалено")
        );
        assert!(!file_left, "файл не удалён");
        assert!(!store.is_busy(5));
    }

    #[test]
    fn places_files_in_downloads_only() {
        let dir = std::env::temp_dir().join(format!("axiom-dl-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        // Путь движка — только подсказка имени: папки и точки в начале отбрасываются.
        assert_eq!(place(&dir, Path::new("report.pdf")), dir.join("report.pdf"));
        assert_eq!(
            place(&dir, Path::new("/tmp/elsewhere/report.pdf")),
            dir.join("report.pdf")
        );
        assert_eq!(
            place(&dir, Path::new("../../.profile")),
            dir.join("profile")
        );
        assert_eq!(place(&dir, Path::new("...")), dir.join("download"));
        assert_eq!(place(&dir, Path::new("")), dir.join("download"));
        assert_eq!(place(&dir, Path::new("a\u{1}b.txt")), dir.join("a_b.txt"));
        let long = place(&dir, Path::new(&format!("{}.pdf", "я".repeat(300))));
        let name = long.file_name().map(|n| n.len()).unwrap_or(0);
        assert!(name <= MAX_NAME && long.extension().is_some_and(|e| e == "pdf"));
        // Опасный тип — под временным именем, которое ничего не запускает.
        let held = place(&dir, Path::new("pwn.desktop"));
        assert_eq!(held, dir.join("pwn.desktop.unconfirmed"));
        assert_eq!(confirmed(&held), Some(dir.join("pwn.desktop")));
        assert_eq!(confirmed(&dir.join("report.pdf")), None);
        assert!(place(&dir, Path::new("SETUP.SH")).ends_with("SETUP.SH.unconfirmed"));
        // Занятое имя (и занятое временное) — с номером.
        let _ = std::fs::write(&held, b"x");
        assert_eq!(
            place(&dir, Path::new("pwn.desktop")),
            dir.join("pwn (1).desktop.unconfirmed")
        );
        let _ = std::fs::write(dir.join("report.pdf"), b"x");
        assert_eq!(
            place(&dir, Path::new("report.pdf")),
            dir.join("report (1).pdf")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn held_file_gets_its_name_when_kept() {
        let dir = std::env::temp_dir().join(format!("axiom-dl-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Downloads::default();
        let kept = started(&mut store, None);
        let dropped = started(&mut store, None);
        let mut files = Vec::new();
        for (id, name) in [(kept, "a.sh"), (dropped, "b.desktop")] {
            let file = place(&dir, Path::new(name));
            let _ = std::fs::write(&file, b"#!/bin/sh");
            let asked = store.apply(DownloadEvent::Disguised {
                id,
                path: file.clone(),
                received: 9,
                program: false,
            });
            assert!(asked.is_some_and(|d| d.ask && d.disguised && !d.program && d.name() == name));
            files.push(file);
        }
        // Файл уже скачан: панель ИИ из-за него не держим.
        assert!(!store.busy_untracked());
        let keep = store.answer_disguised(kept, true);
        let kept_item = keep.and_then(|e| store.apply(e).cloned());
        let drop = store.answer_disguised(dropped, false);
        let dropped_item = drop.and_then(|e| store.apply(e).cloned());
        let left: Vec<bool> = files.iter().map(|f| f.exists()).collect();
        let named = dir.join("a.sh").exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(kept_item.is_some_and(|d| d.status == Status::Done && d.path == dir.join("a.sh")));
        assert!(dropped_item.is_some_and(|d| d.status == Status::Cancelled));
        assert_eq!(left, [false, false]);
        assert!(named, "сохранённый файл не получил своё имя");
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
