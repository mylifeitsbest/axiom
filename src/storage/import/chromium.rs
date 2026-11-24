//! Браузеры на Chromium: Chrome, Edge, Яндекс, Opera, Brave, Vivaldi, Chromium.
//! Закладки — JSON-файл `Bookmarks`, история — SQLite-база `History`.

use std::{fs, path::Path};

use anyhow::Result;
use serde_json::Value;

use super::{Browser, MAX_HISTORY, Profile, TempDb, is_web, subfolder, visits};
use crate::storage::{Paths, bookmarks::Bookmark, history::Entry};

/// Разница между эпохой Chrome (1601-01-01) и Unix в секундах.
const WEBKIT_TO_UNIX: i64 = 11_644_473_600;

/// Последний использованный профиль из `Local State`. У старой Opera профиль —
/// сама папка данных, без `Default` внутри.
pub(super) fn profile(user_data: &Path) -> Option<Profile> {
    let state = fs::read(user_data.join("Local State"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let last = state
        .as_ref()
        .and_then(|v| v.pointer("/profile/last_used")?.as_str())
        .filter(|p| !p.is_empty() && !p.contains(['/', '\\', '.']));
    let has_data = |dir: &Path| dir.join("Bookmarks").is_file() || dir.join("History").is_file();
    let (dir, key) = last
        .into_iter()
        .chain(["Default"])
        .map(|p| (user_data.join(p), p))
        .chain([(user_data.to_path_buf(), "")])
        .find(|(dir, _)| has_data(dir))?;
    Some(Profile {
        name: display_name(state.as_ref(), key),
        dir,
    })
}

/// Имя профиля, как его видит человек («Работа»), — только если профилей больше одного.
fn display_name(state: Option<&Value>, key: &str) -> String {
    let Some(cache) = state
        .and_then(|v| v.pointer("/profile/info_cache"))
        .and_then(Value::as_object)
    else {
        return String::new();
    };
    if cache.len() < 2 {
        return String::new();
    }
    cache
        .get(key)
        .and_then(|p| p.get("name")?.as_str())
        .unwrap_or(key)
        .to_string()
}

pub(super) fn read(
    dir: &Path,
    paths: &Paths,
    browser: Browser,
) -> Result<(Vec<Bookmark>, Vec<Entry>)> {
    let bookmarks = read_bookmarks(&dir.join("Bookmarks")).unwrap_or_default();
    let history = match TempDb::copy(&dir.join("History"), paths, browser)? {
        Some(db) => query_history(&db)?,
        None => Vec::new(),
    };
    Ok((bookmarks, history))
}

fn read_bookmarks(file: &Path) -> Option<Vec<Bookmark>> {
    let root: Value = serde_json::from_slice(&fs::read(file).ok()?).ok()?;
    let roots = root.get("roots")?;
    let mut out = Vec::new();
    for (key, folder) in [
        ("bookmark_bar", "Панель закладок"),
        ("other", "Другие закладки"),
        ("synced", "Мобильные закладки"),
        // Opera: свои корни рядом с обычными, корзину не берём.
        ("custom_root/userRoot", "Мои папки"),
        ("custom_root/unsorted", "Неотсортированные"),
        ("custom_root/speedDial", "Экспресс-панель"),
    ] {
        if let Some(node) = roots.pointer(&format!("/{key}")) {
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
            if is_web(url) {
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
        // Корзина Vivaldi — обычная папка с пометкой.
        Some("folder") if node.pointer("/meta_info/Trash").is_none() => {
            let path = subfolder(folder, name);
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

fn query_history(db: &TempDb) -> Result<Vec<Entry>> {
    let conn = db.open()?;
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
    for (url, title, count, last) in rows.flatten() {
        if is_web(&url) {
            out.push(Entry {
                url,
                title,
                visits: visits(count),
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
#[allow(clippy::unwrap_used)]
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
                ]},
                {"type": "folder", "name": "Корзина", "meta_info": {"Trash": "true"}, "children": [
                    {"type": "url", "name": "old", "url": "https://old.example"}
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
    fn reads_opera_roots() {
        let dir = std::env::temp_dir().join(format!("axiom-opera-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Bookmarks");
        let link = |url: &str| json!({"type": "url", "name": url, "url": url});
        let folder = |items: Vec<Value>| json!({"type": "folder", "name": "", "children": items});
        let data = json!({"roots": {
            "bookmark_bar": folder(vec![link("https://a.example")]),
            "custom_root": {
                "speedDial": folder(vec![link("https://b.example")]),
                "trash": folder(vec![link("https://c.example")]),
                "unsorted": folder(vec![]),
                "userRoot": folder(vec![]),
            },
            "workspaces_v2": folder(vec![link("https://d.example")]),
        }});
        fs::write(&file, data.to_string()).unwrap();
        let urls: Vec<String> = read_bookmarks(&file)
            .unwrap()
            .into_iter()
            .map(|b| b.url)
            .collect();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(urls, ["https://a.example", "https://b.example"]);
    }

    #[test]
    fn finds_profile() {
        let dir = std::env::temp_dir().join(format!("axiom-chromium-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Profile 2")).unwrap();
        fs::write(dir.join("Profile 2").join("History"), b"").unwrap();
        let state = json!({"profile": {"last_used": "Profile 2", "info_cache": {
            "Default": {"name": "Личный"}, "Profile 2": {"name": "Работа"}
        }}});
        fs::write(dir.join("Local State"), state.to_string()).unwrap();
        let found = profile(&dir).unwrap();
        assert_eq!(found.dir, dir.join("Profile 2"));
        assert_eq!(found.name, "Работа");

        // Пустая папка (браузер удалён, папка осталась) — не профиль.
        fs::remove_file(dir.join("Profile 2").join("History")).unwrap();
        assert!(profile(&dir).is_none());

        // Старая Opera: данные прямо в папке.
        fs::write(dir.join("Bookmarks"), b"{}").unwrap();
        assert_eq!(profile(&dir).unwrap().dir, dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn converts_webkit_time() {
        assert_eq!(webkit_to_unix(WEBKIT_TO_UNIX * 1_000_000), 0);
        assert_eq!(webkit_to_unix(0), 0);
    }
}
