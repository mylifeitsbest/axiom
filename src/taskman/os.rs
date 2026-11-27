//! Процессы Axiom глазами системы: сколько памяти и процессора у каждого.
//!
//! Память — частный рабочий набор: страницы в оперативной памяти, которые занимает
//! только этот процесс. Ту же цифру показывает диспетчер задач Windows в столбце
//! «Память». Общие библиотеки и выделенное, но выгруженное на диск сюда не входят.

/// Процесс Axiom или один из его потомков (движок, страницы, видеокарта…).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    /// Когда запущен (в единицах системы): отличает процесс от нового с тем же номером.
    pub started: u64,
    /// Частная память в оперативной памяти, байты.
    pub memory: u64,
    /// Время процессора с запуска, нс (все ядра вместе).
    pub cpu_ns: u64,
    /// Имя исполняемого файла (Linux: первые 15 букв).
    pub name: String,
}

/// Процесс `root` и все его потомки: сначала он сам. Пусто — систему спросить не вышло.
pub fn tree(root: u32) -> Vec<Proc> {
    let mut all = list();
    let keep = descendants(&all, root);
    let mut tree: Vec<Proc> = keep.into_iter().map(|i| all[i].clone()).collect();
    all.clear();
    fill_memory(&mut tree);
    tree
}

/// Номера в `all` процесса `root` и всех его потомков (корень первым). Ребёнок
/// моложе родителя: так процесс с тем же номером, но от умершего родителя, не попадёт.
fn descendants(all: &[Proc], root: u32) -> Vec<usize> {
    let Some(first) = all.iter().position(|p| p.pid == root) else {
        return Vec::new();
    };
    let mut out = vec![first];
    let mut at = 0;
    while at < out.len() {
        let parent = &all[out[at]];
        for (i, p) in all.iter().enumerate() {
            if p.parent == parent.pid
                && p.pid != parent.pid
                && p.started >= parent.started
                && !out.contains(&i)
            {
                out.push(i);
            }
        }
        at += 1;
    }
    out
}

/// Сколько логических ядер: процент процессора считается от всех, как в диспетчере задач.
pub fn cpus() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

// ---------- Windows ----------

/// Все процессы системы одним вызовом: память и время процессора уже в ответе.
#[cfg(windows)]
fn list() -> Vec<Proc> {
    use std::ffi::c_void;

    /// Начало `SYSTEM_PROCESS_INFORMATION` (x64 и x86) — до нужных полей.
    #[repr(C)]
    struct Info {
        next_entry_offset: u32,
        number_of_threads: u32,
        working_set_private_size: i64,
        hard_fault_count: u32,
        number_of_threads_high_watermark: u32,
        cycle_time: u64,
        create_time: i64,
        user_time: i64,
        kernel_time: i64,
        image_name_length: u16,
        image_name_maximum_length: u16,
        image_name_buffer: *const u16,
        base_priority: i32,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQuerySystemInformation(
            class: u32,
            info: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
    }
    const SYSTEM_PROCESS_INFORMATION: u32 = 5;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
    const MAX_BUFFER: usize = 64 * 1024 * 1024;

    // u64: записи в ответе выровнены по 8 байт.
    let mut buf: Vec<u64> = vec![0; 128 * 1024];
    loop {
        let bytes = buf.len() * 8;
        let mut needed = 0u32;
        // SAFETY: буфер наш и длина в байтах верная; система пишет только в него.
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION,
                buf.as_mut_ptr().cast(),
                bytes as u32,
                &mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // Между вызовами могли запуститься новые процессы: берём с запасом.
            let want = (needed as usize).max(bytes) + 256 * 1024;
            if want > MAX_BUFFER {
                return Vec::new();
            }
            buf.resize(want.div_ceil(8), 0);
            continue;
        }
        if status < 0 {
            return Vec::new();
        }
        break;
    }

    let bytes = buf.len() * 8;
    let base = buf.as_ptr().cast::<u8>();
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + size_of::<Info>() <= bytes {
        // SAFETY: запись целиком в буфере (проверено выше); read_unaligned не
        // требует выравнивания.
        let info = unsafe { std::ptr::read_unaligned(base.add(offset).cast::<Info>()) };
        let name = {
            let start = info.image_name_buffer as usize;
            let len = usize::from(info.image_name_length);
            let inside = start >= base as usize && start + len <= base as usize + bytes;
            if start == 0 || len == 0 || !inside || !start.is_multiple_of(2) {
                String::new()
            } else {
                // SAFETY: имя лежит внутри нашего буфера (проверено) и выровнено.
                let units = unsafe { std::slice::from_raw_parts(info.image_name_buffer, len / 2) };
                String::from_utf16_lossy(units)
            }
        };
        let times = |t: i64| u64::try_from(t).unwrap_or(0);
        out.push(Proc {
            pid: info.unique_process_id as u32,
            parent: info.inherited_from_unique_process_id as u32,
            started: times(info.create_time),
            memory: times(info.working_set_private_size),
            // Время в ответе — в сотнях наносекунд.
            cpu_ns: (times(info.user_time) + times(info.kernel_time)).saturating_mul(100),
            name,
        });
        if info.next_entry_offset == 0 {
            break;
        }
        offset += info.next_entry_offset as usize;
    }
    out
}

/// На Windows память уже пришла вместе со списком.
#[cfg(windows)]
fn fill_memory(_tree: &mut [Proc]) {}

// ---------- Linux ----------

/// Все процессы из `/proc/<pid>/stat`: родитель, время запуска и процессора.
/// Память читаем потом и только у своих (`fill_memory`): это дороже.
#[cfg(target_os = "linux")]
fn list() -> Vec<Proc> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| {
            parse_stat(
                pid,
                &std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?,
            )
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn fill_memory(tree: &mut [Proc]) {
    for proc in tree {
        let pid = proc.pid;
        proc.memory = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            .ok()
            .and_then(|text| private_kib(&text))
            .or_else(|| {
                let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
                kib_field(&text, "RssAnon:")
            })
            .unwrap_or(0)
            * 1024;
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn list() -> Vec<Proc> {
    Vec::new()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn fill_memory(_tree: &mut [Proc]) {}

/// Строка `/proc/<pid>/stat`. Имя в скобках может содержать пробелы и скобки,
/// поэтому поля считаем от последней `)`. Время в тиках (USER_HZ = 100 на Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat(pid: u32, stat: &str) -> Option<Proc> {
    const TICK_NS: u64 = 10_000_000;
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // rest[0] — поле 3 (состояние): поле N лежит в rest[N - 3].
    let field = |n: usize| rest.get(n - 3)?.parse::<u64>().ok();
    Some(Proc {
        pid,
        parent: u32::try_from(field(4)?).ok()?,
        started: field(22)?,
        memory: 0,
        cpu_ns: (field(14)? + field(15)?).saturating_mul(TICK_NS),
        name,
    })
}

/// Частная память из `smaps_rollup`: `Private_Clean` + `Private_Dirty`, КиБ.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn private_kib(text: &str) -> Option<u64> {
    Some(kib_field(text, "Private_Clean:")? + kib_field(text, "Private_Dirty:")?)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn kib_field(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find(|l| l.starts_with(name))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, started: u64) -> Proc {
        Proc {
            pid,
            parent,
            started,
            memory: 0,
            cpu_ns: 0,
            name: String::new(),
        }
    }

    #[test]
    fn descendants_follow_parents() {
        let all = vec![
            p(1, 0, 0),
            p(10, 1, 5),
            p(11, 10, 6),
            p(12, 10, 7),
            p(13, 11, 8),
            p(20, 1, 5),
            // Номер 10 когда-то был у другого процесса: этот старше «родителя».
            p(30, 10, 1),
        ];
        let pids: Vec<u32> = descendants(&all, 10).iter().map(|&i| all[i].pid).collect();
        assert_eq!(pids, vec![10, 11, 12, 13]);
        assert!(descendants(&all, 99).is_empty());
    }

    #[test]
    fn stat_with_odd_name() {
        let line = "4242 (Web Content (x)) S 4200 4242 4242 0 -1 4194560 1 0 0 0 \
                    150 50 0 0 20 0 30 0 123456 1000 100 18446744073709551615";
        let p = parse_stat(4242, line).unwrap_or_default();
        assert_eq!(p.name, "Web Content (x)");
        assert_eq!(p.parent, 4200);
        assert_eq!(p.cpu_ns, 200 * 10_000_000);
        assert_eq!(p.started, 123_456);
        assert_eq!(parse_stat(1, "garbage"), None);
    }

    #[test]
    fn smaps_rollup_private() {
        let text = "Rss:   1000 kB\nPss:    700 kB\nPrivate_Clean:   120 kB\n\
                    Private_Dirty:   380 kB\nSwap: 0 kB\n";
        assert_eq!(private_kib(text), Some(500));
        assert_eq!(kib_field("RssAnon:\t  2048 kB\n", "RssAnon:"), Some(2048));
        assert_eq!(private_kib("Rss: 1 kB\n"), None);
    }

    #[test]
    fn this_process_is_seen() {
        if cfg!(any(windows, target_os = "linux")) {
            let tree = tree(std::process::id());
            let me = tree.first().cloned().unwrap_or_default();
            assert_eq!(me.pid, std::process::id());
            assert!(me.memory > 0, "память: {me:?}");
            assert!(cpus() >= 1);
        }
    }
}
