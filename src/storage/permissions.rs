//! Разрешения сайтов: кому можно микрофон, камеру, место, уведомления и прочее,
//! а кому — нет. Решение принимает человек в окошке под адресной строкой
//! или в Настройках → Безопасность; здесь — только память о нём.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{Paths, now_secs, read_json, write_json};

pub const FILE: &str = "permissions.json";
/// Больше сайтов не помним: самые давние вытесняются.
const MAX_SITES: usize = 500;
/// Файл больше этого — не наш (или испорчен): не читаем.
const MAX_FILE: u64 = 512 * 1024;
const MAX_SITE: usize = 300;

/// О чём сайт может попросить.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Perm {
    Microphone,
    Camera,
    Geolocation,
    Notifications,
    Clipboard,
    Midi,
    Fonts,
    Windows,
    Downloads,
    Files,
    /// Показ экрана. Как в Chrome, никогда не запоминается: каждый раз спрашиваем.
    Screen,
    /// Вид из будущей версии Axiom: при чтении отбрасывается.
    #[serde(other)]
    Unknown,
}

impl Perm {
    /// Решение можно запомнить для сайта.
    pub fn remembered(self) -> bool {
        !matches!(self, Self::Screen | Self::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Block,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    /// Origin сайта: `https://example.com` (с портом, если он не стандартный).
    pub site: String,
    pub perms: BTreeMap<Perm, Decision>,
    /// Когда решение по сайту менялось последний раз (секунды Unix).
    pub at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    sites: Vec<Site>,
    /// «Не спрашивать»: что запрещено всем сайтам, кроме разрешённых по отдельности.
    defaults: BTreeMap<Perm, Decision>,
}

impl Permissions {
    pub fn load(paths: &Paths) -> Self {
        let path = paths.root.join(FILE);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) {
            return Self::default();
        }
        read_json::<Self>(&path)
            .map(Self::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_json(&paths.root.join(FILE), self)
    }

    pub fn sites(&self) -> &[Site] {
        &self.sites
    }

    pub fn defaults(&self) -> &BTreeMap<Perm, Decision> {
        &self.defaults
    }

    /// Что решено для сайта: его собственное решение, иначе общее «не спрашивать».
    pub fn get(&self, site: &str, perm: Perm) -> Option<Decision> {
        self.own(site, perm)
            .or_else(|| self.defaults.get(&perm).copied())
            .filter(|_| perm.remembered())
    }

    /// Решение, принятое именно для этого сайта.
    pub fn own(&self, site: &str, perm: Perm) -> Option<Decision> {
        self.sites
            .iter()
            .find(|s| s.site == site)
            .and_then(|s| s.perms.get(&perm).copied())
    }

    /// Запоминает решение для сайта; `None` — снова спрашивать. Возвращает,
    /// поменялось ли что-то.
    pub fn set(&mut self, site: &str, perm: Perm, decision: Option<Decision>) -> bool {
        if !valid_site(site) || !perm.remembered() || decision == Some(Decision::Unknown) {
            return false;
        }
        let at = self.sites.iter().position(|s| s.site == site);
        match (decision, at) {
            (Some(d), Some(i)) => {
                let entry = &mut self.sites[i];
                if entry.perms.get(&perm) == Some(&d) {
                    return false;
                }
                entry.perms.insert(perm, d);
                entry.at = now_secs();
            }
            (Some(d), None) => {
                self.sites.push(Site {
                    site: site.to_string(),
                    perms: BTreeMap::from([(perm, d)]),
                    at: now_secs(),
                });
                self.trim();
            }
            (None, Some(i)) => {
                if self.sites[i].perms.remove(&perm).is_none() {
                    return false;
                }
                if self.sites[i].perms.is_empty() {
                    self.sites.remove(i);
                } else {
                    self.sites[i].at = now_secs();
                }
            }
            (None, None) => return false,
        }
        true
    }

    /// Забыть всё про сайт.
    pub fn forget(&mut self, site: &str) -> bool {
        let before = self.sites.len();
        self.sites.retain(|s| s.site != site);
        self.sites.len() != before
    }

    /// Общее правило для всех сайтов: `Some(Block)` — не спрашивать, сразу запрещать;
    /// `None` — спрашивать. Разрешить всем сразу нельзя.
    pub fn set_default(&mut self, perm: Perm, decision: Option<Decision>) -> bool {
        if !perm.remembered() {
            return false;
        }
        match decision {
            Some(Decision::Block) => {
                self.defaults.insert(perm, Decision::Block) != Some(Decision::Block)
            }
            None => self.defaults.remove(&perm).is_some(),
            Some(_) => false,
        }
    }

    fn sanitized(mut self) -> Self {
        let mut seen = std::collections::HashSet::new();
        // Повторы — оставляем последнее.
        let mut sites: Vec<Site> = std::mem::take(&mut self.sites)
            .into_iter()
            .rev()
            .filter(|s| valid_site(&s.site) && seen.insert(s.site.clone()))
            .map(|mut s| {
                s.perms
                    .retain(|p, d| p.remembered() && *d != Decision::Unknown);
                s
            })
            .filter(|s| !s.perms.is_empty())
            .collect();
        sites.reverse();
        self.sites = sites;
        self.defaults
            .retain(|p, d| p.remembered() && *d == Decision::Block);
        self.trim();
        self
    }

    fn trim(&mut self) {
        if self.sites.len() > MAX_SITES {
            self.sites.sort_by_key(|s| s.at);
            let extra = self.sites.len() - MAX_SITES;
            self.sites.drain(..extra);
        }
    }
}

/// Запомнить можно только origin веб-страницы: `https://host[:port]` без пути.
pub fn valid_site(site: &str) -> bool {
    let Some(rest) = site
        .strip_prefix("https://")
        .or_else(|| site.strip_prefix("http://"))
    else {
        return false;
    };
    site.len() <= MAX_SITE
        && !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-.:[]_".contains(&b))
        && !rest.bytes().any(|b| b.is_ascii_uppercase())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const S: &str = "https://meet.example.com";

    #[test]
    fn remembers_changes_and_forgets() {
        let mut p = Permissions::default();
        assert_eq!(p.get(S, Perm::Camera), None);
        assert!(p.set(S, Perm::Camera, Some(Decision::Allow)));
        assert!(!p.set(S, Perm::Camera, Some(Decision::Allow)));
        assert!(p.set(S, Perm::Microphone, Some(Decision::Block)));
        assert_eq!(p.get(S, Perm::Camera), Some(Decision::Allow));
        assert_eq!(p.get(S, Perm::Microphone), Some(Decision::Block));
        assert_eq!(p.get("https://other.example", Perm::Camera), None);
        assert_eq!(p.sites().len(), 1);
        // «Спрашивать» убирает решение, последнее — и сам сайт.
        assert!(p.set(S, Perm::Camera, None));
        assert!(!p.set(S, Perm::Camera, None));
        assert!(p.set(S, Perm::Microphone, None));
        assert!(p.sites().is_empty());
        p.set(S, Perm::Geolocation, Some(Decision::Allow));
        assert!(p.forget(S));
        assert!(!p.forget(S));
    }

    #[test]
    fn screen_and_junk_are_never_saved() {
        let mut p = Permissions::default();
        assert!(!p.set(S, Perm::Screen, Some(Decision::Allow)));
        assert!(!p.set(S, Perm::Unknown, Some(Decision::Allow)));
        assert!(!p.set(S, Perm::Camera, Some(Decision::Unknown)));
        for bad in [
            "javascript:alert(1)",
            "https://",
            "https://Example.com",
            "https://a.example/path",
            "file:///C:/a.html",
            "null",
        ] {
            assert!(!p.set(bad, Perm::Camera, Some(Decision::Allow)), "{bad}");
        }
        assert!(p.set("http://127.0.0.1:8765", Perm::Camera, Some(Decision::Allow)));
        assert!(p.set("http://[::1]:3000", Perm::Camera, Some(Decision::Allow)));
    }

    #[test]
    fn defaults_block_only_without_own_decision() {
        let mut p = Permissions::default();
        assert!(p.set_default(Perm::Notifications, Some(Decision::Block)));
        assert!(!p.set_default(Perm::Notifications, Some(Decision::Block)));
        // Разрешить всем сразу нельзя.
        assert!(!p.set_default(Perm::Camera, Some(Decision::Allow)));
        assert_eq!(p.get(S, Perm::Notifications), Some(Decision::Block));
        p.set(S, Perm::Notifications, Some(Decision::Allow));
        assert_eq!(p.get(S, Perm::Notifications), Some(Decision::Allow));
        assert!(p.set_default(Perm::Notifications, None));
        assert_eq!(p.get("https://other.example", Perm::Notifications), None);
    }

    #[test]
    fn reads_future_and_broken_files() {
        let raw = r#"{
            "sites": [
                {"site": "https://a.example", "perms": {"camera": "allow", "teleport": "allow", "microphone": "maybe"}, "at": 5},
                {"site": "javascript:x", "perms": {"camera": "allow"}, "at": 6},
                {"site": "https://b.example", "perms": {"screen": "allow"}, "at": 7},
                {"site": "https://a.example", "perms": {"geolocation": "block"}, "at": 8}
            ],
            "defaults": {"notifications": "block", "camera": "allow"},
            "future": true
        }"#;
        let p = serde_json::from_str::<Permissions>(raw)
            .map(Permissions::sanitized)
            .expect("файл читается");
        // Повтор сайта — последнее, мусор и показ экрана — прочь.
        assert_eq!(p.sites().len(), 1);
        assert_eq!(
            p.get("https://a.example", Perm::Geolocation),
            Some(Decision::Block)
        );
        assert_eq!(p.get("https://a.example", Perm::Camera), None);
        assert_eq!(p.defaults().len(), 1);
    }

    #[test]
    fn keeps_the_newest_sites() {
        let mut p = Permissions::default();
        for i in 0..MAX_SITES + 10 {
            p.set(
                &format!("https://s{i}.example"),
                Perm::Camera,
                Some(Decision::Allow),
            );
        }
        assert_eq!(p.sites().len(), MAX_SITES);
    }
}
