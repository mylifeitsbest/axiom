//! Импорт закладок и истории из других браузеров.
//!
//! Chrome, Edge, Яндекс, Opera, Brave, Vivaldi и Chromium хранят данные одинаково
//! (это всё Chromium) — их читает [`chromium`]. Firefox и Zen — в [`firefox`].
//!
//! Пароли и куки не переносятся: браузеры шифруют их ключом, привязанным к себе
//! (в Chrome — app-bound encryption), и обходить эту защиту мы не будем.

mod chromium;
mod firefox;

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use super::{Paths, bookmarks::Bookmark, history::Entry};

/// Сколько последних страниц истории забираем из одного браузера.
const MAX_HISTORY: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Browser {
    Chrome,
    Edge,
    Yandex,
    Opera,
    OperaGx,
    Brave,
    Vivaldi,
    Chromium,
    Firefox,
    Zen,
}

impl Browser {
    pub const ALL: [Self; 10] = [
        Self::Chrome,
        Self::Edge,
        Self::Yandex,
        Self::Opera,
        Self::OperaGx,
        Self::Brave,
        Self::Vivaldi,
        Self::Chromium,
        Self::Firefox,
        Self::Zen,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Chrome => "Google Chrome",
            Self::Edge => "Microsoft Edge",
            Self::Yandex => "Яндекс Браузер",
            Self::Opera => "Opera",
            Self::OperaGx => "Opera GX",
            Self::Brave => "Brave",
            Self::Vivaldi => "Vivaldi",
            Self::Chromium => "Chromium",
            Self::Firefox => "Firefox",
            Self::Zen => "Zen",
        }
    }

    /// Для фраз «из …»: «Перенести из Яндекс Браузера?».
    pub fn from(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
            Self::Yandex => "Яндекс Браузера",
            other => other.name(),
        }
    }

    fn is_firefox(self) -> bool {
        matches!(self, Self::Firefox | Self::Zen)
    }

    /// Где браузер держит профили: у Chromium — папка `User Data`,
    /// у Firefox — папка с `profiles.ini`. Берётся первая, где нашёлся профиль.
    fn roots(self) -> Vec<PathBuf> {
        directories::BaseDirs::new().map_or_else(Vec::new, |base| roots(self, &base))
    }
}

#[cfg(windows)]
fn roots(browser: Browser, base: &directories::BaseDirs) -> Vec<PathBuf> {
    let local = base.data_local_dir();
    let roaming = base.data_dir();
    let user_data = |vendor: &[&str]| {
        let mut dir = local.to_path_buf();
        dir.extend(vendor);
        vec![dir.join("User Data")]
    };
    match browser {
        Browser::Chrome => user_data(&["Google", "Chrome"]),
        Browser::Edge => user_data(&["Microsoft", "Edge"]),
        Browser::Yandex => user_data(&["Yandex", "YandexBrowser"]),
        Browser::Brave => user_data(&["BraveSoftware", "Brave-Browser"]),
        Browser::Vivaldi => user_data(&["Vivaldi"]),
        Browser::Chromium => user_data(&["Chromium"]),
        Browser::Opera => vec![roaming.join("Opera Software").join("Opera Stable")],
        Browser::OperaGx => vec![roaming.join("Opera Software").join("Opera GX Stable")],
        Browser::Firefox => {
            let mut dirs = vec![roaming.join("Mozilla").join("Firefox")];
            // Firefox из Microsoft Store живёт в папке пакета.
            if let Ok(packages) = fs::read_dir(local.join("Packages")) {
                dirs.extend(
                    packages
                        .flatten()
                        .filter(|e| {
                            e.file_name()
                                .to_string_lossy()
                                .starts_with("Mozilla.Firefox_")
                        })
                        .map(|e| e.path().join(r"LocalCache\Roaming\Mozilla\Firefox")),
                );
            }
            dirs
        }
        Browser::Zen => vec![roaming.join("zen")],
    }
}

#[cfg(target_os = "macos")]
fn roots(browser: Browser, base: &directories::BaseDirs) -> Vec<PathBuf> {
    // ~/Library/Application Support
    let support = base.data_dir();
    let dir = match browser {
        Browser::Chrome => "Google/Chrome",
        Browser::Edge => "Microsoft Edge",
        Browser::Yandex => "Yandex/YandexBrowser",
        Browser::Brave => "BraveSoftware/Brave-Browser",
        Browser::Vivaldi => "Vivaldi",
        Browser::Chromium => "Chromium",
        Browser::Opera => "com.operasoftware.Opera",
        Browser::OperaGx => "com.operasoftware.OperaGX",
        Browser::Firefox => "Firefox",
        Browser::Zen => "zen",
    };
    vec![support.join(dir)]
}

/// Linux и прочие Unix: обычная установка, snap и flatpak.
#[cfg(not(any(windows, target_os = "macos")))]
fn roots(browser: Browser, base: &directories::BaseDirs) -> Vec<PathBuf> {
    let config = base.config_dir();
    let home = base.home_dir();
    let flatpak = |app: &str, rest: &str| home.join(".var/app").join(app).join(rest);
    match browser {
        Browser::Chrome => vec![
            config.join("google-chrome"),
            flatpak("com.google.Chrome", "config/google-chrome"),
        ],
        Browser::Edge => vec![
            config.join("microsoft-edge"),
            flatpak("com.microsoft.Edge", "config/microsoft-edge"),
        ],
        Browser::Yandex => vec![config.join("yandex-browser")],
        Browser::Brave => vec![
            config.join("BraveSoftware/Brave-Browser"),
            flatpak("com.brave.Browser", "config/BraveSoftware/Brave-Browser"),
        ],
        Browser::Vivaldi => vec![config.join("vivaldi")],
        Browser::Chromium => vec![
            config.join("chromium"),
            home.join("snap/chromium/common/chromium"),
            flatpak("org.chromium.Chromium", "config/chromium"),
        ],
        Browser::Opera => vec![config.join("opera")],
        Browser::OperaGx => Vec::new(),
        Browser::Firefox => vec![
            home.join(".mozilla/firefox"),
            home.join("snap/firefox/common/.mozilla/firefox"),
            flatpak("org.mozilla.firefox", ".mozilla/firefox"),
        ],
        Browser::Zen => vec![home.join(".zen"), flatpak("app.zen_browser.zen", ".zen")],
    }
}

/// Найденный профиль браузера.
#[derive(Debug, Clone)]
struct Profile {
    dir: PathBuf,
    /// Имя профиля, если у браузера их несколько; иначе пусто.
    name: String,
}

impl Profile {
    /// Когда браузером пользовались в последний раз: по времени записи истории.
    fn used(&self, browser: Browser) -> SystemTime {
        let files: &[&str] = if browser.is_firefox() {
            &["places.sqlite-wal", "places.sqlite"]
        } else {
            &["History", "Bookmarks"]
        };
        files
            .iter()
            .filter_map(|f| {
                fs::metadata(self.dir.join(f))
                    .and_then(|m| m.modified())
                    .ok()
            })
            .max()
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }
}

fn find_profile(browser: Browser) -> Option<Profile> {
    browser.roots().into_iter().find_map(|root| {
        if !root.is_dir() {
            return None;
        }
        if browser.is_firefox() {
            firefox::profile(&root)
        } else {
            chromium::profile(&root)
        }
    })
}

/// Что показать в интерфейсе: какие браузеры есть на компьютере.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub id: Browser,
    pub name: &'static str,
    pub from: &'static str,
    pub profile: String,
}

/// Браузеры с профилем на этом компьютере, сначала те, где сидели недавно.
pub fn detect() -> Vec<Found> {
    let mut found: Vec<(SystemTime, Found)> = Browser::ALL
        .into_iter()
        .filter_map(|browser| {
            let profile = find_profile(browser)?;
            Some((
                profile.used(browser),
                Found {
                    id: browser,
                    name: browser.name(),
                    from: browser.from(),
                    profile: profile.name,
                },
            ))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, f)| f).collect()
}

#[derive(Debug)]
pub struct Imported {
    pub profile: String,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<Entry>,
}

pub fn import(browser: Browser, paths: &Paths) -> Result<Imported> {
    let profile =
        find_profile(browser).ok_or_else(|| anyhow!("не найден профиль {}", browser.from()))?;
    let (bookmarks, history) = if browser.is_firefox() {
        firefox::read(&profile.dir, paths, browser)?
    } else {
        chromium::read(&profile.dir, paths, browser)?
    };
    Ok(Imported {
        profile: profile.name,
        bookmarks,
        history,
    })
}

/// Копия базы браузера во временной папке профиля Axiom: пока браузер запущен,
/// оригинал занят, а свежие записи лежат рядом в `-wal` или `-journal`.
/// Папка удаляется вместе со значением.
struct TempDb {
    dir: PathBuf,
    file: PathBuf,
}

impl TempDb {
    fn copy(src: &Path, paths: &Paths, browser: Browser) -> Result<Option<Self>> {
        if !src.is_file() {
            return Ok(None);
        }
        let id = format!("{browser:?}").to_lowercase();
        let dir = paths.root.join(format!("import-{id}.tmp"));
        let _ = fs::remove_dir_all(&dir);
        // Копия чужой истории: только для владельца, как и весь профиль.
        super::private_dir(&dir)?;
        let temp = Self {
            file: dir.join(src.file_name().unwrap_or_default()),
            dir,
        };
        fs::copy(src, &temp.file).with_context(|| {
            format!(
                "не удалось прочитать историю {} — закройте браузер и повторите",
                browser.from()
            )
        })?;
        for suffix in ["-wal", "-journal"] {
            let mut side = src.as_os_str().to_owned();
            side.push(suffix);
            let mut copy = temp.file.as_os_str().to_owned();
            copy.push(suffix);
            // Без журнала база всё равно читается, просто без последних минут.
            let _ = fs::copy(PathBuf::from(side), PathBuf::from(copy));
        }
        Ok(Some(temp))
    }

    /// Открываем на запись: это наша копия, и SQLite может понадобиться
    /// применить к ней журнал.
    fn open(&self) -> Result<rusqlite::Connection> {
        Ok(rusqlite::Connection::open(&self.file)?)
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn is_web(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Папка закладки: «Панель закладок / Rust / Web».
fn subfolder(folder: &str, name: &str) -> String {
    if folder.is_empty() || name.is_empty() {
        format!("{folder}{name}")
    } else {
        format!("{folder} / {name}")
    }
}

fn visits(count: i64) -> u32 {
    u32::try_from(count.max(1)).unwrap_or(u32::MAX)
}
