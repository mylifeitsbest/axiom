//! Импорт из Google Chrome: закладки и история.
//!
//! Пароли и куки не переносятся: Chrome шифрует их ключом, привязанным к самому
//! Chrome (app-bound encryption), и обходить эту защиту мы не будем.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{Paths, bookmarks::Bookmark, history::Entry};

/// Разница между эпохой Chrome (1601-01-01) и Unix в секундах.
const WEBKIT_TO_UNIX: i64 = 11_644_473_600;
const MAX_HISTORY: usize = 5000;

#[derive(Debug, Default)]
pub struct Imported {
    pub profile: String,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<Entry>,
}

/// Папка данных Chrome (на Linux также Chromium — он стоит в Kali и Arch).
fn user_data_dir() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    #[cfg(windows)]
    let candidates = [base
        .data_local_dir()
        .join("Google")
        .join("Chrome")
        .join("User Data")];
    #[cfg(target_os = "macos")]
    let candidates = [base.data_dir().join("Google").join("Chrome")];
    #[cfg(not(any(windows, target_os = "macos")))]
    let candidates = [
        base.config_dir().join("google-chrome"),
        base.config_dir().join("chromium"),
    ];
    candidates.into_iter().find(|dir| dir.is_dir())
}

pub fn is_available() -> bool {
    user_data_dir().is_some()
}

/// Последний использованный профиль из `Local State`.
fn last_profile(user_data: &Path) -> String {
    fs::read(user_data.join("Local State"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| v.pointer("/profile/last_used")?.as_str().map(String::from))
        .filter(|p| !p.contains(['/', '\\', '.']) && user_data.join(p).is_dir())
        .unwrap_or_else(|| "Default".into())
}

pub fn import(paths: &Paths) -> Result<Imported> {
    let user_data =
        user_data_dir().ok_or_else(|| anyhow!("не найден профиль Chrome или Chromium"))?;
    let profile = last_profile(&user_data);
    let dir = user_data.join(&profile);
    let bookmarks = read_bookmarks(&dir.join("Bookmarks")).unwrap_or_default();
    let history = read_history(&dir.join("History"), paths)?;
    Ok(Imported {
        profile,
        bookmarks,
        history,
    })
}

fn read_bookmarks(file: &Path) -> Option<Vec<Bookmark>> {
    let root: Value = serde_json::from_slice(&fs::read(file).ok()?).ok()?;
    let mut out = Vec::new();
    for (key, folder) in [
        ("bookmark_bar", "Панель закладок"),
        ("other", "Другие закладки"),
        ("synced", "Мобильные закладки"),
    ] {
        if let Some(node) = root.pointer(&format!("/roots/{key}")) {
            walk(node, folder, &mut out);
        }
    }
    Some(out)
}

fn walk(node: &Value, folder: &str, out: &mut Vec<Bookmark>) {
    let name = node.get("name").and_then(Value::as_str).unwrap_or("");
    match node.get("type").and_then(Value::as_str) {
        Some("url") => {
            let url = node.get("url").and_then(Value::as_str).unwrap_or("");
            if url.starts_with("http://") || url.starts_with("https://") {
                let added = node
                    .get("date_added")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<i64>().ok())
                    .map(webkit_to_unix)
                    .unwrap_or(0);
                out.push(Bookmark {
                    url: url.to_string(),
                    title: name.to_string(),
                    folder: folder.to_string(),
                    added,
                });
            }
        }
        Some("folder") => {
            let path = if folder.is_empty() || name.is_empty() {
                format!("{folder}{name}")
            } else {
                format!("{folder} / {name}")
            };
            for child in node
                .get("children")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(child, &path, out);
            }
        }
        _ => {}
    }
}

/// Базу истории копируем: пока Chrome запущен, оригинал заблокирован.
fn read_history(file: &Path, paths: &Paths) -> Result<Vec<Entry>> {
    if !file.exists() {
        return Ok(Vec::new());
    }
    let copy = paths.root.join("chrome-history.tmp");
    fs::copy(file, &copy)
        .context("не удалось прочитать историю Chrome — закройте Chrome и повторите")?;
    let result = query_history(&copy);
    let _ = fs::remove_file(&copy);
    result
}

fn query_history(db: &Path) -> Result<Vec<Entry>> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT url, title, visit_count, last_visit_time FROM urls \
         WHERE hidden = 0 ORDER BY last_visit_time DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([MAX_HISTORY as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (url, title, visits, last) = row;
        if url.starts_with("http://") || url.starts_with("https://") {
            out.push(Entry {
                url,
                title,
                visits: u32::try_from(visits.max(1)).unwrap_or(u32::MAX),
                last: webkit_to_unix(last),
            });
        }
    }
    Ok(out)
}

fn webkit_to_unix(micros: i64) -> u64 {
    u64::try_from(micros / 1_000_000 - WEBKIT_TO_UNIX).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn walks_bookmark_tree() {
        let tree = json!({
            "type": "folder", "name": "Rust", "children": [
                {"type": "url", "name": "Book", "url": "https://doc.rust-lang.org/book/", "date_added": "13300000000000000"},
                {"type": "url", "name": "Bad", "url": "javascript:alert(1)"},
                {"type": "folder", "name": "Web", "children": [
                    {"type": "url", "name": "wry", "url": "https://docs.rs/wry"}
                ]}
            ]
        });
        let mut out = Vec::new();
        walk(&tree, "Панель закладок", &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].folder, "Панель закладок / Rust");
        assert_eq!(out[1].folder, "Панель закладок / Rust / Web");
        assert!(out[0].added > 1_600_000_000);
    }

    #[test]
    fn converts_webkit_time() {
        assert_eq!(webkit_to_unix(WEBKIT_TO_UNIX * 1_000_000), 0);
        assert_eq!(webkit_to_unix(0), 0);
    }
}
