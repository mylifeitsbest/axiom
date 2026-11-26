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

/// Спросить сервер об ответах на свежие сообщения и сохранить новые. Журнал помнит
/// запуски Axiom, который ещё не давал ключа, — заодно узнать, нет ли ответа на старое.
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
    // Старых запусков в журнале нет — и старых сообщений быть не может.
    static OLD: OnceLock<bool> = OnceLock::new();
    let old = *OLD.get_or_init(|| has_old(&runs(root)));
    if tokens.is_empty() && !old {
        return;
    }
    let Some(reply) = ask(&url, &tokens, old, &[]) else {
        return;
    };
    let mut changed = edit(|list| merge(list, &reply.answers));
    if !reply.old.is_empty() {
        let claims = find_claims(&runs(root), &reply.old);
        if !claims.is_empty()
            && let Some(got) = ask(&url, &[], false, &claims)
        {
            changed |= edit(|list| adopt(list, got.claimed));
        }
    }
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

/// Старые сообщения, которые браузер узнал, — в список, по времени отправки.
fn adopt(list: &mut Vec<Message>, claimed: Vec<Message>) -> bool {
    let mut changed = false;
    for m in claimed {
        if !list.iter().any(|x| x.token == m.token) {
            list.push(m);
            changed = true;
        }
    }
    list.sort_by_key(|m| m.at);
    changed
}

/// Что ответил сервер.
#[derive(Debug, Default)]
struct Reply {
    answers: Vec<(String, Answer)>,
    /// Ответы на старые сообщения (без ключа): по чему их узнать.
    old: Vec<Old>,
    /// Старые сообщения, которые этот браузер узнал: с ключом, текстом и ответом.
    claimed: Vec<Message>,
}

/// Ответ на старое сообщение: соль и хэши того, что есть только у отправителя.
#[derive(Debug, Clone, Deserialize)]
struct Old {
    id: u64,
    s: String,
    /// Сколько строк запуска подряд (0 — строка о системе и час отправки).
    k: usize,
    h: Vec<String>,
}

/// `POST /a {"t": [ключи], "old": true, "c": [заявки]}`.
fn ask(url: &str, tokens: &[String], old: bool, claims: &[(u64, String)]) -> Option<Reply> {
    let mut body = serde_json::json!({ "t": tokens });
    if old {
        body["old"] = true.into();
    }
    if !claims.is_empty() {
        body["c"] = claims
            .iter()
            .map(|(id, p)| serde_json::json!({ "id": id, "p": p }))
            .collect();
    }
    let body = serde_json::to_vec(&body).ok()?;
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
    let ids: Vec<u64> = claims.iter().map(|(id, _)| *id).collect();
    Some(parse(&out, tokens, &ids))
}

/// Разобрать ответ сервера: только спрошенные ключи и заявки, разумные поля.
fn parse(body: &[u8], tokens: &[String], claimed: &[u64]) -> Reply {
    use std::collections::BTreeMap;
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        answers: BTreeMap<String, serde_json::Value>,
        #[serde(default)]
        old: Vec<serde_json::Value>,
        #[serde(default)]
        claimed: BTreeMap<String, serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct RawAnswer {
        s: Status,
        #[serde(default)]
        text: String,
        #[serde(default)]
        v: String,
        #[serde(default)]
        at: String,
    }
    #[derive(Deserialize)]
    struct RawClaim {
        token: String,
        sent: String,
        msg: String,
        #[serde(flatten)]
        answer: RawAnswer,
    }
    fn answer(raw: RawAnswer) -> Option<Answer> {
        if raw.text.chars().count() > MAX_ANSWER_TEXT || raw.v.len() > 40 || raw.at.len() > 40 {
            return None;
        }
        Some(Answer {
            status: raw.s,
            text: raw.text.trim().to_string(),
            version: crate::update::Version::parse(&raw.v)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            at: raw.at,
        })
    }
    let Ok(raw) = serde_json::from_slice::<Raw>(body) else {
        return Reply::default();
    };
    // Что не разобрать (новый статус от будущего сервера), пропускаем по одному.
    let answers = raw
        .answers
        .into_iter()
        .filter(|(token, _)| tokens.contains(token))
        .filter_map(|(token, v)| Some((token, answer(serde_json::from_value(v).ok()?)?)))
        .collect();
    let old = raw
        .old
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Old>(v).ok())
        .filter(|o| {
            o.s.len() <= 64 && o.k <= 8 && o.h.len() <= 4 && o.h.iter().all(|h| h.len() == 64)
        })
        .take(64)
        .collect();
    let claimed = raw
        .claimed
        .into_iter()
        .filter(|(id, _)| id.parse().is_ok_and(|id| claimed.contains(&id)))
        .filter_map(|(_, v)| serde_json::from_value::<RawClaim>(v).ok())
        .filter(|c| c.token.len() == 32 && c.token.bytes().all(|b| b.is_ascii_hexdigit()))
        .filter_map(|c| {
            Some(Message {
                at: parse_utc(&c.sent).unwrap_or_else(now),
                text: c.msg.trim().chars().take(KEEP_TEXT).collect(),
                answer: Some(answer(c.answer)?),
                token: c.token,
                seen: false,
            })
        })
        .collect();
    Reply {
        answers,
        old,
        claimed,
    }
}

// ---------- старые сообщения: узнать своё по журналу ----------

/// С этой версии сообщение уходит с ключом ответа; у прежних его нет.
const TOKENS_SINCE: crate::update::Version = crate::update::Version::new(0, 6, 1, Some(151));
/// Сколько журнала читать (`axiom.log` растёт до 4 МБ, плюс `axiom.log.1`).
const JOURNAL_MAX: u64 = 12 * 1024 * 1024;
/// Час отправки ищем не дальше стольких часов от начала запуска.
const RUN_HOURS: u64 = 7 * 24;

/// Один запуск из журнала: строка о запуске, версия, время и строка о системе.
#[derive(Debug, PartialEq)]
struct Run {
    header: String,
    version: String,
    start: u64,
    system: Option<String>,
}

fn runs(root: &Path) -> Vec<Run> {
    parse_runs(&super::journal::tail(root, JOURNAL_MAX))
}

/// Запуски по строкам «=== Axiom 0.6.1-beta.148 · windows · 2026-11-26 18:39:13 UTC ===».
fn parse_runs(journal: &str) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for line in journal.lines().map(str::trim) {
        if let Some(run) = parse_header(line) {
            out.push(run);
        } else if let (Some(run), Some(system)) = (out.last_mut(), line.strip_prefix("система: "))
            && run.system.is_none()
        {
            run.system = Some(system.to_string());
        }
    }
    out
}

fn parse_header(line: &str) -> Option<Run> {
    let inner = line.strip_prefix("=== Axiom ")?.strip_suffix(" ===")?;
    let mut parts = inner.split(" · ");
    let (version, _os, time) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || version.is_empty() || !time.ends_with(" UTC") {
        return None;
    }
    Some(Run {
        header: line.to_string(),
        version: version.to_string(),
        start: parse_utc(time)?,
        system: None,
    })
}

/// В журнале есть запуски Axiom, который ещё не давал ключа ответа.
fn has_old(runs: &[Run]) -> bool {
    runs.iter()
        .any(|r| crate::update::Version::parse(&r.version).is_some_and(|v| v < TOKENS_SINCE))
}

/// Сверить хэши старых ответов со своим журналом. Совпало — заявка `(номер, прообраз)`.
/// Отчёт очищали перед отправкой — так же очищаем и свои строки.
fn find_claims(runs: &[Run], old: &[Old]) -> Vec<(u64, String)> {
    let scrub = |s: &str| match super::STATE.get() {
        Some(state) => state.private.scrub(s),
        None => s.to_string(),
    };
    let heads: Vec<String> = runs.iter().map(|r| scrub(&r.header)).collect();
    // Строка о системе того запуска и все часы, пока он шёл.
    let mut abouts: Vec<String> = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        let Some(system) = &run.system else { continue };
        let about = scrub(&format!("Axiom {} · {system}", run.version));
        let end = runs.get(i + 1).map_or_else(now, |next| next.start);
        let (first, last) = (run.start / 3600, end.max(run.start) / 3600);
        for hour in first..=last.min(first + RUN_HOURS) {
            abouts.push(format!("a|{about}|{}", hour_key(hour * 3600)));
        }
    }
    let mut claims = Vec::new();
    for entry in old {
        let found = if entry.k > 0 {
            heads
                .windows(entry.k)
                .map(|w| format!("h|{}", w.join("\n")))
                .find(|p| entry.h.contains(&fp_hash(&entry.s, p)))
        } else {
            abouts
                .iter()
                .find(|p| entry.h.contains(&fp_hash(&entry.s, p)))
                .cloned()
        };
        if let Some(p) = found {
            claims.push((entry.id, p));
        }
        if claims.len() == 2 {
            break;
        }
    }
    claims
}

fn fp_hash(salt: &str, preimage: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{salt}{preimage}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `2026-11-26 18:39:13 UTC` или `2026-11-26T18:39:13Z` → секунды Unix.
fn parse_utc(s: &str) -> Option<u64> {
    let n: Vec<u64> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let [y, mo, d, h, mi, sec] = n[..] else {
        return None;
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Дни от 1970-01-01 по григорианскому календарю (алгоритм Хиннанта).
    let y = y as i64 - i64::from(mo <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((mo as i64 + 9) % 12) + 2) / 5 + d as i64 - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from(days * 86_400 + (h * 3600 + mi * 60 + sec) as i64).ok()
}

/// Час как на сервере: `2026-11-26T18`.
fn hour_key(secs: u64) -> String {
    let (y, m, d) = crate::stats::civil((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}", secs % 86_400 / 3600)
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
        let answers = |body: &str, tokens: &[String]| parse(body.as_bytes(), tokens, &[]).answers;
        let got = answers(body, std::slice::from_ref(&mine));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, mine);
        assert_eq!(got[0].1.status, Status::Fixed);
        assert_eq!(got[0].1.text, "Готово");
        assert_eq!(got[0].1.version, "0.6.1-beta.150");
        // Мусор вместо версии — без версии; неизвестный статус и не JSON — ничего.
        let odd =
            r#"{"answers": {"0123456789abcdef0123456789abcdef": {"s": "reply", "v": "<b>"}}}"#;
        assert_eq!(answers(odd, std::slice::from_ref(&mine))[0].1.version, "");
        let bad = r#"{"answers": {"0123456789abcdef0123456789abcdef": {"s": "hack"}}}"#;
        assert!(answers(bad, std::slice::from_ref(&mine)).is_empty());
        let other = "11111111111111111111111111111111".to_string();
        let mixed = r#"{"answers": {
            "0123456789abcdef0123456789abcdef": {"s": "hack"},
            "11111111111111111111111111111111": {"s": "reply", "text": "ок"}
        }}"#;
        assert_eq!(answers(mixed, &[mine.clone(), other]).len(), 1);
        assert!(answers("not json", &[mine]).is_empty());
    }

    const JOURNAL: &str = "\n=== Axiom 0.6.0-beta.119 · windows · 2026-11-26 13:53:06 UTC ===\n\
        установленная копия обновлена\n\
        \n=== Axiom 0.6.1-beta.137 · windows · 2026-11-26 18:13:28 UTC ===\n\
        система: Windows 11 (сборка 26200, 25H2) · WebView2 158\n\
        прочее\n\
        \n=== журнал продолжен · 2026-11-26 18:20:00 UTC ===\n\
        \n=== Axiom 0.6.1-beta.148 · windows · 2026-11-26 20:39:13 UTC ===\n\
        система: Windows 11 (сборка 26200, 25H2) · WebView2 158\n";

    #[test]
    fn journal_runs_are_read() {
        let runs = parse_runs(JOURNAL);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].version, "0.6.0-beta.119");
        assert_eq!(runs[0].system, None);
        assert_eq!(
            runs[1].start,
            parse_utc("2026-11-26T18:13:28Z").unwrap_or(0)
        );
        assert_eq!(
            runs[2].system.as_deref(),
            Some("Windows 11 (сборка 26200, 25H2) · WebView2 158")
        );
        assert!(has_old(&runs));
        assert!(!has_old(&parse_runs(
            "=== Axiom 0.6.1-beta.152 · linux · 2026-11-27 10:00:00 UTC ==="
        )));
    }

    #[test]
    fn utc_times_round_trip() {
        assert_eq!(parse_utc("1970-01-01 00:00:00 UTC"), Some(0));
        assert_eq!(parse_utc("2025-11-25 19:03:12 UTC"), Some(1_764_097_392));
        assert_eq!(parse_utc("2025-11-25T19:03:12Z"), Some(1_764_097_392));
        assert_eq!(
            parse_utc("2024-02-29 00:00:00 UTC").map(super::super::journal::utc),
            Some("2024-02-29 00:00:00 UTC".to_string())
        );
        assert_eq!(parse_utc("2026-13-01 00:00:00 UTC"), None);
        assert_eq!(parse_utc("вчера"), None);
        assert_eq!(hour_key(1_764_097_392), "2025-11-25T19");
    }

    /// Так же, как сервер (server/stats.py, old_fingerprints).
    fn entry(id: u64, k: usize, preimage: &str) -> Old {
        let s = format!("salt{id}");
        Old {
            id,
            k,
            h: vec![fp_hash(&s, preimage)],
            s,
        }
    }

    #[test]
    fn own_old_messages_are_found_in_the_journal() {
        let runs = parse_runs(JOURNAL);
        let heads: Vec<&str> = runs.iter().map(|r| r.header.as_str()).collect();
        // С журналом — последние строки запуска подряд; без — система и час отправки.
        let by_heads = format!("h|{}", heads[1..3].join("\n"));
        let by_hour =
            "a|Axiom 0.6.1-beta.137 · Windows 11 (сборка 26200, 25H2) · WebView2 158|2026-11-26T19";
        let old = [
            entry(5, 2, &by_heads),
            entry(6, 0, by_hour),
            // Чужое: другой порядок запусков и час, когда этот запуск уже кончился.
            entry(7, 2, &format!("h|{}\n{}", heads[2], heads[1])),
            entry(8, 0, &by_hour.replace("T19", "T21")),
        ];
        let claims = find_claims(&runs, &old);
        assert_eq!(
            claims,
            vec![(5, by_heads.clone()), (6, by_hour.to_string())]
        );
        assert!(find_claims(&runs, &old[2..]).is_empty());
        assert!(find_claims(&parse_runs(""), &old).is_empty());
    }

    #[test]
    fn claimed_old_messages_need_a_claim_and_a_real_token() {
        let body = r#"{"claimed": {
            "5": {"token": "0123456789abcdef0123456789abcdef", "sent": "2026-11-26T18:20:05Z",
                  "msg": " Добавьте «Что нового» ", "s": "fixed", "text": "Сделали", "v": "0.6.1-beta.152", "at": "x"},
            "6": {"token": "short", "sent": "2026-11-26T18:20:05Z", "msg": "", "s": "fixed"},
            "9": {"token": "ffffffffffffffffffffffffffffffff", "sent": "2026-11-26T18:20:05Z", "msg": "", "s": "fixed"}
        }, "old": [{"id": 3, "s": "abc", "k": 3, "h": ["x"]},
                   {"id": 4, "s": "abc", "k": 0, "h": ["0000000000000000000000000000000000000000000000000000000000000000"]}]}"#;
        let reply = parse(body.as_bytes(), &[], &[5, 6]);
        assert_eq!(reply.claimed.len(), 1);
        let m = &reply.claimed[0];
        assert_eq!(m.text, "Добавьте «Что нового»");
        assert_eq!(m.at, parse_utc("2026-11-26T18:20:05Z").unwrap_or(0));
        assert_eq!(
            m.answer.as_ref().map(|a| a.version.as_str()),
            Some("0.6.1-beta.152")
        );
        // Хэш не той длины — запись пропускается.
        assert_eq!(reply.old.len(), 1);
        assert_eq!(reply.old[0].id, 4);
        let mut list = vec![message("a")];
        assert!(adopt(&mut list, reply.claimed));
        assert_eq!(list[0].token, "0123456789abcdef0123456789abcdef");
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
