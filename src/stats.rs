//! Тихий счётчик: раз в час Axiom шлёт на наш сервер всего несколько байт —
//! версию, систему и флаги «первый запуск за день/неделю/месяц», «новая
//! установка» и «только что обновился». Ни идентификатора, ни истории, ни
//! адресов; IP на сервере не сохраняется. Этого хватает, чтобы видеть
//! DAU/WAU/MAU, установки и обновления, но по данным нельзя узнать конкретного
//! человека.
//!
//! Сеть — через системный `curl`, как и проверка обновлений: своего HTTP-клиента
//! в браузере нет. Любая ошибка просто глотается: статистика не должна мешать.

use std::{
    path::Path,
    process::{Command, Stdio},
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
}

/// Метка в `ver` для профиля, который был до счётчика (0.3.0 и раньше):
/// это тоже обновление, хотя прошлой отметки нет.
const VER_BEFORE_STATS: &str = "old";

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
}

fn flags(prev: &State, day: &str, week: &str, month: &str) -> Flags {
    let up = match prev.ver.as_str() {
        // Старый формат файла: если в нём уже есть отметка — его писала
        // предыдущая версия, то есть сейчас первая отметка после обновления.
        "" => !prev.day.is_empty(),
        v => v != VERSION,
    };
    let d = prev.day != day;
    Flags {
        new: prev.new,
        d,
        w: prev.week != week,
        m: prev.month != month,
        up,
        vd: d || up,
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
            ..State::default()
        };
        let _ = crate::storage::write_json(&file, &state);
    }
    thread::spawn(move || {
        thread::sleep(WARMUP);
        let mut last: Option<(u64, String)> = None;
        loop {
            let now = now_secs();
            let day = buckets(now).0;
            let due = last.as_ref().is_none_or(|(at, last_day)| {
                *last_day != day || now < *at || now - at >= EVERY.as_secs()
            });
            if due {
                tick(&file);
                last = Some((now, day));
            }
            thread::sleep(POLL);
        }
    });
}

/// Один цикл: вычислить флаги, отправить, обновить файл состояния.
fn tick(file: &Path) {
    let now = now_secs();
    let (day, week, month) = buckets(now);

    let prev = crate::storage::read_json::<State>(file).unwrap_or_default();
    let f = flags(&prev, &day, &week, &month);

    let body = format!(
        "{{\"v\":\"{}\",\"os\":\"{}\",\"new\":{},\"d\":{},\"w\":{},\"m\":{},\"up\":{},\"vd\":{}}}",
        VERSION,
        os_name(),
        f.new,
        f.d,
        f.w,
        f.m,
        f.up,
        f.vd,
    );

    if send(&body).is_ok() {
        // Записываем только после удачной отправки: иначе при обрыве связи мы бы
        // «потеряли» этот день и не отметились бы, когда сеть вернётся.
        let state = State {
            day,
            week,
            month,
            new: false,
            ver: VERSION.into(),
        };
        let _ = crate::storage::write_json(file, &state);
    }
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
mod tests {
    use super::*;

    fn state(day: &str, ver: &str) -> State {
        State {
            day: day.into(),
            week: "w1".into(),
            month: "2026-11".into(),
            new: false,
            ver: ver.into(),
        }
    }

    #[test]
    fn fresh_install_is_new_not_update() {
        let prev = State {
            new: true,
            ..State::default()
        };
        let f = flags(&prev, "2026-11-24", "w1", "2026-11");
        assert!(f.new && f.d && f.w && f.m && f.vd);
        assert!(!f.up);
    }

    #[test]
    fn same_version_later_today_sends_nothing_new() {
        let f = flags(&state("2026-11-24", VERSION), "2026-11-24", "w1", "2026-11");
        assert_eq!(
            f,
            Flags {
                new: false,
                d: false,
                w: false,
                m: false,
                up: false,
                vd: false
            }
        );
    }

    #[test]
    fn update_on_the_same_day_counts_for_the_new_version() {
        let f = flags(&state("2026-11-24", "0.0.1"), "2026-11-24", "w1", "2026-11");
        assert!(f.up && f.vd);
        assert!(!f.d, "человек уже отметился сегодня на старой версии");
    }

    #[test]
    fn file_from_before_the_ver_field_means_update() {
        let f = flags(&state("2026-11-23", ""), "2026-11-24", "w1", "2026-11");
        assert!(f.up && f.d && f.vd);
    }

    #[test]
    fn profile_from_before_stats_means_update() {
        let prev = State {
            ver: VER_BEFORE_STATS.into(),
            ..State::default()
        };
        let f = flags(&prev, "2026-11-24", "w1", "2026-11");
        assert!(f.up && !f.new);
    }
}
