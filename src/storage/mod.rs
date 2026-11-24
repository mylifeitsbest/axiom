//! Папка профиля и простые JSON-файлы: настройки, сессия, история.

pub mod bookmarks;
pub mod chrome;
pub mod history;
pub mod session;

use std::{
    fs,
    io::Write,
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
    match serde_json::from_slice(json) {
        Ok(value) => Some(value),
        Err(_) => {
            // Битый файл (сбой питания, опечатка при ручной правке) иначе молча
            // заменился бы пустым при следующем сохранении. Откладываем его в сторону.
            let _ = fs::rename(path, path.with_extension("json.bad"));
            None
        }
    }
}

/// Атомарная запись: сначала во временный файл, затем переименование.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes(path, &serde_json::to_vec_pretty(value)?)
}

/// То же без отступов: для больших файлов, которые руками не правят (история).
pub fn write_json_compact<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes(path, &serde_json::to_vec(value)?)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let mut file = fs::File::create(&tmp).with_context(|| format!("запись {}", tmp.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("запись {}", tmp.display()))?;
    // Без этого после сбоя питания переименование может пережить данные,
    // и на месте файла окажется пустой или обнулённый.
    file.sync_all()
        .with_context(|| format!("запись {}", tmp.display()))?;
    drop(file);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_file_is_set_aside() {
        let root = std::env::temp_dir().join(format!("axiom-json-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        let file = root.join("settings.json");
        let _ = fs::write(&file, b"{\"onboarded\": tru");
        let read: Option<serde_json::Value> = read_json(&file);
        let bad = fs::read(root.join("settings.json.bad")).unwrap_or_default();
        let gone = !file.exists();
        assert!(write_json(&file, &serde_json::json!({ "a": 1 })).is_ok());
        let back: Option<serde_json::Value> = read_json(&file);
        let _ = fs::remove_dir_all(&root);
        assert!(read.is_none());
        assert!(gone);
        assert_eq!(bad, b"{\"onboarded\": tru");
        assert_eq!(back, Some(serde_json::json!({ "a": 1 })));
    }
}
