//! «Что нового»: изменения между двумя версиями — в окошке Axiom, а не на GitHub.
//!
//! Выпуски описаны в `CHANGELOG.md` (раздел `## X.Y.Z — дата` на версию), его
//! берём из main: там разделы всех вышедших версий. У бета-версий раздела нет —
//! beta.yml кладёт в пре-релиз `beta` файл `NOTES.txt`: первая строка — номер
//! бета-версии, дальше по строке на коммит после выпуска, `N<TAB>заголовок`,
//! где N — номер бета-версии с этим коммитом. Из заголовков коммитов в окошко
//! попадают только изменения для людей (без CI, тестов, сборки пакетов).

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use super::{Version, base_url, curl, curl_error, number};

/// Файл со списком изменений бета-версии в пре-релизе `beta`.
const BETA_NOTES: &str = "NOTES.txt";
/// Больше разделов в окошке не нужно: всё остальное — на GitHub.
const MAX_SECTIONS: usize = 12;

/// Какие изменения показать.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Which {
    /// Что принесёт доступное обновление.
    Next,
    /// Что нового в установленной версии.
    Current,
}

/// Изменения после `from` до `to` включительно. Без `from` — только сама `to`:
/// раздел выпуска или вся бета после выпуска.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub from: Option<Version>,
    pub to: Version,
}

impl Range {
    fn has(&self, v: Version) -> bool {
        v <= self.to
            && match self.from {
                Some(from) => v > from,
                // Бета без прежней версии — вся бета после своего выпуска.
                None => {
                    v == self.to
                        || (v.is_beta() && (v.0, v.1, v.2) == (self.to.0, self.to.1, self.to.2))
                }
            }
    }

    /// Есть ли в диапазоне выпуски (их описывает CHANGELOG.md).
    fn has_releases(&self) -> bool {
        let release = Version::new(self.to.0, self.to.1, self.to.2, None);
        match self.from {
            Some(from) => release > from,
            None => !self.to.is_beta(),
        }
    }
}

/// Раздел окошка: версия и её изменения.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    /// `0.6.1 — 26 ноября 2026` или `Бета-версия 0.6.1-beta.140`.
    pub title: String,
    /// Абзацы и списки (`- `, вложенные `  - `), внутри — `**жирный**` и `` `код` ``.
    pub text: String,
}

/// Скачивает и выбирает изменения. Один источник не ответил — показываем другой.
pub fn fetch(range: Range) -> Result<Vec<Section>> {
    let changelog = range
        .has_releases()
        .then(|| get(&format!("{}/raw/main/CHANGELOG.md", base_url())));
    let beta = range.to.is_beta().then(|| {
        get(&format!(
            "{}/releases/download/{}/{BETA_NOTES}",
            base_url(),
            super::BETA_TAG
        ))
    });
    let error = match (&changelog, &beta) {
        (Some(Err(err)), None | Some(Err(_))) | (None, Some(Err(err))) => Some(anyhow!("{err:#}")),
        _ => None,
    };
    if let Some(err) = error {
        return Err(err);
    }
    let ok = |r: Option<Result<String>>| r.and_then(Result::ok);
    Ok(select(ok(changelog).as_deref(), ok(beta).as_deref(), range))
}

fn get(url: &str) -> Result<String> {
    let out = curl()
        .args([
            "--fail",
            "--location",
            "--max-time",
            "30",
            "--max-filesize",
            "1048576",
        ])
        .arg(url)
        .output()
        .context("не удалось запустить curl")?;
    if !out.status.success() {
        return Err(curl_error(&out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Разделы диапазона, новые сверху: сначала бета, потом выпуски.
pub fn select(changelog: Option<&str>, beta: Option<&str>, range: Range) -> Vec<Section> {
    let mut out = Vec::new();
    if range.to.is_beta()
        && let Some(beta) = beta
    {
        let items: Vec<String> = beta_items(beta)
            .into_iter()
            .filter(|(v, _)| range.has(*v))
            .map(|(_, subject)| bullet(subject))
            .collect();
        if !items.is_empty() {
            out.push(Section {
                // «— после 0.6.1»: интерфейс покажет это мельче, как дату у выпуска.
                title: format!(
                    "Бета-версия {} — после {}.{}.{}",
                    range.to, range.to.0, range.to.1, range.to.2
                ),
                text: items.join("\n"),
            });
        }
    }
    if range.has_releases()
        && let Some(changelog) = changelog
    {
        out.extend(
            releases(changelog)
                .into_iter()
                .filter(|(v, _)| range.has(*v))
                .map(|(_, s)| s),
        );
    }
    out.truncate(MAX_SECTIONS);
    out
}

/// Разделы `## X.Y.Z — дата` из CHANGELOG.md в порядке файла (новые сверху).
fn releases(changelog: &str) -> Vec<(Version, Section)> {
    let mut out = Vec::new();
    let mut current: Option<(Version, String, Vec<&str>)> = None;
    let mut flush = |cur: Option<(Version, String, Vec<&str>)>| {
        if let Some((version, title, body)) = cur {
            out.push((
                version,
                Section {
                    title,
                    text: body.join("\n").trim_matches('\n').to_string(),
                },
            ));
        }
    };
    for line in changelog.lines() {
        let line = line.trim_end();
        if let Some(head) = line.strip_prefix("## ") {
            flush(current.take());
            current = head
                .split_whitespace()
                .next()
                .and_then(Version::parse)
                .filter(|v| !v.is_beta())
                .map(|v| (v, head.trim().to_string(), Vec::new()));
        } else if line.starts_with("# ") {
            flush(current.take());
        } else if let Some((_, _, body)) = current.as_mut() {
            body.push(line);
        }
    }
    flush(current);
    out
}

/// Строки `N<TAB>заголовок` из NOTES.txt — с номером бета-версии каждой.
fn beta_items(notes: &str) -> Vec<(Version, &str)> {
    let mut lines = notes.lines();
    let Some(head) = lines
        .next()
        .and_then(Version::parse)
        .filter(Version::is_beta)
    else {
        return Vec::new();
    };
    lines
        .filter_map(|line| {
            let (n, subject) = line.split_once('\t')?;
            let n = number(n.trim()).filter(|&n| n > 0)?;
            let subject = subject.trim();
            (!subject.is_empty() && !internal(subject))
                .then_some((Version::new(head.0, head.1, head.2, Some(n)), subject))
        })
        .collect()
}

/// Коммит не для людей: CI, сборка, тесты, номер версии. И статистика — о ней не пишем.
fn internal(subject: &str) -> bool {
    const SKIP: [&str; 9] = [
        "AUR",
        "CI",
        "README",
        "Тест",
        "Тесты",
        "Релиз",
        "Релизы",
        "Статистика",
        "Merge",
    ];
    let first = subject
        .split(|c: char| c.is_whitespace() || c == ':' || c == ',')
        .next()
        .unwrap_or_default();
    if SKIP.contains(&first) {
        return true;
    }
    // Коммит с номером версии: «Axiom 0.6.1», «Axiom 0.6.0 бета: …».
    if let Some(rest) = subject.strip_prefix("Axiom ")
        && rest
            .split_whitespace()
            .next()
            .and_then(Version::parse)
            .is_some()
    {
        return true;
    }
    subject.contains("clippy")
}

/// Заголовок коммита строкой списка: «Островок: чёлка …» → «- **Островок.** Чёлка …».
fn bullet(subject: &str) -> String {
    let subject = subject.replace("**", "");
    if let Some((lead, rest)) = subject.split_once(": ")
        && !lead.is_empty()
        && lead.chars().count() <= 40
        && !rest.trim().is_empty()
    {
        let lead = lead.trim_end_matches(['.', ' ']);
        return format!("- **{lead}.** {}", capital(rest.trim()));
    }
    format!("- {}", capital(subject.trim()))
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANGELOG: &str = "# Что нового в Axiom\r\n\r\nВступление.\r\n\r\n\
## 0.6.1 — 26 ноября 2026\r\n\r\nИсправления.\r\n\r\n- **Сайты без сна.** Текст.\r\n  - вложенный\r\n\r\n\
## 0.6.0 — 26 ноября 2026\r\n\r\n- **Островок.** Чёлка.\r\n\r\n\
## 0.5.5 — 25 ноября 2026\r\n\r\n- Внутренние изменения.\r\n";

    const NOTES: &str = "0.6.1-beta.142\n\
142\tПанель снизу: вкладки и адресная строка встают у нижнего края\n\
141\tСтатистика: «Сейчас» считает открытые браузеры\n\
140\tAUR: axiom-browser-bin 0.6.1\n\
139\tПодмена Referer на Linux: заголовок меняет расширение\n\
138\tТест: вылет по abort оставляет строку\n\
137\tLinux, NVIDIA: видео декодирует процессор\n\
136\tAxiom 0.6.1\n";

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("не версия: {s}"))
    }

    fn range(from: Option<&str>, to: &str) -> Range {
        Range {
            from: from.map(v),
            to: v(to),
        }
    }

    fn titles(sections: &[Section]) -> Vec<&str> {
        sections.iter().map(|s| s.title.as_str()).collect()
    }

    #[test]
    fn reads_changelog_sections() {
        let all = releases(CHANGELOG);
        let versions: Vec<String> = all.iter().map(|(v, _)| v.to_string()).collect();
        assert_eq!(versions, ["0.6.1", "0.6.0", "0.5.5"]);
        assert_eq!(all[0].1.title, "0.6.1 — 26 ноября 2026");
        assert_eq!(
            all[0].1.text,
            "Исправления.\n\n- **Сайты без сна.** Текст.\n  - вложенный"
        );
        assert_eq!(all[2].1.text, "- Внутренние изменения.");
    }

    #[test]
    fn update_between_releases_shows_every_version_in_between() {
        let s = select(Some(CHANGELOG), None, range(Some("0.5.5"), "0.6.1"));
        assert_eq!(
            titles(&s),
            ["0.6.1 — 26 ноября 2026", "0.6.0 — 26 ноября 2026"]
        );
        let s = select(Some(CHANGELOG), None, range(None, "0.6.0"));
        assert_eq!(titles(&s), ["0.6.0 — 26 ноября 2026"]);
    }

    #[test]
    fn beta_shows_only_commits_after_the_installed_beta() {
        let s = select(
            None,
            Some(NOTES),
            range(Some("0.6.1-beta.137"), "0.6.1-beta.142"),
        );
        assert_eq!(titles(&s), ["Бета-версия 0.6.1-beta.142 — после 0.6.1"]);
        assert_eq!(
            s[0].text,
            "- **Панель снизу.** Вкладки и адресная строка встают у нижнего края\n\
             - **Подмена Referer на Linux.** Заголовок меняет расширение"
        );
        // Новее той, что ставим, — не показываем.
        let s = select(
            None,
            Some(NOTES),
            range(Some("0.6.1-beta.137"), "0.6.1-beta.139"),
        );
        assert_eq!(
            s[0].text,
            "- **Подмена Referer на Linux.** Заголовок меняет расширение"
        );
    }

    #[test]
    fn beta_after_an_older_release_also_shows_the_release() {
        let r = range(Some("0.6.0"), "0.6.1-beta.142");
        assert!(r.has_releases());
        let s = select(Some(CHANGELOG), Some(NOTES), r);
        assert_eq!(
            titles(&s),
            [
                "Бета-версия 0.6.1-beta.142 — после 0.6.1",
                "0.6.1 — 26 ноября 2026"
            ]
        );
        // Бета после того же выпуска: выпуск уже стоит.
        assert!(!range(Some("0.6.1"), "0.6.1-beta.142").has_releases());
        assert!(!range(Some("0.6.1-beta.130"), "0.6.1-beta.142").has_releases());
    }

    #[test]
    fn release_after_beta_shows_only_the_release() {
        let s = select(
            Some(CHANGELOG),
            Some(NOTES),
            range(Some("0.6.0-beta.120"), "0.6.1"),
        );
        assert_eq!(titles(&s), ["0.6.1 — 26 ноября 2026"]);
    }

    #[test]
    fn current_beta_without_previous_version_shows_the_whole_beta() {
        let s = select(None, Some(NOTES), range(None, "0.6.1-beta.139"));
        assert_eq!(titles(&s), ["Бета-версия 0.6.1-beta.139 — после 0.6.1"]);
        assert_eq!(
            s[0].text,
            "- **Подмена Referer на Linux.** Заголовок меняет расширение\n\
             - **Linux, NVIDIA.** Видео декодирует процессор"
        );
    }

    #[test]
    fn notes_of_another_release_are_not_mixed_in() {
        // Вышла 0.6.2, а у пользователя ещё бета после 0.6.1: строки уже про другое.
        let notes = "0.6.2-beta.150\n149\tОстровок: что-то новое\n";
        let s = select(
            None,
            Some(notes),
            range(Some("0.6.1-beta.137"), "0.6.1-beta.142"),
        );
        assert!(s.is_empty());
        assert!(select(None, Some("мусор\n1\tтекст"), range(None, "0.6.1-beta.2")).is_empty());
    }

    #[test]
    fn hides_commits_not_meant_for_people() {
        for subject in [
            "AUR: axiom-browser-bin 0.6.0",
            "CI: кэш",
            "README: островок",
            "Тест: вылет",
            "Релизы: в описании",
            "Статистика: «Сейчас»",
            "Axiom 0.6.1",
            "Axiom 0.6.0 бета: островок",
            "Островок: на Linux без лишнего кода (clippy)",
        ] {
            assert!(internal(subject), "{subject}");
        }
        for subject in [
            "Axiom открывает ссылки быстрее",
            "Linux, NVIDIA: видео",
            "Тестеры: отчёт",
            "Панель ИИ — только для AI Mode: ссылки",
        ] {
            assert!(!internal(subject), "{subject}");
        }
    }

    #[test]
    fn subjects_become_list_items() {
        assert_eq!(
            bullet("Панель ИИ — только для AI Mode: ссылки из ответа"),
            "- **Панель ИИ — только для AI Mode.** Ссылки из ответа"
        );
        assert_eq!(bullet("починил **жирный**"), "- Починил жирный");
        let long = format!("{}: хвост", "очень ".repeat(10));
        assert!(bullet(&long).starts_with("- Очень очень"));
    }
}
