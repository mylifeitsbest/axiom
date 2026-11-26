//! Ответы разработчика на «Сообщить о проблеме».
//!
//! Сообщение человека уходит со случайным ключом ответа (128 бит): с браузером,
//! пингами и другими сообщениями он не связан. Свои сообщения браузер помнит в
//! `feedback.json` профиля и, пока Axiom открыт, раз в 20 минут спрашивает сервер
//! (`POST {STATS_URL}/a` с ключами), ответили ли на них. Пришёл ответ — интерфейс
//! показывает его кнопкой в панели и в Настройках → Система.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};
use tao::event_loop::EventLoopProxy;

use crate::events::UserEvent;

const FILE: &str = "feedback.json";
/// Как часто спрашивать об ответах и сколько ждать после запуска.
const ASK_EVERY: Duration = Duration::from_secs(20 * 60);
const WARMUP: Duration = Duration::from_secs(40);
/// Об ответе спрашиваем месяц после отправки, а помним сообщение 90 дней — столько
/// же, сколько его хранит сервер.
const ASK_FOR: u64 = 30 * 24 * 60 * 60;
const KEEP_FOR: u64 = 90 * 24 * 60 * 60;
/// Сообщений помним не больше (столько же сервер принимает ключей за раз).
const MAX_MESSAGES: usize = 16;
/// Сколько своего текста хранить для подписи «вы писали…» (символов).
const KEEP_TEXT: usize = 600;
/// Ответ сервера больше этого — не наш.
const MAX_RESPONSE: usize = 64 * 1024;
/// Текст ответа на сервере — до 2000 символов; с запасом.
const MAX_ANSWER_TEXT: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// «Исправлено» (или «сделано» — для просьб).
    Fixed,
    /// Просто ответ: уточнить, объяснить.
    Reply,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub status: Status,
    #[serde(default)]
    pub text: String,
    /// В какой версии исправлено (может не быть).
    #[serde(default)]
    pub version: String,
    /// Когда ответили (время сервера, ISO 8601): другое время — новый ответ.
    #[serde(default)]
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Message {
    token: String,
    /// Когда отправлено (секунды Unix).
    at: u64,
    /// Начало того, что человек написал.
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answer: Option<Answer>,
    /// Ответ уже видели («Понятно»).
    #[serde(default)]
    seen: bool,
}

/// Сообщение для интерфейса: без ключа ответа.
#[derive(Debug, Serialize)]
pub struct View {
    /// Для «Понятно»: начало ключа, его достаточно, чтобы отличить сообщения.
    id: String,
    at: u64,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<Answer>,
    seen: bool,
    /// Ответ «исправлено в версии X»: X уже у человека (`true`) или придёт с обновлением.
    #[serde(skip_serializing_if = "Option::is_none")]
    has_fix: Option<bool>,
}

/// Один замок на чтение-изменение-запись файла: его трогают поток опроса,
/// поток отправки и интерфейс.
static LOCK: Mutex<()> = Mutex::new(());
static ROOT: OnceLock<PathBuf> = OnceLock::new();
static EVENTS: OnceLock<Mutex<EventLoopProxy<UserEvent>>> = OnceLock::new();

/// Новый ключ ответа для сообщения.
pub(super) fn token() -> String {
    crate::instance::random_token()
}

/// Запомнить отправленное (или отложенное до связи) сообщение.
pub(super) fn remember(root: &Path, token: String, text: &str) {
    let _ = ROOT.set(root.to_path_buf());
    let text: String = text.trim().chars().take(KEEP_TEXT).collect();
    let changed = edit(|list| {
        list.push(Message {
            token,
            at: now(),
            text,
            answer: None,
            seen: false,
        });
        true
    });
    if changed {
        notify();
    }
}

/// Интерфейс готов принимать новости: запустить опрос (один раз за запуск).
pub fn listen(root: &Path, events: EventLoopProxy<UserEvent>) {
    let _ = ROOT.set(root.to_path_buf());
    if EVENTS.set(Mutex::new(events)).is_err() {
        return;
    }
    let spawned = thread::Builder::new().name("answers".into()).spawn(|| {
        let every = every();
        thread::sleep(WARMUP.min(every));
        loop {
            check();
            thread::sleep(every);
        }
    });
    if let Err(err) = spawned {
        eprintln!("ответы на сообщения не проверяются: {err}");
    }
}

/// Как часто спрашивать. Debug-сборка: `AXIOM_ANSWERS_EVERY=<секунды>` для проверки.
fn every() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(secs) = std::env::var("AXIOM_ANSWERS_EVERY")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
    {
        return Duration::from_secs(secs);
    }
    ASK_EVERY
}

/// Свои сообщения для интерфейса, новые сверху.
pub fn list() -> Vec<View> {
    let Some(root) = ROOT.get() else {
        return Vec::new();
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let current = crate::update::Version::current();
    let mut out: Vec<View> = load(root)
        .into_iter()
        .map(|m| View {
            id: short(&m.token).to_string(),
            at: m.at,
            has_fix: m
                .answer
                .as_ref()
                .filter(|a| a.status == Status::Fixed)
                .and_then(|a| crate::update::Version::parse(&a.version))
                .map(|v| current >= v),
            text: m.text,
            answer: m.answer,
            seen: m.seen,
        })
        .collect();
    out.reverse();
    out
}

/// «Понятно»: ответы на эти сообщения увидели.
pub fn seen(ids: &[String]) {
    let changed = edit(|list| {
        let mut changed = false;
        for m in list.iter_mut() {
            if m.answer.is_some() && !m.seen && ids.iter().any(|id| id == short(&m.token)) {
                m.seen = true;
                changed = true;
            }
        }
        changed
    });
    if changed {
        notify();
    }
}

fn short(token: &str) -> &str {
    token.get(..12).unwrap_or(token)
}

/// Спросить сервер об ответах на свежие сообщения и сохранить новые.
fn check() {
    // Тестовые запуски release-сборки (`AXIOM_NO_STATS=1`) на сервер не ходят.
    if std::env::var_os("AXIOM_NO_STATS").is_some_and(|v| !v.is_empty() && v != "0") {
        return;
    }
    let Some(root) = ROOT.get() else { return };
    let Some(url) = super::endpoint("a") else {
        return;
    };
    let tokens: Vec<String> = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let since = now().saturating_sub(ASK_FOR);
        load(root)
            .into_iter()
            .filter(|m| m.at >= since)
            .map(|m| m.token)
            .collect()
    };
    if tokens.is_empty() {
        return;
    }
    let Some(found) = ask(&url, &tokens) else {
        return;
    };
    let changed = edit(|list| merge(list, &found));
    if changed {
        notify();
    }
}

/// Новые и изменённые ответы — в список; такой ответ ещё не видели.
fn merge(list: &mut [Message], found: &[(String, Answer)]) -> bool {
    let mut changed = false;
    for (token, answer) in found {
        if let Some(m) = list.iter_mut().find(|m| &m.token == token)
            && m.answer.as_ref() != Some(answer)
        {
            m.answer = Some(answer.clone());
            m.seen = false;
            changed = true;
        }
    }
    changed
}

/// `POST /a {"t": [ключи]}` → ответы на те из них, на которые ответили.
fn ask(url: &str, tokens: &[String]) -> Option<Vec<(String, Answer)>> {
    let body = serde_json::to_vec(&serde_json::json!({ "t": tokens })).ok()?;
    let mut cmd = crate::stats::curl();
    cmd.args([
        "--fail",
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
    ])
    .args(["--data-binary", "@-"])
    .args(["--max-filesize", &MAX_RESPONSE.to_string()])
    .arg(url)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&body).ok()?;
    }
    let mut out = Vec::new();
    child
        .stdout
        .take()?
        .take(MAX_RESPONSE as u64 + 1)
        .read_to_end(&mut out)
        .ok()?;
    // Старый сервер (без `/a`) отвечает 404 — `--fail` даёт ошибку: ответов нет.
    if !child.wait().ok()?.success() || out.len() > MAX_RESPONSE {
        return None;
    }
    Some(parse(&out, tokens))
}

/// Разобрать ответ сервера: только спрошенные ключи и разумные поля.
fn parse(body: &[u8], tokens: &[String]) -> Vec<(String, Answer)> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        answers: std::collections::BTreeMap<String, serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Raw {
        s: Status,
        #[serde(default)]
        text: String,
        #[serde(default)]
        v: String,
        #[serde(default)]
        at: String,
    }
    let Ok(reply) = serde_json::from_slice::<Reply>(body) else {
        return Vec::new();
    };
    reply
        .answers
        .into_iter()
        .filter(|(token, _)| tokens.contains(token))
        // Ответ, который не разобрать (новый статус от будущего сервера), — пропускаем.
        .filter_map(|(token, raw)| Some((token, serde_json::from_value::<Raw>(raw).ok()?)))
        .filter(|(_, raw)| {
            raw.text.chars().count() <= MAX_ANSWER_TEXT && raw.v.len() <= 40 && raw.at.len() <= 40
        })
        .map(|(token, raw)| {
            let version = match crate::update::Version::parse(&raw.v) {
                Some(v) => v.to_string(),
                None => String::new(),
            };
            let answer = Answer {
                status: raw.s,
                text: raw.text.trim().to_string(),
                version,
                at: raw.at,
            };
            (token, answer)
        })
        .collect()
}

/// Прочитать, изменить и записать список (под замком). `f` возвращает, изменилось ли что-то.
fn edit(f: impl FnOnce(&mut Vec<Message>) -> bool) -> bool {
    let Some(root) = ROOT.get() else { return false };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = load(root);
    let before = list.len();
    let changed = f(&mut list);
    let extra = list.len().saturating_sub(MAX_MESSAGES);
    list.drain(..extra);
    if !changed && list.len() == before {
        return false;
    }
    if let Err(err) = crate::storage::write_json(&root.join(FILE), &list) {
        eprintln!("ответы на сообщения не сохранены: {err:#}");
    }
    changed
}

/// Свои сообщения, старые первыми; просроченные отброшены.
fn load(root: &Path) -> Vec<Message> {
    let path = root.join(FILE);
    if !path.exists() {
        return Vec::new();
    }
    let since = now().saturating_sub(KEEP_FOR);
    let mut list: Vec<Message> = crate::storage::read_json(&path).unwrap_or_default();
    list.retain(|m| m.at >= since && !m.token.is_empty());
    list
}

fn notify() {
    if let Some(events) = EVENTS.get() {
        let events = events.lock().unwrap_or_else(|e| e.into_inner());
        let _ = events.send_event(UserEvent::Answers);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn message(token: &str) -> Message {
        Message {
            token: token.into(),
            at: now(),
            text: "кнопка не жмётся".into(),
            answer: None,
            seen: false,
        }
    }

    #[test]
    fn server_reply_is_parsed_and_filtered() {
        let mine = "0123456789abcdef0123456789abcdef".to_string();
        let body = r#"{"answers": {
            "0123456789abcdef0123456789abcdef": {"s": "fixed", "text": " Готово ", "v": "0.6.1-beta.150", "at": "2026-11-26T20:00:00Z"},
            "ffffffffffffffffffffffffffffffff": {"s": "reply", "text": "чужой", "v": "", "at": "x"}
        }}"#;
        let got = parse(body.as_bytes(), std::slice::from_ref(&mine));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, mine);
        assert_eq!(got[0].1.status, Status::Fixed);
        assert_eq!(got[0].1.text, "Готово");
        assert_eq!(got[0].1.version, "0.6.1-beta.150");
        // Мусор вместо версии — без версии; неизвестный статус и не JSON — ничего.
        let odd =
            r#"{"answers": {"0123456789abcdef0123456789abcdef": {"s": "reply", "v": "<b>"}}}"#;
        assert_eq!(
            parse(odd.as_bytes(), std::slice::from_ref(&mine))[0]
                .1
                .version,
            ""
        );
        let bad = r#"{"answers": {"0123456789abcdef0123456789abcdef": {"s": "hack"}}}"#;
        assert!(parse(bad.as_bytes(), std::slice::from_ref(&mine)).is_empty());
        let other = "11111111111111111111111111111111".to_string();
        let mixed = r#"{"answers": {
            "0123456789abcdef0123456789abcdef": {"s": "hack"},
            "11111111111111111111111111111111": {"s": "reply", "text": "ок"}
        }}"#;
        assert_eq!(parse(mixed.as_bytes(), &[mine.clone(), other]).len(), 1);
        assert!(parse(b"not json", &[mine]).is_empty());
    }

    #[test]
    fn new_or_changed_answers_are_unseen_again() {
        let answer = Answer {
            status: Status::Fixed,
            text: String::new(),
            version: String::new(),
            at: "2026-11-26T20:00:00Z".into(),
        };
        let mut list = vec![message("a"), message("b")];
        let found = vec![("b".to_string(), answer.clone())];
        assert!(merge(&mut list, &found));
        assert_eq!(list[1].answer.as_ref(), Some(&answer));
        assert!(list[0].answer.is_none());
        list[1].seen = true;
        // Тот же ответ ещё раз — ничего не меняется, «видели» остаётся.
        assert!(!merge(&mut list, &found));
        assert!(list[1].seen);
        // Ответ поправили — снова новость.
        let fixed = Answer {
            at: "2026-11-27T09:00:00Z".into(),
            ..answer
        };
        assert!(merge(&mut list, &[("b".to_string(), fixed)]));
        assert!(!list[1].seen);
    }

    #[test]
    fn old_and_broken_messages_are_dropped() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join(format!("axiom-answers-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        let mut old = message("old");
        old.at = now() - KEEP_FOR - 10;
        let list = vec![old, message(""), message("fresh")];
        crate::storage::write_json(&dir.join(FILE), &list)?;
        let got = load(&dir);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].token, "fresh");
        fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
