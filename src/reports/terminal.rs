//! Linux: журнал ещё и на экран терминала, и строка о вылете в нативном коде.
//!
//! Axiom запустили из терминала (stderr — терминал): всё, что пишется в stderr, идёт
//! и в `axiom.log`, и на экран. На дескриптор 2 встаёт труба, а поток `journal`
//! переписывает её в файл и в терминал. Дескриптор 2 наследуют процессы WebKitGTK —
//! их сообщения видны тоже. Без терминала stderr — сам файл, как раньше.
//!
//! Вылет в нативном коде (SIGSEGV и другие) паники не оставляет, и в журнале после
//! строки о запуске было пусто. Теперь обработчик сигнала дочитывает остаток трубы
//! (поток умрёт вместе с процессом, а там последние слова WebKit и GLib), дописывает
//! строку с именем сигнала прямо в файл журнала и в терминал, возвращает обработчик
//! по умолчанию и повторяет сигнал: процесс завершается так же, как без него.
//! В обработчике — только то, что можно звать из обработчика сигнала: атомики,
//! `read`, `write`, `signal`, `raise`.

use std::{
    fs::File,
    io::ErrorKind,
    os::fd::{AsRawFd, IntoRawFd, RawFd},
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    thread,
};

/// Файл журнала для обработчика сигнала; без трубы — сам stderr (он и есть файл).
static LOG: AtomicI32 = AtomicI32::new(2);
/// Терминал, на который идёт копия; -1 — копии нет.
static TTY: AtomicI32 = AtomicI32::new(-1);
/// Читающий конец трубы (неблокирующий); -1 — трубы нет.
static PIPE: AtomicI32 = AtomicI32::new(-1);
/// Терминал уже проверяли: это делает только первый `redirect`.
static CHECKED: AtomicBool = AtomicBool::new(false);

/// Сигналы вылета в нативном коде и строки о них. SIGTRAP — `CRASH()` и
/// `RELEASE_ASSERT` в WebKit и фатальные ошибки GLib.
const SIGNALS: [(libc::c_int, &str); 6] = [
    (
        libc::SIGSEGV,
        "\nAxiom упал: сигнал SIGSEGV (обращение к недоступной памяти)\n",
    ),
    (
        libc::SIGBUS,
        "\nAxiom упал: сигнал SIGBUS (ошибка доступа к памяти)\n",
    ),
    (
        libc::SIGILL,
        "\nAxiom упал: сигнал SIGILL (недопустимая инструкция)\n",
    ),
    (
        libc::SIGFPE,
        "\nAxiom упал: сигнал SIGFPE (арифметическая ошибка)\n",
    ),
    (
        libc::SIGABRT,
        "\nAxiom упал: сигнал SIGABRT (аварийное завершение, abort)\n",
    ),
    (
        libc::SIGTRAP,
        "\nAxiom упал: сигнал SIGTRAP (сработала внутренняя проверка WebKit или GLib)\n",
    ),
];

/// Направить stderr в журнал `file`. Первый раз, если stderr — терминал, — через
/// трубу с копией на экран. Новый журнал взамен большого встаёт туда же.
pub fn redirect(file: File) -> bool {
    if PIPE.load(Ordering::Acquire) >= 0 {
        // Труба уже есть: новый файл встаёт под номер прежнего, поток пишет туда же.
        let log = LOG.load(Ordering::Acquire);
        // SAFETY: оба дескриптора открыты; `log` принадлежит журналу до конца процесса.
        return unsafe { libc::dup2(file.as_raw_fd(), log) } != -1;
    }
    // SAFETY: isatty только смотрит, что за дескриптор 2.
    let file = if !CHECKED.swap(true, Ordering::AcqRel) && unsafe { libc::isatty(2) } == 1 {
        match mirror(file) {
            Ok(()) => return true,
            Err(file) => file,
        }
    } else {
        file
    };
    // SAFETY: оба дескриптора открыты; после dup2 файл закрывается, а копия на 2 остаётся.
    unsafe { libc::dup2(file.as_raw_fd(), 2) != -1 }
}

/// Копия журнала идёт на экран.
pub fn mirrored() -> bool {
    PIPE.load(Ordering::Acquire) >= 0
}

/// Труба вместо stderr и поток, который переписывает её в `file` и в терминал.
/// Не вышло — файл возвращается, и stderr станет им самим.
fn mirror(file: File) -> Result<(), File> {
    // SAFETY: копия открытого дескриптора 2 (терминала); дочерним процессам не достаётся.
    let tty = unsafe { libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 3) };
    if tty < 0 {
        return Err(file);
    }
    let mut ends: [libc::c_int; 2] = [-1, -1];
    // SAFETY: `ends` — два дескриптора, как ждёт pipe2; оба конца не наследуются.
    if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        close(tty);
        return Err(file);
    }
    let [read, write] = ends;
    // Конец для чтения — неблокирующий: остаток трубы дочитывают и выход, и обработчик
    // сигнала, не рискуя зависнуть.
    // SAFETY: `read` открыт; меняются только флаги его описания.
    unsafe {
        let flags = libc::fcntl(read, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(read, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    let log = file.as_raw_fd();
    let spawned = thread::Builder::new()
        .name("journal".into())
        .spawn(move || copy(read, log, tty));
    if spawned.is_err() {
        close(read);
        close(write);
        close(tty);
        return Err(file);
    }
    // На место stderr — пишущий конец, без O_CLOEXEC: его наследуют процессы WebKit.
    // SAFETY: `write` открыт; после dup2 он сам больше не нужен.
    let ok = unsafe { libc::dup2(write, 2) } != -1;
    // Не вышло — у трубы не остаётся пишущих, поток дочитывает её и закрывает свои концы.
    close(write);
    if !ok {
        return Err(file);
    }
    // Файл журнала живёт до конца процесса под этим номером (см. `redirect`).
    LOG.store(file.into_raw_fd(), Ordering::Release);
    TTY.store(tty, Ordering::Release);
    PIPE.store(read, Ordering::Release);
    Ok(())
}

/// Поток `journal`: всё из трубы — в файл журнала и в терминал.
fn copy(read: RawFd, log: RawFd, mut tty: RawFd) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        // SAFETY: `buf` — свой буфер указанной длины.
        let n = unsafe { libc::read(read, buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            let chunk = buf.get(..n as usize).unwrap_or_default();
            write_all(log, chunk);
            if tty >= 0 && !write_all(tty, chunk) {
                // Терминал закрыли: дальше только файл. Дескриптор не закрываем — его
                // номер мог уже взять обработчик сигнала.
                TTY.store(-1, Ordering::Release);
                tty = -1;
            }
            continue;
        }
        if n == 0 {
            // Пишущих не осталось.
            break;
        }
        match std::io::Error::last_os_error().kind() {
            ErrorKind::Interrupted => {}
            ErrorKind::WouldBlock => {
                wait(read, libc::POLLIN, -1);
            }
            _ => break,
        }
    }
    if PIPE
        .compare_exchange(read, -1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        // Поток встал, а труба на месте stderr: запись в полную трубу остановила бы
        // Axiom, поэтому stderr — снова прямо файл. Номера трубы и терминала не
        // закрываем: их мог уже взять обработчик сигнала.
        TTY.store(-1, Ordering::Release);
        // SAFETY: оба дескриптора открыты.
        unsafe { libc::dup2(log, 2) };
    } else {
        // Труба так и не встала на место stderr (см. `mirror`).
        close(read);
        if tty >= 0 {
            close(tty);
        }
    }
}

/// Записать всё; на неблокирующем терминале подождать до секунды. `false` — не вышло.
fn write_all(fd: RawFd, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        // SAFETY: `bytes` — живой срез указанной длины.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = bytes.get(n as usize..).unwrap_or_default();
            continue;
        }
        let retry = n < 0
            && match std::io::Error::last_os_error().kind() {
                ErrorKind::Interrupted => true,
                ErrorKind::WouldBlock => wait(fd, libc::POLLOUT, 1000),
                _ => false,
            };
        if !retry {
            return false;
        }
    }
    true
}

/// Дождаться готовности дескриптора; `timeout` в миллисекундах, -1 — без срока.
fn wait(fd: RawFd, events: libc::c_short, timeout: libc::c_int) -> bool {
    let mut ready = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: один `pollfd`, живой на время вызова.
    unsafe { libc::poll(&mut ready, 1, timeout) > 0 }
}

fn close(fd: RawFd) {
    // SAFETY: дескриптор открыт и больше нигде не используется.
    unsafe { libc::close(fd) };
}

/// Выход из Axiom: дописать то, что ещё в трубе, — поток не дождутся.
pub fn flush() {
    drain();
}

/// Остаток трубы — в файл и в терминал. Годится для обработчика сигнала: только
/// атомики, `read` и `write`, буфер на стеке.
fn drain() {
    let pipe = PIPE.load(Ordering::Acquire);
    if pipe < 0 {
        return;
    }
    let log = LOG.load(Ordering::Acquire);
    let tty = TTY.load(Ordering::Acquire);
    let mut buf = [0u8; 512];
    // Не больше 256 КБ: в трубе 64 КБ, но пишущие могут не останавливаться.
    for _ in 0..512 {
        // SAFETY: `buf` — свой буфер указанной длины; труба неблокирующая.
        let n = unsafe { libc::read(pipe, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            return;
        }
        let chunk = buf.get(..n as usize).unwrap_or_default();
        raw_write(log, chunk);
        if tty >= 0 {
            raw_write(tty, chunk);
        }
    }
}

/// `write` до конца или до первой ошибки — без ожиданий, для обработчика сигнала.
fn raw_write(fd: RawFd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: `bytes` — живой срез указанной длины.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n <= 0 {
            return;
        }
        bytes = bytes.get(n as usize..).unwrap_or_default();
    }
}

/// Строка о вылете в журнале и на экране для сигналов из [`SIGNALS`].
pub fn catch_crashes() {
    let handler = on_crash as extern "C" fn(libc::c_int);
    for (signal, _) in SIGNALS {
        // SAFETY: `sigaction` из нулей — допустимое значение (без флагов, пустая маска),
        // дальше заполняются нужные поля. Обработчик — `extern "C" fn(c_int)`, как
        // ждёт `sa_handler` без SA_SIGINFO. SA_RESETHAND возвращает обработчик по
        // умолчанию при входе, SA_ONSTACK — запасной стек, если поток его завёл
        // (Rust заводит для своих потоков: переполнение стека — тоже SIGSEGV).
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = libc::SA_ONSTACK | libc::SA_RESETHAND;
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

extern "C" fn on_crash(signal: libc::c_int) {
    let line = SIGNALS
        .iter()
        .find(|(s, _)| *s == signal)
        .map_or("\nAxiom упал: сигнал\n", |(_, line)| *line);
    // Сначала то, что ещё в трубе: последние сообщения перед вылетом.
    drain();
    raw_write(LOG.load(Ordering::Acquire), line.as_bytes());
    let tty = TTY.load(Ordering::Acquire);
    if tty >= 0 {
        raw_write(tty, line.as_bytes());
    }
    // SAFETY: `signal` и `raise` можно звать из обработчика сигнала. Сигнал заблокирован,
    // пока обработчик не вернётся, — тогда он и сработает по умолчанию (завершение,
    // дамп памяти), с тем же кодом, что и без Axiom.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}
