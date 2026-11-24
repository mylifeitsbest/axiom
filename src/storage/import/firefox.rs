//! Firefox и браузеры на его основе (Zen): закладки и история лежат в одной
//! SQLite-базе `places.sqlite`, профили перечислены в `profiles.ini`.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::Result;
use rusqlite::Connection;

use super::{Browser, MAX_HISTORY, Profile, TempDb, is_web, subfolder, visits};
use crate::storage::{Paths, bookmarks::Bookmark, history::Entry};

type Section = (String, HashMap<String, String>);

fn parse_ini(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push((name.to_string(), HashMap::new()));
        } else if let (Some((key, value)), Some((_, section))) =
            (line.split_once('='), out.last_mut())
        {
            section.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    out
}

fn has_places(dir: &Path) -> bool {
    dir.join("places.sqlite").is_file()
}

/// Профиль, который открывает установленный Firefox (`[Install…]`), иначе
/// отмеченный `Default=1`, иначе любой с данными.
pub(super) fn profile(root: &Path) -> Option<Profile> {
    let ini = fs::read_to_string(root.join("profiles.ini")).ok()?;
    let sections = parse_ini(&ini);
    let resolve = |path: &str, relative: bool| {
        if relative && !Path::new(path).is_absolute() {
            root.join(path)
        } else {
            PathBuf::from(path)
        }
    };
    let profiles: Vec<(PathBuf, &str, bool)> = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Profile"))
        .filter_map(|(_, kv)| {
            let relative = kv.get("IsRelative").is_none_or(|v| v == "1");
            Some((
                resolve(kv.get("Path")?, relative),
                kv.get("Name").map_or("", String::as_str),
                kv.get("Default").is_some_and(|v| v == "1"),
            ))
        })
        .collect();
    let mut installs: Vec<PathBuf> = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Install"))
        .filter_map(|(_, kv)| kv.get("Default"))
        .map(|path| resolve(path, true))
        .collect();
    // Несколько установок (обычный Firefox и Developer Edition) — берём ту, где сидели позже.
    installs.sort_by_key(|dir| {
        std::cmp::Reverse(
            fs::metadata(dir.join("places.sqlite"))
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH),
        )
    });
    let dir = installs
        .into_iter()
        .chain(profiles.iter().filter(|p| p.2).map(|p| p.0.clone()))
        .chain(profiles.iter().map(|p| p.0.clone()))
        .find(|dir| has_places(dir))?;
    let with_data = profiles.iter().filter(|p| has_places(&p.0)).count();
    let name = profiles
        .iter()
        .find(|p| p.0 == dir)
        .map(|p| p.1)
        // «default-release» и подобные Firefox придумывает сам — человеку они ничего не скажут.
        .filter(|name| with_data > 1 && !name.to_lowercase().starts_with("default"))
        .unwrap_or("")
        .to_string();
    Some(Profile { dir, name })
}

pub(super) fn read(
    dir: &Path,
    paths: &Paths,
    browser: Browser,
) -> Result<(Vec<Bookmark>, Vec<Entry>)> {
    let Some(db) = TempDb::copy(&dir.join("places.sqlite"), paths, browser)? else {
        return Ok(Default::default());
    };
    // Соединение закрывается раньше, чем удаляется копия: объявлено позже.
    let conn = db.open()?;
    let bookmarks = read_bookmarks(&conn).unwrap_or_default();
    let history = read_history(&conn)?;
    Ok((bookmarks, history))
}

fn read_history(conn: &Connection) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT url, title, visit_count, last_visit_date FROM moz_places \
         WHERE hidden = 0 AND last_visit_date IS NOT NULL \
         ORDER BY last_visit_date DESC LIMIT ?1",
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
                last: micros_to_unix(last),
            });
        }
    }
    Ok(out)
}

struct Node {
    id: i64,
    kind: i64,
    title: String,
    url: String,
    added: u64,
}

const BOOKMARK: i64 = 1;
const FOLDER: i64 = 2;

fn read_bookmarks(conn: &Connection) -> Result<Vec<Bookmark>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.type, b.parent, b.guid, b.title, p.title, p.url, b.dateAdded \
         FROM moz_bookmarks b LEFT JOIN moz_places p ON p.id = b.fk \
         ORDER BY b.parent, b.position",
    )?;
    let mut children: HashMap<i64, Vec<Node>> = HashMap::new();
    let mut guids: HashMap<String, i64> = HashMap::new();
    let rows = stmt.query_map([], |row| {
        let title = row
            .get::<_, Option<String>>(4)?
            .filter(|t| !t.is_empty())
            .or(row.get::<_, Option<String>>(5)?)
            .unwrap_or_default();
        Ok((
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            Node {
                id: row.get(0)?,
                kind: row.get(1)?,
                title,
                url: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                added: micros_to_unix(row.get::<_, Option<i64>>(7)?.unwrap_or(0)),
            },
        ))
    })?;
    for (parent, guid, node) in rows.flatten() {
        guids.insert(guid, node.id);
        children.entry(parent).or_default().push(node);
    }
    let mut out = Vec::new();
    // Метки (`tags________`) — тоже папки в этой таблице, но это не закладки.
    for (guid, folder) in [
        ("toolbar_____", "Панель закладок"),
        ("menu________", "Меню закладок"),
        ("unfiled_____", "Другие закладки"),
        ("mobile______", "Мобильные закладки"),
    ] {
        if let Some(&id) = guids.get(guid) {
            walk(id, folder, &children, &mut out, 0);
        }
    }
    Ok(out)
}

fn walk(
    parent: i64,
    folder: &str,
    children: &HashMap<i64, Vec<Node>>,
    out: &mut Vec<Bookmark>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    for node in children.get(&parent).into_iter().flatten() {
        match node.kind {
            BOOKMARK if is_web(&node.url) => out.push(Bookmark {
                url: node.url.clone(),
                title: node.title.clone(),
                folder: folder.to_string(),
                added: node.added,
            }),
            FOLDER => walk(
                node.id,
                &subfolder(folder, &node.title),
                children,
                out,
                depth + 1,
            ),
            _ => {}
        }
    }
}

fn micros_to_unix(micros: i64) -> u64 {
    u64::try_from(micros / 1_000_000).unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axiom-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn picks_installed_profile() {
        let root = temp("ff-ini");
        for p in ["Profiles/old.default", "Profiles/new.default-release"] {
            fs::create_dir_all(root.join(p)).unwrap();
            fs::write(root.join(p).join("places.sqlite"), b"").unwrap();
        }
        let ini = "[Install308046B0AF4A39CB]\r\nDefault=Profiles/new.default-release\r\nLocked=1\r\n\r\n\
                   [Profile1]\r\nName=default-release\r\nIsRelative=1\r\nPath=Profiles/new.default-release\r\n\r\n\
                   [Profile0]\r\nName=default\r\nIsRelative=1\r\nPath=Profiles/old.default\r\nDefault=1\r\n\r\n\
                   [General]\r\nStartWithLastProfile=1\r\nVersion=2\r\n";
        fs::write(root.join("profiles.ini"), ini).unwrap();
        let found = profile(&root).unwrap();
        assert_eq!(found.dir, root.join("Profiles/new.default-release"));
        assert_eq!(found.name, "");

        // Без своей базы профиль пропускаем.
        fs::remove_file(root.join("Profiles/new.default-release/places.sqlite")).unwrap();
        assert_eq!(
            profile(&root).unwrap().dir,
            root.join("Profiles/old.default")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_places() {
        let dir = temp("ff-places");
        let conn = Connection::open(dir.join("places.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT, \
               visit_count INTEGER DEFAULT 0, hidden INTEGER DEFAULT 0, last_visit_date INTEGER);
             CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER, \
               parent INTEGER, position INTEGER, title TEXT, dateAdded INTEGER, guid TEXT);
             INSERT INTO moz_places VALUES
               (1, 'https://docs.rs/', 'Docs.rs', 3, 0, 1700000000000000),
               (2, 'https://tagged.example/', 'Tagged', 1, 0, NULL),
               (3, 'place:sort=8', NULL, 0, 1, NULL),
               (4, 'https://deep.example/', 'Deep', 0, 0, 1600000000000000);
             INSERT INTO moz_bookmarks VALUES
               (1, 2, NULL, 0, 0, '', 0, 'root________'),
               (2, 2, NULL, 1, 0, 'menu', 0, 'menu________'),
               (3, 2, NULL, 1, 1, 'toolbar', 0, 'toolbar_____'),
               (4, 2, NULL, 1, 2, 'tags', 0, 'tags________'),
               (5, 1, 1, 3, 0, NULL, 1700000000000000, 'aaaaaaaaaaaa'),
               (6, 2, NULL, 3, 1, 'Rust', 0, 'bbbbbbbbbbbb'),
               (7, 1, 4, 6, 0, 'Глубоко', 0, 'cccccccccccc'),
               (8, 2, NULL, 4, 0, 'work', 0, 'dddddddddddd'),
               (9, 1, 2, 8, 0, NULL, 0, 'eeeeeeeeeeee'),
               (10, 3, NULL, 2, 0, NULL, 0, 'ffffffffffff');",
        )
        .unwrap();
        drop(conn);
        let paths = Paths { root: dir.clone() };
        let (bookmarks, history) = read(&dir, &paths, Browser::Firefox).unwrap();
        let marks: Vec<(&str, &str, &str)> = bookmarks
            .iter()
            .map(|b| (b.url.as_str(), b.title.as_str(), b.folder.as_str()))
            .collect();
        assert_eq!(
            marks,
            [
                ("https://docs.rs/", "Docs.rs", "Панель закладок"),
                ("https://deep.example/", "Глубоко", "Панель закладок / Rust"),
            ]
        );
        assert_eq!(bookmarks[0].added, 1_700_000_000);
        let urls: Vec<&str> = history.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(urls, ["https://docs.rs/", "https://deep.example/"]);
        assert_eq!(history[0].visits, 3);
        assert_eq!(history[1].visits, 1);
        // Временная копия убрана за собой.
        assert!(!dir.join("import-firefox.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
