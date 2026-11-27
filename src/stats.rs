//! Статистика использования: раз в час Axiom шлёт на наш сервер всего несколько
//! байт — версию, систему и флаги «первый запуск за день/неделю/месяц», «новая
//! установка», «только что обновился», «перенёс данные из другого браузера»
//! (один раз на профиль, без названия браузера и без того, что перенесено) и
//! «в счёт часа» — по нему сервер считает, сколько браузеров открыто сейчас.
//! Ни идентификатора, ни истории, ни адресов; IP на сервере не сохраняется.
//! Этого хватает, чтобы видеть DAU/WAU/MAU, установки, обновления и переезды,
//! но по данным нельзя узнать конкретного человека.
//!
//! Об этом говорит приветствие первого запуска и PRIVACY.md, а выключается всё
//! в Настройки → Система → «Статистика использования» ([`set_enabled`]):
//! выключено — не уходит ни одной отметки, и импорт не запоминается.
//!
//! Сеть — через системный `curl`, как и проверка обновлений: своего HTTP-клиента
//! в браузере нет. Любая ошибка просто глотается: статистика не должна мешать.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU8, Ordering},
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};

/// Куда слать. В debug-сборке адрес можно подменить (`AXIOM_STATS_URL=...`),
/// чтобы проверить всё на локальном сервере. В release менять нельзя.
pub(crate) const STATS_URL: &str = "https://179-254-172-240.sslip.io";

/// Как часто отмечаться, пока браузер открыт.
const EVERY: Duration = Duration::from_secs(60 * 60);
/// Как часто проверять, не пора ли. Один долгий `sleep` не годится: на Windows
/// время в спящем режиме в таймаут не входит, и после пробуждения ноутбука
/// первая отметка за день откладывалась бы почти на час.
const POLL: Duration = Duration::from_secs(60);
/// Первый пинг — не сразу при старте, чтобы не толкаться с остальной загрузкой.
const WARMUP: Duration = Duration::from_secs(20);

const SECS_PER_DAY: u64 = 86_400;

/// Что лежит в `stats.json` рядом с профилем: когда в последний раз отметились.
/// Строки, а не числа, — чтобы файл можно было прочитать глазами.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    day: String,
    week: String,
    month: String,
    /// Новая установка, о которой сервер ещё не узнал.
    #[serde(default)]
    new: bool,
    /// Версия, с которой отметились в последний раз. Пусто — файл записала
    /// версия до 0.3.4, где этого поля ещё не было.
    #[serde(default)]
    ver: String,
    /// Переносили ли сюда данные из другого браузера.
    #[serde(default)]
    import: Import,
    /// Профиль уже проверили на импорт, сделанный до этого счётчика.
    #[serde(default)]
    scanned: bool,
    /// Когда (секунды UTC) отправили последнюю отметку «в счёт часа».
    /// 0 — ещё ни разу (или файл записала версия до этого поля).
    #[serde(default)]
    at: u64,
}

/// Импорт считается один раз на профиль: сервер видит, сколько людей переехало,
/// а не сколько раз жали кнопку.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Import {
    #[default]
    No,
    /// Перенесли, сервер ещё не знает.
    Pending,
    /// Переносили ещё до счётчика (нашлось в профиле), сервер ещё не знает.
    /// Идёт отдельным флагом: иначе в день релиза «переехали сегодня» показало
    /// бы всех, кто переезжал за прошлые недели.
    Earlier,
    /// Сервер уже посчитал.
    Sent,
}

/// 2026-11-23 00:00 UTC — до этого Axiom не было (0.1.0 вышла 23 ноября), так
/// что посещение старше в истории могло попасть туда только импортом.
const AXIOM_BORN: u64 = 1_795_392_000;

/// `stats.json` этого профиля; пусто, пока счётчик выключен (debug-сборка,
/// `AXIOM_NO_STATS`) — тогда и импорт не отмечаем.
static FILE: OnceLock<PathBuf> = OnceLock::new();
/// Файл пишут два потока: счётчик и импорт. Без замка отметка об импорте,
/// сделанная, пока идёт отправка, затёрлась бы.
static LOCK: Mutex<()> = Mutex::new(());
/// Включена ли статистика в настройках: 0 — настройки ещё не прочитаны,
/// 1 — выключена, 2 — включена. Пока 0, поток счётчика ждёт и ничего не шлёт.
static ENABLED: AtomicU8 = AtomicU8::new(0);

/// Метка в `ver` для профиля, который был до счётчика (0.3.0 и раньше):
/// это тоже обновление, хотя прошлой отметки нет.
const VER_BEFORE_STATS: &str = "old";

/// Версия, которой отмечаемся: `0.6.3` или у беты `0.6.3-beta.190`. Бета со своим
/// номером видна на панели отдельно, а переход на следующую бету — обновление.
fn version() -> &'static str {
    crate::update::current_string()
}

/// Что сообщить серверу при этой отметке.
#[derive(Debug, PartialEq)]
struct Flags {
    /// Новая установка.
    new: bool,
    /// Первая отметка за день / неделю / месяц.
    d: bool,
    w: bool,
    m: bool,
    /// Первая отметка на этой версии — значит, только что обновились.
    up: bool,
    /// Первая отметка за день на этой версии: по ней сервер считает, сколько
    /// людей сегодня на каждой версии (после обновления `d` уже false).
    vd: bool,
    /// Перенесли данные из другого браузера.
    im: bool,
    /// Переносили ещё до счётчика.
    imo: bool,
    /// Отметка «в счёт часа»: сервер складывает только такие, когда считает,
    /// сколько браузеров открыто. Открытый браузер даёт одну такую в час, а
    /// отметка при перезапуске (обновление, вылет, закрыл и открыл) — нет.
    o: bool,
}

/// Пора ли отметиться «в счёт часа»: прошлая такая отметка была час назад или
/// раньше. `now < at` — часы перевели назад, считаем, что пора.
fn online(at: u64, now: u64) -> bool {
    at == 0 || now < at || now - at >= EVERY.as_secs()
}

/// Первая ли это отметка после обновления до `current`. Бета на бету (и бета,
/// отмечавшаяся до 0.6.3-beta.N номером выпуска, на свой номер) — тоже обновление.
fn is_update(prev: &State, current: &str) -> bool {
    match prev.ver.as_str() {
        // Старый формат файла: если в нём уже есть отметка — его писала
        // предыдущая версия, то есть сейчас первая отметка после обновления.
        "" => !prev.day.is_empty(),
        v => v != current,
    }
}

fn flags(prev: &State, now: u64, day: &str, week: &str, month: &str) -> Flags {
    let up = is_update(prev, version());
    let d = prev.day != day;
    Flags {
        new: prev.new,
        d,
        w: prev.week != week,
        m: prev.month != month,
        up,
        vd: d || up,
        im: prev.import == Import::Pending,
        imo: prev.import == Import::Earlier,
        o: online(prev.at, now),
    }
}

/// Был ли импорт до этого счётчика. Видно по самим данным: свои закладки
/// (звёздочка) Axiom кладёт без папки, а импорт — всегда в папку («Панель
/// закладок», «Другие закладки»…); посещений раньше [`AXIOM_BORN`] сам браузер
/// записать не мог. Читается только локально, на сервер уходит один флаг.
fn imported_before(root: &Path) -> bool {
    #[derive(Deserialize)]
    struct Mark {
        #[serde(default)]
        folder: String,
    }
    #[derive(Deserialize)]
    struct Visit {
        #[serde(default)]
        last: u64,
    }
    let marks: Vec<Mark> =
        crate::storage::read_json(&root.join("bookmarks.json")).unwrap_or_default();
    if marks.iter().any(|m| !m.folder.is_empty()) {
        return true;
    }
    let visits: Vec<Visit> =
        crate::storage::read_json(&root.join("history.json")).unwrap_or_default();
    visits.iter().any(|v| v.last > 0 && v.last < AXIOM_BORN)
}

/// Один раз на профиль: поискать импорт, сделанный до счётчика.
fn scan(file: &Path) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = crate::storage::read_json::<State>(file).unwrap_or_default();
    if state.scanned {
        return;
    }
    if state.import == Import::No && file.parent().is_some_and(imported_before) {
        state.import = Import::Earlier;
    }
    state.scanned = true;
    let _ = crate::storage::write_json(file, &state);
}

/// Включить или выключить статистику (настройка «Статистика использования»).
/// Вызывается, как только прочитаны настройки, и при каждом их сохранении.
pub fn set_enabled(on: bool) {
    ENABLED.store(if on { 2 } else { 1 }, Ordering::Relaxed);
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) == 2
}

/// Отметить, что в профиль перенесли данные из другого браузера. Уйдёт со
/// следующей часовой отметкой; повторный импорт не считается.
pub fn imported() {
    let Some(file) = FILE.get() else {
        return;
    };
    if !enabled() {
        return;
    }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = crate::storage::read_json::<State>(file).unwrap_or_default();
    if state.import == Import::No {
        state.import = Import::Pending;
        let _ = crate::storage::write_json(file, &state);
    }
}

/// Файлы, которые есть у любого, кто уже пользовался браузером.
const USED_PROFILE: [&str; 3] = ["settings.json", "session.json", "history.json"];

/// Запустить фоновый поток счётчика. Вызывать один раз, у главного экземпляра.
pub fn start(profile_root: &Path) {
    // В debug молчим, пока адрес не подменили вручную.
    #[cfg(debug_assertions)]
    if std::env::var_os("AXIOM_STATS_URL").is_none() {
        return;
    }
    // Для тестовых запусков release-сборки: не засорять статистику.
    if std::env::var_os("AXIOM_NO_STATS").is_some_and(|v| !v.is_empty() && v != "0") {
        return;
    }
    let file = profile_root.join("stats.json");
    // Решаем до того, как браузер создаст свои файлы: нет счётчика и нет профиля —
    // новая установка. Профиль есть, а счётчика нет — обновились со старой версии.
    if !file.exists() {
        let used = USED_PROFILE.iter().any(|f| profile_root.join(f).exists());
        let state = State {
            new: !used,
            ver: if used {
                VER_BEFORE_STATS.into()
            } else {
                String::new()
            },
            // В новом профиле искать нечего.
            scanned: !used,
            ..State::default()
        };
        let _ = crate::storage::write_json(&file, &state);
    }
    let _ = FILE.set(file.clone());
    thread::spawn(move || {
        thread::sleep(WARMUP);
        // Выключена (или настройки ещё не прочитаны) — ждём, не трогая сеть.
        while !enabled() {
            thread::sleep(POLL);
        }
        scan(&file);
        // last — от какого времени отсчитывать час и день прошлой отметки. Час
        // идёт от последней отметки «в счёт часа», даже если её отправил прошлый
        // запуск: после перезапуска браузер не начинает свой час заново.
        let mut last: Option<(u64, String)> = None;
        loop {
            let now = now_secs();
            let day = buckets(now).0;
            let due = last.as_ref().is_none_or(|(at, last_day)| {
                *last_day != day || now < *at || now - at >= EVERY.as_secs()
            });
            if due && enabled() {
                let at = tick(&file, now_secs(), send);
                last = Some((at, day));
            }
            thread::sleep(POLL);
        }
    });
}

/// Один цикл: вычислить флаги, отправить, обновить файл состояния. Возвращает,
/// от какого времени ждать следующий час.
fn tick(file: &Path, now: u64, send: impl FnOnce(&str) -> Result<(), ()>) -> u64 {
    let (day, week, month) = buckets(now);

    let prev = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::storage::read_json::<State>(file).unwrap_or_default()
    };
    let f = flags(&prev, now, &day, &week, &month);
    // Не в счёт часа — значит, тот час ещё идёт: следующая отметка в его конце.
    let at = if f.o { now } else { prev.at };

    let body = format!(
        "{{\"v\":\"{}\",\"os\":\"{}\",\"new\":{},\"d\":{},\"w\":{},\"m\":{},\"up\":{},\"vd\":{},\"im\":{},\"imo\":{},\"o\":{}}}",
        version(),
        os_name(),
        f.new,
        f.d,
        f.w,
        f.m,
        f.up,
        f.vd,
        f.im,
        f.imo,
        f.o,
    );

    if send(&body).is_ok() {
        // Записываем только после удачной отправки: иначе при обрыве связи мы бы
        // «потеряли» этот день и не отметились бы, когда сеть вернётся.
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Импорт мог случиться, пока шла отправка: берём свежее значение.
        let cur = crate::storage::read_json::<State>(file).unwrap_or(prev);
        let state = State {
            day,
            week,
            month,
            new: false,
            ver: version().into(),
            import: if f.im || f.imo {
                Import::Sent
            } else {
                cur.import
            },
            scanned: cur.scanned,
            at,
        };
        let _ = crate::storage::write_json(file, &state);
    }
    // Не дошла — следующая попытка через час, как и раньше, а не каждую минуту.
    at
}

/// Ключи «день / неделя / месяц» по времени UTC. Недели считаем с понедельника
/// (1970-01-01 — четверг, отсюда сдвиг +3). Сравниваются только между собой,
/// на сервер уходят лишь булевы флаги.
fn buckets(now: u64) -> (String, String, String) {
    let days = (now / SECS_PER_DAY) as i64;
    let (y, mo, d) = civil(days);
    let day = format!("{y:04}-{mo:02}-{d:02}");
    let week = format!("w{}", (days + 3).div_euclid(7));
    let month = format!("{y:04}-{mo:02}");
    (day, week, month)
}

/// Григорианская дата из числа дней с 1970-01-01 (алгоритм Хиннанта).
pub(crate) fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub(crate) fn os_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "other"
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Базовый адрес. В debug его можно подменить локальным сервером.
/// Им же пользуются отчёты об ошибках (`reports`).
pub(crate) fn base_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("AXIOM_STATS_URL") {
        let url = url.trim_end_matches('/');
        if url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:") {
            return url.to_string();
        }
    }
    STATS_URL.to_string()
}

/// POST тела на `{base}/p`. Возвращает ошибку при любом сбое — вызывающий её
/// просто игнорирует и попробует снова через час.
fn send(body: &str) -> Result<(), ()> {
    let url = format!("{}/p", base_url());
    let mut cmd = curl();
    // --fail: ответ 5xx — тоже неудача, иначе день записался бы как отмеченный.
    cmd.args([
        "--fail",
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
    ])
    .arg("--data")
    .arg(body)
    .arg(&url);
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(()),
        _ => Err(()),
    }
}

pub(crate) fn curl() -> Command {
    #[cfg(windows)]
    let mut cmd = {
        use std::{os::windows::process::CommandExt, path::PathBuf};
        let exe = std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join(r"System32\curl.exe"))
            .unwrap_or_else(|| PathBuf::from("curl.exe"));
        let mut cmd = Command::new(exe);
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new("curl");
    cmd.args([
        "-q",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "10",
        "--max-time",
        "20",
    ])
    .arg("--user-agent")
    .arg(concat!("Axiom/", env!("CARGO_PKG_VERSION")))
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    if base_url().starts_with("https://") {
        cmd.args(["--proto", "=https", "--proto-redir", "=https"]);
    }
    cmd
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// «Сейчас» в тестах; прошлая отметка в счёт часа — за 10 минут до него.
    const T: u64 = 1_795_500_000;

    fn state(day: &str, ver: &str) -> State {
        State {
            day: day.into(),
            week: "w1".into(),
            month: "2026-11".into(),
            new: false,
            ver: ver.into(),
            import: Import::No,
            scanned: true,
            at: T - 600,
        }
    }

    fn profile(name: &str, bookmarks: &str, history: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axiom-stats-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bookmarks.json"), bookmarks).unwrap();
        std::fs::write(dir.join("history.json"), history).unwrap();
        dir
    }

    #[test]
    fn finds_import_made_before_the_counter() {
        let star = r#"[{"url":"https://a.example/","title":"A","folder":"","added":1795500000}]"#;
        let fresh = r#"[{"url":"https://a.example/","title":"A","visits":3,"last":1795500000}]"#;

        let own = profile("own", star, fresh);
        assert!(
            !imported_before(&own),
            "свои закладки и свежая история — не импорт"
        );

        let marks = profile(
            "marks",
            r#"[{"url":"https://b.example/","title":"B","folder":"Панель закладок","added":1600000000}]"#,
            "[]",
        );
        assert!(imported_before(&marks));

        let old = profile(
            "old",
            star,
            r#"[{"url":"https://c.example/","title":"C","visits":1,"last":1790000000}]"#,
        );
        assert!(imported_before(&old));

        let empty = std::env::temp_dir().join(format!("axiom-stats-none-{}", std::process::id()));
        assert!(!imported_before(&empty), "нет файлов — нет импорта");

        for dir in [own, marks, old] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn scan_marks_earlier_import_once() {
        let dir = profile(
            "scan",
            r#"[{"url":"https://b.example/","title":"B","folder":"Другие закладки"}]"#,
            "[]",
        );
        let file = dir.join("stats.json");
        let mut s = state("2026-11-25", version());
        s.scanned = false;
        crate::storage::write_json(&file, &s).unwrap();
        scan(&file);
        let s: State = crate::storage::read_json(&file).unwrap();
        assert_eq!(s.import, Import::Earlier);
        assert!(s.scanned);
        let f = flags(&s, T, "2026-11-25", "w1", "2026-11");
        assert!(f.imo && !f.im);

        // Уже посчитанный профиль второй раз не всплывает.
        let mut s = s;
        s.import = Import::Sent;
        s.scanned = false;
        crate::storage::write_json(&file, &s).unwrap();
        scan(&file);
        let s: State = crate::storage::read_json(&file).unwrap();
        assert_eq!(s.import, Import::Sent);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fresh_install_is_new_not_update() {
        let prev = State {
            new: true,
            ..State::default()
        };
        let f = flags(&prev, T, "2026-11-24", "w1", "2026-11");
        assert!(f.new && f.d && f.w && f.m && f.vd);
        assert!(!f.up);
    }

    #[test]
    fn same_version_later_today_sends_nothing_new() {
        let f = flags(
            &state("2026-11-24", version()),
            T,
            "2026-11-24",
            "w1",
            "2026-11",
        );
        assert_eq!(
            f,
            Flags {
                new: false,
                d: false,
                w: false,
                m: false,
                up: false,
                vd: false,
                im: false,
                imo: false,
                o: false
            }
        );
    }

    #[test]
    fn import_is_sent_once() {
        let mut prev = state("2026-11-24", version());
        prev.import = Import::Pending;
        assert!(flags(&prev, T, "2026-11-24", "w1", "2026-11").im);
        prev.import = Import::Sent;
        assert!(!flags(&prev, T, "2026-11-24", "w1", "2026-11").im);
    }

    #[test]
    fn file_without_import_field_means_no_import() {
        let s: State = serde_json::from_str(
            r#"{"day":"2026-11-24","week":"w1","month":"2026-11","new":false,"ver":"0.5.4"}"#,
        )
        .unwrap();
        assert_eq!(s.import, Import::No);
        let s: State =
            serde_json::from_str(r#"{"day":"","week":"","month":"","import":"pending"}"#).unwrap();
        assert_eq!(s.import, Import::Pending);
    }

    #[test]
    fn update_on_the_same_day_counts_for_the_new_version() {
        let f = flags(
            &state("2026-11-24", "0.0.1"),
            T,
            "2026-11-24",
            "w1",
            "2026-11",
        );
        assert!(f.up && f.vd);
        assert!(!f.d, "человек уже отметился сегодня на старой версии");
    }

    #[test]
    fn beta_number_is_its_own_version() {
        let on = |ver: &str| state("2026-11-27", ver);
        assert!(
            is_update(&on("0.6.3"), "0.6.3-beta.190"),
            "бета шла номером выпуска"
        );
        assert!(is_update(&on("0.6.3-beta.190"), "0.6.3-beta.191"));
        assert!(is_update(&on("0.6.3-beta.191"), "0.6.4"));
        assert!(!is_update(&on("0.6.3-beta.191"), "0.6.3-beta.191"));
    }

    #[test]
    fn file_from_before_the_ver_field_means_update() {
        let f = flags(&state("2026-11-23", ""), T, "2026-11-24", "w1", "2026-11");
        assert!(f.up && f.d && f.vd);
    }

    #[test]
    fn open_browser_counts_once_an_hour() {
        assert!(online(0, T), "первая отметка в жизни профиля");
        assert!(!online(T - 600, T), "перезапуск через 10 минут");
        assert!(!online(T - 3599, T));
        assert!(online(T - 3600, T), "прошёл час");
        assert!(online(T + 600, T), "часы перевели назад");
    }

    #[test]
    fn restart_within_the_hour_is_not_counted_but_still_reports() {
        // Обновился через 20 минут после отметки: «обновился» уходит сразу,
        // а в счёт часа — нет, человек уже посчитан.
        let mut prev = state("2026-11-24", "0.0.1");
        prev.at = T - 1200;
        let f = flags(&prev, T, "2026-11-24", "w1", "2026-11");
        assert!(f.up && !f.o);
        prev.at = T - 3700;
        assert!(flags(&prev, T, "2026-11-24", "w1", "2026-11").o);
    }

    #[test]
    fn restart_keeps_the_hour_of_the_previous_run() {
        let dir = std::env::temp_dir().join(format!("axiom-stats-tick-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("stats.json");
        let _ = std::fs::remove_file(&file);
        let sent = std::cell::RefCell::new(Vec::<String>::new());
        let ok = |body: &str| {
            sent.borrow_mut().push(body.to_string());
            Ok(())
        };

        // Первый запуск: в счёт часа.
        assert_eq!(tick(&file, T, ok), T);
        // Перезапуск через 20 минут: отметка уходит, но не в счёт, и следующую
        // надо слать через час от первой, а не от перезапуска.
        assert_eq!(tick(&file, T + 1200, ok), T);
        // Связь пропала ровно к концу часа: следующая попытка — через час.
        assert_eq!(tick(&file, T + 3600, |_| Err(())), T + 3600);
        // Вернулась: прошлая отметка в счёт была больше часа назад — в счёт.
        assert_eq!(tick(&file, T + 7200, ok), T + 7200);

        let sent = sent.into_inner();
        assert_eq!(sent.len(), 3);
        // Бета-сборка (AXIOM_BETA_BUILD=N) отмечается своим номером.
        let v = format!(r#"{{"v":"{}","#, crate::update::current_string());
        assert!(sent[0].starts_with(&v), "{}", sent[0]);
        assert!(sent[0].ends_with(r#""o":true}"#), "{}", sent[0]);
        assert!(sent[1].ends_with(r#""o":false}"#), "{}", sent[1]);
        assert!(sent[2].ends_with(r#""o":true}"#), "{}", sent[2]);
        let s: State = crate::storage::read_json(&file).unwrap();
        assert_eq!(s.at, T + 7200);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn file_without_at_field_counts_right_away() {
        let s: State = serde_json::from_str(
            r#"{"day":"2026-11-24","week":"w1","month":"2026-11","ver":"0.6.1"}"#,
        )
        .unwrap();
        assert_eq!(s.at, 0);
        assert!(flags(&s, T, "2026-11-24", "w1", "2026-11").o);
    }

    #[test]
    fn profile_from_before_stats_means_update() {
        let prev = State {
            ver: VER_BEFORE_STATS.into(),
            ..State::default()
        };
        let f = flags(&prev, T, "2026-11-24", "w1", "2026-11");
        assert!(f.up && !f.new);
    }
}
