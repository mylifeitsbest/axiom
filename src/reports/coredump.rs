//! Linux: кто именно упал — по записям systemd-coredump.
//!
//! «free(): corrupted unsorted chunks» в журнале пишет glibc того процесса, чья куча
//! испорчена, но stderr у Axiom общий с процессами WebKitGTK, и по самой строке не
//! понять, чей это вылет. Обработчик сигнала (`terminal`) пишет стек только процесса
//! Axiom; раз его строки в журнале нет, упал, скорее всего, процесс WebKit.
//!
//! Во многих дистрибутивах (Arch, Fedora, openSUSE…) такие вылеты принимает
//! systemd-coredump и сохраняет стек упавшего потока. В отчёт о вылете идут записи
//! с начала того запуска — только процессов Axiom и его WebKitGTK (`webkit2gtk-4.1`)
//! этого пользователя: имя процесса, сигнал и стек (функции и библиотеки). Командная
//! строка, окружение и сам дамп памяти не берутся. Без `coredumpctl` — ничего.

use std::{
    io::Read,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use serde::Deserialize;

/// Больше упавших процессов в одном отчёте не нужно: первый по времени обычно и есть виновник.
const MAX_DUMPS: usize = 3;
/// Кадров стека на процесс: хватает дойти от `abort` до виновника.
const MAX_FRAMES: usize = 32;
/// `coredumpctl` читает журнал systemd: на большом журнале это бывает долго.
const LIMIT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct Dump {
    pid: u64,
    #[serde(default)]
    uid: Option<u32>,
    #[serde(default)]
    exe: Option<String>,
}

/// Упавшие процессы Axiom и WebKitGTK с момента `since` (секунды Unix), со стеками;
/// `None` — таких записей нет или systemd-coredump в системе нет.
pub fn since(since: u64) -> Option<String> {
    if since == 0 {
        return None;
    }
    let list = run(&["list", "--json=short", &format!("--since=@{since}")])?;
    // SAFETY: getuid ничего не принимает и всегда успешна.
    let uid = unsafe { libc::getuid() };
    let dumps = ours(&list, uid);
    let parts: Vec<String> = dumps
        .iter()
        .take(MAX_DUMPS)
        .filter_map(|pid| run(&["info", &pid.to_string()]).and_then(|info| summary(&info)))
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "Упавшие процессы (systemd-coredump):\n{}",
        parts.join("\n\n")
    ))
}

/// PID упавших процессов Axiom и его WebKitGTK у пользователя `uid`, от старых к новым.
fn ours(list: &str, uid: u32) -> Vec<u64> {
    let dumps: Vec<Dump> = serde_json::from_str(list.trim()).unwrap_or_default();
    dumps
        .into_iter()
        .filter(|d| d.uid.is_none_or(|u| u == uid))
        .filter(|d| d.exe.as_deref().is_some_and(is_ours))
        .map(|d| d.pid)
        .collect()
}

/// Сам Axiom или процесс его движка (`/usr/lib/webkit2gtk-4.1/WebKitWebProcess` и др.).
/// WebKitGTK других программ обычно другой (`webkitgtk-6.0`) — их вылеты не берём.
fn is_ours(exe: &str) -> bool {
    let name = exe.rsplit('/').next().unwrap_or(exe);
    name.starts_with("axiom") || (exe.contains("webkit2gtk-4.1") && name.starts_with("WebKit"))
}

/// Из `coredumpctl info`: имя процесса, сигнал и стек первого потока (упавшего).
fn summary(info: &str) -> Option<String> {
    let mut name = None;
    let mut signal = None;
    let mut frames = Vec::new();
    let mut in_stack = false;
    for line in info.lines() {
        let line = line.trim();
        if in_stack {
            if line.starts_with('#') {
                if frames.len() < MAX_FRAMES {
                    frames.push(line.to_string());
                }
                continue;
            }
            // Стек первого потока кончился: остальные потоки не нужны.
            if !frames.is_empty() {
                break;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("PID:") {
            // `274366 (WebKitWebProcess)` — только имя.
            name = rest
                .split_once('(')
                .and_then(|(_, n)| n.strip_suffix(')'))
                .map(str::to_string);
        } else if let Some(rest) = line.strip_prefix("Signal:") {
            signal = Some(rest.trim().to_string());
        } else if line.starts_with("Stack trace of thread") {
            in_stack = true;
        }
    }
    let name = name?;
    let mut text = format!(
        "{name} · сигнал {}",
        signal.as_deref().unwrap_or("неизвестен")
    );
    if frames.is_empty() {
        text.push_str("\n(стека в записи нет)");
    }
    for frame in frames {
        text.push('\n');
        text.push_str(&frame);
    }
    Some(text)
}

/// `coredumpctl <args>` без пейджера и цвета; дольше [`LIMIT`] — снимаем.
fn run(args: &[&str]) -> Option<String> {
    let mut child = Command::new("coredumpctl")
        .arg("--no-pager")
        .args(args)
        .env("SYSTEMD_COLORS", "0")
        // Символы отладки по сети не нужны: хватит того, что уже в записи.
        .env_remove("DEBUGINFOD_URLS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let bytes = rx.recv_timeout(LIMIT);
    if bytes.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().ok()?;
    let bytes = bytes.ok()?;
    if !status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_only_our_processes() {
        let list = r#"[
            {"time":1764252987000000,"pid":101,"uid":1000,"gid":1000,"sig":6,"corefile":"present","exe":"/usr/lib/webkit2gtk-4.1/WebKitNetworkProcess","size":1},
            {"time":1764252988000000,"pid":102,"uid":1000,"gid":1000,"sig":11,"corefile":"present","exe":"/usr/lib/webkitgtk-6.0/WebKitWebProcess","size":1},
            {"time":1764252989000000,"pid":103,"uid":1000,"gid":1000,"sig":6,"corefile":"none","exe":"/usr/bin/axiom","size":null},
            {"time":1764252990000000,"pid":104,"uid":0,"gid":0,"sig":6,"corefile":"present","exe":"/usr/bin/axiom","size":1},
            {"time":1764252991000000,"pid":105,"uid":1000,"gid":1000,"sig":6,"corefile":"present","exe":"/usr/bin/firefox","size":1}
        ]"#;
        assert_eq!(ours(list, 1000), vec![101, 103]);
        assert!(ours("не json", 1000).is_empty());
    }

    #[test]
    fn summary_keeps_name_signal_and_first_stack() {
        let info = "           PID: 274366 (WebKitNetworkProcess)
           UID: 1000 (user)
        Signal: 6 (ABRT)
     Timestamp: Thu 2026-11-27 14:20:11 +05 (1h ago)
  Command Line: /usr/lib/webkit2gtk-4.1/WebKitNetworkProcess 7 39
    Executable: /usr/lib/webkit2gtk-4.1/WebKitNetworkProcess
       Message: Process 274366 (WebKitNetworkProcess) of user 1000 dumped core.

                Stack trace of thread 274380:
                #0  0x00007f3c4e2a5e5c n/a (libc.so.6 + 0x8de5c)
                #1  0x00007f3c4e24dd40 raise (libc.so.6 + 0x35d40)
                #2  0x00007f3c4e2354c3 abort (libc.so.6 + 0x1d4c3)
                #3  0x00007f3c4d0c1a2b n/a (libnvidia-eglcore.so.580.95.05 + 0x8c1a2b)

                Stack trace of thread 274366:
                #0  0x00007f3c4e31e88d poll (libc.so.6 + 0x10688d)
";
        let text = summary(info).unwrap_or_default();
        assert!(
            text.starts_with("WebKitNetworkProcess · сигнал 6 (ABRT)"),
            "{text}"
        );
        assert!(text.contains("libnvidia-eglcore"), "{text}");
        assert!(!text.contains("poll"), "{text}");
        // Командная строка в отчёт не идёт.
        assert!(!text.contains(" 7 39"), "{text}");
        assert_eq!(text.lines().count(), 5, "{text}");
    }

    #[test]
    fn summary_without_stack() {
        let info = "PID: 5 (axiom)\nSignal: 11 (SEGV)\n";
        assert_eq!(
            summary(info).as_deref(),
            Some("axiom · сигнал 11 (SEGV)\n(стека в записи нет)")
        );
        assert_eq!(summary("ничего"), None);
    }
}
