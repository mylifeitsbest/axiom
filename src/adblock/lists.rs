//! Списки фильтров: встроенный стартовый набор и свежие копии в профиле.
//!
//! Встроенные списки — `assets/adblock/*.txt`, build.rs сжимает их в программу.
//! Свежие Axiom раз в [`UPDATE_EVERY`] скачивает в `<профиль>/adblock/` (если это
//! не выключено в настройках): с условием «если изменился» (`--time-cond`), так что
//! неизменный список заново не качается. Скачанный список годится, только если он
//! похож на список фильтров (заголовок `[Adblock Plus`, достаточно правил); из двух
//! копий — встроенной и скачанной — берётся более новая по `! Version:`.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};

/// Как часто проверять списки. Сами списки обещают свежесть на 1–4 дня.
pub const UPDATE_EVERY: Duration = Duration::from_secs(4 * 24 * 60 * 60);
/// Больше этого список не бывает (EasyList — около 2 МБ).
const MAX_SIZE: u64 = 16 << 20;
/// Меньше правил — не список, а страница ошибки или обрезок.
const MIN_RULES: usize = 1000;

pub struct Source {
    /// Имя файла в профиле и в `assets/adblock`.
    pub id: &'static str,
    pub title: &'static str,
    /// Откуда качать: первый адрес, а не ответил — следующий.
    urls: &'static [&'static str],
    /// Встроенная копия, сжатая build.rs.
    packed: &'static [u8],
}

pub const SOURCES: [Source; 2] = [
    Source {
        id: "easylist",
        title: "EasyList",
        urls: &[
            "https://easylist-downloads.adblockplus.org/easylist.txt",
            "https://easylist.to/easylist/easylist.txt",
        ],
        packed: include_bytes!(concat!(env!("OUT_DIR"), "/easylist.txt.z")),
    },
    Source {
        id: "ruadlist",
        title: "RuAdList",
        urls: &["https://easylist-downloads.adblockplus.org/advblock.txt"],
        packed: include_bytes!(concat!(env!("OUT_DIR"), "/ruadlist.txt.z")),
    },
];

/// Список, по которому собран движок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub id: &'static str,
    pub title: &'static str,
    /// `! Version:` из заголовка: число вида 202611271902.
    pub version: u64,
    /// `! Last modified:` как в заголовке, например «27 Nov 2026 19:02 UTC».
    pub modified: String,
    /// Свежая копия из профиля, а не встроенная.
    pub downloaded: bool,
    pub text: String,
}

/// Встроенная копия списка.
fn embedded(source: &Source) -> Result<String> {
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(source.packed, MAX_SIZE as usize)
        .map_err(|err| anyhow!("встроенный {} не распаковался: {err:?}", source.title))?;
    String::from_utf8(raw).with_context(|| format!("встроенный {} — не UTF-8", source.title))
}

pub fn path_of(dir: &Path, source: &Source) -> PathBuf {
    dir.join(format!("{}.txt", source.id))
}

/// Скачанная копия, если она есть и годится.
fn downloaded(dir: &Path, source: &Source) -> Option<String> {
    let path = path_of(dir, source);
    let meta = fs::metadata(&path).ok()?;
    if meta.len() > MAX_SIZE {
        return None;
    }
    let text = fs::read_to_string(&path).ok()?;
    valid(&text).then_some(text)
}

/// Все списки: для каждого — более новая из двух копий.
pub fn load(dir: &Path) -> Vec<Loaded> {
    SOURCES
        .iter()
        .filter_map(|source| {
            let built_in = match embedded(source) {
                Ok(text) => Some(text),
                Err(err) => {
                    eprintln!("блокировка рекламы: {err:#}");
                    None
                }
            };
            let fresh = downloaded(dir, source);
            let pick = match (built_in, fresh) {
                (Some(a), Some(b)) => {
                    if version(&b) > version(&a) {
                        (b, true)
                    } else {
                        (a, false)
                    }
                }
                (Some(a), None) => (a, false),
                (None, Some(b)) => (b, true),
                (None, None) => return None,
            };
            let (text, downloaded) = pick;
            Some(Loaded {
                id: source.id,
                title: source.title,
                version: version(&text),
                modified: header(&text, "Last modified").unwrap_or_default(),
                downloaded,
                text,
            })
        })
        .collect()
}

/// Значение строки заголовка `! Name: value`.
fn header(text: &str, name: &str) -> Option<String> {
    text.lines()
        .take(60)
        .take_while(|line| line.starts_with('!') || line.starts_with('['))
        .find_map(|line| {
            let rest = line.strip_prefix('!')?.trim_start();
            let value = rest.strip_prefix(name)?.strip_prefix(':')?;
            Some(value.trim().to_string())
        })
}

fn version(text: &str) -> u64 {
    header(text, "Version")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Похоже ли на список фильтров ABP.
fn valid(text: &str) -> bool {
    text.trim_start_matches('\u{feff}')
        .starts_with("[Adblock Plus")
        && text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('!'))
            .nth(MIN_RULES)
            .is_some()
}

/// Скачивает новые версии списков. Возвращает id изменившихся; ни один адрес
/// списка не ответил — ошибка (остальные списки при этом обновляются).
pub fn update(dir: &Path) -> Result<Vec<&'static str>> {
    crate::storage::private_dir(dir).context("нет папки для списков")?;
    let mut changed = Vec::new();
    let mut failed = None;
    for source in &SOURCES {
        match fetch(dir, source) {
            Ok(true) => changed.push(source.id),
            Ok(false) => {}
            Err(err) => {
                eprintln!("блокировка рекламы: {} не обновился: {err:#}", source.title);
                failed = Some(err);
            }
        }
    }
    match failed {
        Some(err) if changed.is_empty() => Err(err),
        _ => Ok(changed),
    }
}

/// `true` — скачан новый список.
fn fetch(dir: &Path, source: &Source) -> Result<bool> {
    let path = path_of(dir, source);
    let tmp = dir.join(format!("{}.txt.part", source.id));
    let mut last = None;
    for url in source.urls {
        let _ = fs::remove_file(&tmp);
        let mut cmd = crate::update::curl();
        cmd.args(["--fail", "--location", "--compressed", "--max-time", "180"])
            .args(["--max-filesize", &MAX_SIZE.to_string()])
            .arg("--output")
            .arg(&tmp);
        // Есть своя копия — сервер ответит 304 и ничего не пришлёт, если список тот же.
        if path.exists() {
            cmd.arg("--time-cond").arg(&path);
        }
        let out = cmd.arg(url).output().context("не удалось запустить curl")?;
        if !out.status.success() {
            last = Some(anyhow!(
                "{url}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            continue;
        }
        // 304: файла нет — список не менялся.
        let Ok(text) = fs::read_to_string(&tmp) else {
            return Ok(false);
        };
        if !valid(&text) {
            let _ = fs::remove_file(&tmp);
            last = Some(anyhow!("{url}: это не список фильтров"));
            continue;
        }
        let old = fs::read_to_string(&path).ok();
        if old.as_deref() == Some(text.as_str()) {
            let _ = fs::remove_file(&tmp);
            return Ok(false);
        }
        fs::rename(&tmp, &path).context("список не сохранён")?;
        return Ok(true);
    }
    let _ = fs::remove_file(&tmp);
    match last {
        Some(err) => Err(err),
        None => bail!("нет адреса"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn embedded_lists_unpack() {
        for source in &SOURCES {
            let text = embedded(source).unwrap();
            assert!(valid(&text), "{}", source.title);
            assert!(version(&text) > 202_000_000_000, "{}", source.title);
            assert!(header(&text, "Last modified").is_some(), "{}", source.title);
        }
    }

    #[test]
    fn header_and_validity() {
        let mut text = String::from(
            "[Adblock Plus 2.0]\n! Version: 202611271902\n! Title: EasyList\n! Last modified: 27 Nov 2026 19:02 UTC\n",
        );
        assert_eq!(version(&text), 202_611_271_902);
        assert_eq!(
            header(&text, "Last modified").as_deref(),
            Some("27 Nov 2026 19:02 UTC")
        );
        assert_eq!(header(&text, "Expires"), None);
        assert!(!valid(&text), "без правил");
        for i in 0..=MIN_RULES {
            text.push_str(&format!("||ads{i}.example^\n"));
        }
        assert!(valid(&text));
        assert!(!valid("<html>Rate limited</html>"));
        assert!(valid(&format!("\u{feff}{text}")));
    }

    #[test]
    fn newer_copy_wins() {
        let dir = std::env::temp_dir().join(format!("axiom-adblock-lists-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut fresh = String::from(
            "[Adblock Plus 2.0]\n! Version: 999912312359\n! Last modified: 31 Dec 9999 23:59 UTC\n",
        );
        for i in 0..=MIN_RULES {
            fresh.push_str(&format!("||ads{i}.example^\n"));
        }
        fs::write(dir.join("easylist.txt"), &fresh).unwrap();
        // Старее встроенного — берётся встроенный.
        fs::write(
            dir.join("ruadlist.txt"),
            fresh.replace("999912312359", "200001010000"),
        )
        .unwrap();
        let lists = load(&dir);
        assert_eq!(lists.len(), 2);
        assert!(lists[0].downloaded && lists[0].version == 999_912_312_359);
        assert!(!lists[1].downloaded);
        // Не список — не берётся.
        fs::write(dir.join("easylist.txt"), "<html>").unwrap();
        assert!(!load(&dir)[0].downloaded);
        let _ = fs::remove_dir_all(&dir);
    }
}
