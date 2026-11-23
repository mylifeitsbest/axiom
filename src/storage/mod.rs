//! Папка профиля и простые JSON-файлы: настройки, сессия, история.

pub mod bookmarks;
pub mod chrome;
pub mod history;
pub mod session;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, Clone)]
pub struct Paths {
    /// Корень профиля: `%LOCALAPPDATA%\Axiom\data` на Windows.
    pub root: PathBuf,
}

impl Paths {
    /// `AXIOM_PROFILE=<папка>` — отдельный профиль (портативный режим, тесты).
    pub fn resolve() -> Result<Self> {
        let root = match std::env::var_os("AXIOM_PROFILE").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => ProjectDirs::from("", "", "Axiom")
                .ok_or_else(|| anyhow!("не найдена домашняя папка пользователя"))?
                .data_local_dir()
                .to_path_buf(),
        };
        fs::create_dir_all(&root)
            .with_context(|| format!("не удалось создать папку профиля {}", root.display()))?;
        Ok(Self { root })
    }

    /// Данные webview вкладок и интерфейса (куки, кэш).
    pub fn webview_data(&self) -> PathBuf {
        self.root.join("webview")
    }

    /// Данные панели ИИ. Для каждого прокси — своя папка: WebView2 требует,
    /// чтобы окружения с разными аргументами запуска не делили одну папку.
    pub fn ai_data(&self, proxy_key: &str) -> PathBuf {
        self.root.join(format!("ai-{proxy_key}"))
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    // Блокнот сохраняет UTF-8 с BOM — serde_json такое не принимает.
    let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    serde_json::from_slice(json).ok()
}

/// Атомарная запись: сначала во временный файл, затем переименование.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(&tmp, bytes).with_context(|| format!("запись {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("замена {}", path.display()))?;
    Ok(())
}

/// Текущее время в секундах Unix.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
