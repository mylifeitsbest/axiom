//! Один экземпляр браузера.
//!
//! Когда Axiom — браузер по умолчанию, каждая ссылка из другой программы
//! запускает `axiom.exe <url>`. Второй процесс передаёт адреса первому через
//! локальный сокет (127.0.0.1 + секретный токен из папки профиля) и сразу выходит.
//!
//! Подключиться к сокету может любая программа на компьютере, поэтому сервер
//! соединениям не доверяет: каждое — в своём потоке и не дольше [`DEADLINE`],
//! одновременно не больше [`MAX_CLIENTS`], токен сравнивается за постоянное время.
//! При выходе `instance.json` с портом и токеном удаляется.
//!
//! Открыт ли Axiom, решает не сокет, а замок `instance.lock`: его держит главный
//! экземпляр, пока жив, и система снимает его сама, даже после вылета. Сокет
//! начинает отвечать, только когда окно построено, а на медленном компьютере
//! сразу после обновления это бывает дольше десяти секунд. Раньше второй запуск
//! через три секунды без ответа считал Axiom закрытым и открывал второе окно
//! в том же профиле: тот записывал ложный вылет, открывал адреса дважды,
//! а WebView2 мог и вовсе отказать («Требуемый ресурс занят»).

use std::{
    collections::hash_map::RandomState,
    fs::{File, TryLockError},
    hash::{BuildHasher, Hasher},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use tao::event_loop::EventLoopProxy;

use crate::{
    events::UserEvent,
    ipc::MAX_INPUT,
    storage::{Paths, open_private, read_json, write_json},
};

const FILE: &str = "instance.json";
const LOCK: &str = "instance.lock";
const MAGIC: &str = "AXIOM1";
const MAX_URLS: usize = 50;
/// Сколько соединений обслуживаем одновременно; остальные ждут в очереди сокета.
const MAX_CLIENTS: usize = 8;
/// Срок на весь запрос. Второй процесс Axiom отправляет его сразу, одним куском,
/// а молчащее соединение дольше места не занимает.
const DEADLINE: Duration = Duration::from_secs(2);
/// Сколько новый запуск ждёт открытый Axiom, который ещё не готов принять адреса.
const WAIT: Duration = Duration::from_secs(30);
/// Как часто смотреть, появилась ли запись открытого Axiom и не закрылся ли он.
const POLL: Duration = Duration::from_millis(200);

#[derive(Serialize, Deserialize)]
struct Info {
    port: u16,
    token: String,
}

pub struct Server {
    listener: TcpListener,
    token: String,
    file: PathBuf,
}

/// Мы — главный экземпляр: замок профиля и сокет (если поднялся).
pub struct Claimed {
    server: Option<Server>,
    /// `None` — замки на этом диске не работают (бывает на сетевом), живём без него.
    lock: Option<File>,
}

/// Запись о нас в `instance.json` (её нужно убрать при выходе) и замок профиля.
pub struct Registration {
    entry: Option<(PathBuf, String)>,
    /// Сервер больше не принимает адреса: окно закрывается.
    closing: Arc<AtomicBool>,
    _lock: Option<File>,
}

/// `None` — адреса переданы уже запущенному окну, этому процессу пора выйти.
pub fn claim(paths: &Paths, urls: &[String]) -> Option<Claimed> {
    claim_within(paths, urls, WAIT)
}

fn claim_within(paths: &Paths, urls: &[String], wait: Duration) -> Option<Claimed> {
    let file = paths.root.join(FILE);
    let until = Instant::now() + wait;
    let mut first = true;
    loop {
        match lock(&paths.root) {
            // Открытого Axiom с замком нет. Запись в `instance.json` осталась от вылета
            // или от версии без замка — такой, если она ещё открыта, отдаём адреса, как раньше.
            Lock::Ours(lock) => {
                if first && forwarded(&file, urls) {
                    return None;
                }
                return Some(Claimed {
                    server: bind(paths).ok(),
                    lock: Some(lock),
                });
            }
            Lock::Unsupported => {
                if forwarded(&file, urls) {
                    return None;
                }
                return Some(Claimed {
                    server: bind(paths).ok(),
                    lock: None,
                });
            }
            // Axiom открыт. Он может ещё строить окно (сокет ответит после этого)
            // или закрываться (тогда откажет, а замок вот-вот освободится).
            Lock::Held => {
                if let Some(info) = read_json::<Info>(&file) {
                    match forward(&info, urls, until) {
                        // Без ответа запрос всё равно лежит у него в очереди сокета:
                        // второй раз не шлём, иначе адреса откроются дважды.
                        Ok(()) | Err(Failed::NoReply) => return None,
                        Err(Failed::Refused) => {}
                    }
                }
                // Не дождались: второе окно в том же профиле хуже, чем ни одного.
                if Instant::now() >= until {
                    return None;
                }
                thread::sleep(POLL);
            }
        }
        first = false;
    }
}

/// Адреса приняло окно из `instance.json`, уже готовое отвечать.
fn forwarded(file: &Path, urls: &[String]) -> bool {
    // Все места сервера могут быть заняты: с запасом на их срок.
    let reply_by = Instant::now() + DEADLINE + Duration::from_secs(1);
    read_json::<Info>(file).is_some_and(|info| forward(&info, urls, reply_by).is_ok())
}

enum Lock {
    Ours(File),
    /// Замок держит другой процесс — открытый Axiom.
    Held,
    Unsupported,
}

/// Замок на весь файл: держится, пока открыт этот `File`, и снимается системой,
/// когда процесс завершился любым путём. Дочерние процессы его не наследуют.
fn lock(root: &Path) -> Lock {
    let Ok(file) = open_private(&root.join(LOCK)) else {
        return Lock::Unsupported;
    };
    match file.try_lock() {
        Ok(()) => Lock::Ours(file),
        Err(TryLockError::WouldBlock) => Lock::Held,
        Err(TryLockError::Error(_)) => Lock::Unsupported,
    }
}

/// Почему адреса не переданы.
#[derive(Debug, PartialEq)]
enum Failed {
    /// Не дошли: порт закрыт, там не наш сервер или он отказал (окно закрывается).
    Refused,
    /// Запрос ушёл, а ответа к сроку нет: Axiom ещё строит окно и прочтёт его позже.
    NoReply,
}

fn forward(info: &Info, urls: &[String], reply_by: Instant) -> Result<(), Failed> {
    let refused = |_| Failed::Refused;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_millis(400)).map_err(refused)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(refused)?;
    let mut message = format!("{MAGIC} {}\n", info.token);
    for url in urls.iter().take(MAX_URLS) {
        message.push_str(&url.replace(['\r', '\n'], ""));
        message.push('\n');
    }
    message.push('\n');
    stream.write_all(message.as_bytes()).map_err(refused)?;
    let mut reply = [0u8; 3];
    let mut timed = Timed {
        stream: &stream,
        until: reply_by,
    };
    match timed.read_exact(&mut reply) {
        Ok(()) if &reply == b"ok\n" => Ok(()),
        Ok(()) => Err(Failed::Refused),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) =>
        {
            Err(Failed::NoReply)
        }
        Err(_) => Err(Failed::Refused),
    }
}

fn bind(paths: &Paths) -> io::Result<Server> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let token = random_token();
    let file = paths.root.join(FILE);
    write_json(
        &file,
        &Info {
            port,
            token: token.clone(),
        },
    )
    .map_err(io::Error::other)?;
    Ok(Server {
        listener,
        token,
        file,
    })
}

impl Claimed {
    /// Принимает адреса в фоне. Возвращает запись, которую нужно убрать при выходе;
    /// замок профиля живёт вместе с ней.
    pub fn spawn(self, events: EventLoopProxy<UserEvent>) -> Registration {
        let closing = Arc::new(AtomicBool::new(false));
        let entry = self.server.map(|server| {
            let entry = (server.file, server.token.clone());
            let (listener, token, closing) = (server.listener, server.token, closing.clone());
            thread::spawn(move || {
                serve(&listener, &token, &closing, move |urls| {
                    let _ = events.send_event(UserEvent::External(urls));
                });
            });
            entry
        });
        Registration {
            entry,
            closing,
            _lock: self.lock,
        }
    }
}

/// Цикл сервера: каждое соединение — в своём потоке, не больше [`MAX_CLIENTS`] сразу.
fn serve<F>(listener: &TcpListener, token: &str, closing: &Arc<AtomicBool>, deliver: F)
where
    F: Fn(Vec<String>) + Clone + Send + 'static,
{
    let token: Arc<str> = token.into();
    let busy = Arc::new(AtomicUsize::new(0));
    loop {
        // Все места заняты: каждое освободится не позже чем через DEADLINE.
        if busy.load(Ordering::Acquire) >= MAX_CLIENTS {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        let Ok((stream, _)) = listener.accept() else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        let slot = Slot::take(&busy);
        let (token, closing, deliver) = (token.clone(), closing.clone(), deliver.clone());
        // Поток не создался — замыкание вместе с местом освобождается сразу.
        let _ = thread::Builder::new().spawn(move || {
            let _slot = slot;
            if let Some(urls) = read_request(stream, &token, &closing) {
                deliver(urls);
            }
        });
    }
}

impl Registration {
    /// Окно закрывается: новые адреса не принимаем (новый запуск дождётся, пока
    /// этот процесс выйдет и отпустит замок, и откроет окно сам) и убираем
    /// `instance.json`, если там всё ещё мы, а не экземпляр, запущенный после нас.
    pub fn release(&self) {
        self.closing.store(true, Ordering::Release);
        let Some((file, token)) = &self.entry else {
            return;
        };
        if read_json::<Info>(file).is_some_and(|info| &info.token == token) {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// Занятое место в сервере; освобождается, когда соединение обслужено.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(busy: &Arc<AtomicUsize>) -> Self {
        busy.fetch_add(1, Ordering::AcqRel);
        Self(busy.clone())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Чтение с общим сроком на всё соединение. Таймаут сокета действует на каждый
/// `read` отдельно, и соединение, присылающее по байту, иначе висело бы сколько угодно.
struct Timed<'a> {
    stream: &'a TcpStream,
    until: Instant,
}

impl Read for Timed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        Read::read(&mut self.stream, buf)
    }
}

fn read_request(stream: TcpStream, token: &str, closing: &AtomicBool) -> Option<Vec<String>> {
    let timed = Timed {
        stream: &stream,
        until: Instant::now() + DEADLINE,
    };
    let mut reader = BufReader::new(timed.take(64 * 1024));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let expected = format!("{MAGIC} {token}");
    if !same_secret(line.trim_end().as_bytes(), expected.as_bytes()) {
        return None;
    }
    let mut urls = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let url = line.trim();
        if url.is_empty() {
            break;
        }
        if url.len() <= MAX_INPUT && urls.len() < MAX_URLS {
            urls.push(url.to_string());
        }
    }
    // Закрывающемуся окну адреса ни к чему: соединение рвётся без «ok».
    if closing.load(Ordering::Acquire) {
        return None;
    }
    stream.set_write_timeout(Some(DEADLINE)).ok()?;
    (&stream).write_all(b"ok\n").ok()?;
    Some(urls)
}

/// Сравнение без раннего выхода: время ответа не выдаёт, сколько первых символов
/// токена угадано. Длина не секрет — у токена она всегда одна.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// 128 бит из системного генератора, которым std засевает `RandomState`.
pub(crate) fn random_token() -> String {
    let part = || {
        let mut h = RandomState::new().build_hasher();
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_differ() {
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 32);
    }

    #[test]
    fn compares_secrets() {
        assert!(same_secret(b"AXIOM1 abc", b"AXIOM1 abc"));
        assert!(!same_secret(b"AXIOM1 abd", b"AXIOM1 abc"));
        assert!(!same_secret(b"AXIOM1 ab", b"AXIOM1 abc"));
        assert!(!same_secret(b"", b"AXIOM1 abc"));
        assert!(same_secret(b"", b""));
    }

    #[test]
    fn roundtrip_over_socket() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok();
        let Some(listener) = listener else { return };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().ok()?;
            read_request(stream, "secret", &AtomicBool::new(false))
        });
        let info = Info {
            port,
            token: "secret".into(),
        };
        let urls = vec!["https://example.com".to_string(), "rust".to_string()];
        assert!(forward(&info, &urls, soon()).is_ok());
        let got = server.join().ok().flatten();
        assert_eq!(got, Some(urls));
    }

    #[test]
    fn rejects_wrong_token() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok();
        let Some(listener) = listener else { return };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().ok()?;
            read_request(stream, "secret", &AtomicBool::new(false))
        });
        let info = Info {
            port,
            token: "guess".into(),
        };
        assert!(forward(&info, &[], soon()).is_err());
        assert_eq!(server.join().ok().flatten(), None);
    }

    #[test]
    fn slow_client_gets_cut_off() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok();
        let Some(listener) = listener else { return };
        let Ok(addr) = listener.local_addr() else {
            return;
        };
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().ok()?;
            let started = Instant::now();
            let got = read_request(stream, "secret", &AtomicBool::new(false));
            Some((got, started.elapsed()))
        });
        // Начало строки и молчание: сервер не ждёт дольше своего срока.
        let client = TcpStream::connect(addr).ok();
        if let Some(mut client) = client {
            let _ = client.write_all(b"AXIOM1 se");
            let (got, took) = server
                .join()
                .ok()
                .flatten()
                .unwrap_or((None, Duration::MAX));
            assert_eq!(got, None);
            assert!(took < DEADLINE + Duration::from_secs(1), "{took:?}");
        }
    }

    #[test]
    fn silent_clients_do_not_block_others() {
        let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
            return;
        };
        let Ok(addr) = listener.local_addr() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            serve(&listener, "secret", &Arc::default(), move |urls| {
                let _ = tx.send(urls);
            });
        });
        // Молчат все, кроме одного места: настоящий запрос не ждёт их срока.
        let silent: Vec<_> = (0..MAX_CLIENTS - 1)
            .filter_map(|_| TcpStream::connect(addr).ok())
            .collect();
        let started = Instant::now();
        let info = Info {
            port: addr.port(),
            token: "secret".into(),
        };
        let urls = vec!["https://example.com".to_string()];
        let sent = forward(&info, &urls, soon());
        let took = started.elapsed();
        let got = rx.recv_timeout(Duration::from_secs(1)).ok();
        drop(silent);
        assert!(sent.is_ok());
        assert!(took < DEADLINE, "{took:?}");
        assert_eq!(got, Some(urls));
    }

    #[test]
    fn releases_only_own_file() {
        let root = std::env::temp_dir().join(format!("axiom-instance-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let paths = Paths { root: root.clone() };
        let registration = |s: Server| Registration {
            entry: Some((s.file, s.token)),
            closing: Arc::default(),
            _lock: None,
        };
        let ours = bind(&paths).map(registration);
        // После нас запустился другой экземпляр: его запись не трогаем.
        let theirs = bind(&paths).map(registration);
        let file = root.join(FILE);
        if let Ok(ours) = &ours {
            ours.release();
        }
        let kept = file.exists();
        if let Ok(theirs) = &theirs {
            theirs.release();
        }
        let removed = !file.exists();
        let _ = std::fs::remove_dir_all(&root);
        assert!(ours.is_ok() && theirs.is_ok());
        assert!(kept, "чужой instance.json удалён");
        assert!(removed, "instance.json остался после выхода");
    }

    fn soon() -> Instant {
        Instant::now() + DEADLINE + Duration::from_secs(1)
    }

    /// Папка профиля для теста; удаляется вместе со значением.
    struct Profile(Paths);

    impl Profile {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "axiom-claim-{name}-{}-{}",
                std::process::id(),
                random_token()
            ));
            let _ = std::fs::create_dir_all(&root);
            Self(Paths { root })
        }
    }

    impl Drop for Profile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.root);
        }
    }

    fn held(root: &Path) -> Option<File> {
        match lock(root) {
            Lock::Ours(file) => Some(file),
            _ => None,
        }
    }

    #[test]
    fn lock_is_exclusive_until_dropped() {
        let profile = Profile::new("lock");
        let first = held(&profile.0.root);
        assert!(first.is_some(), "замок не взялся");
        assert!(matches!(lock(&profile.0.root), Lock::Held));
        drop(first);
        assert!(held(&profile.0.root).is_some(), "замок не отпустился");
    }

    /// Открытый Axiom ещё строит окно: сокет есть, но принимать начнёт через 3.5 с —
    /// дольше прежних трёх секунд, после которых второй запуск открывал своё окно.
    #[test]
    fn waits_for_instance_that_is_still_starting() {
        let profile = Profile::new("slow");
        let lock = held(&profile.0.root);
        let server = bind(&profile.0).ok();
        let (Some(_lock), Some(server)) = (lock, server) else {
            panic!("профиль не занят");
        };
        let paths = Paths {
            root: profile.0.root.clone(),
        };
        let urls = vec!["https://example.com/".to_string()];
        let sent = urls.clone();
        let second = thread::spawn(move || {
            let started = Instant::now();
            let claimed = claim_within(&paths, &sent, Duration::from_secs(20)).is_some();
            (claimed, started.elapsed())
        });
        thread::sleep(Duration::from_millis(3500));
        let (tx, rx) = std::sync::mpsc::channel();
        let (listener, token) = (server.listener, server.token);
        thread::spawn(move || {
            serve(&listener, &token, &Arc::default(), move |urls| {
                let _ = tx.send(urls);
            });
        });
        let (claimed, took) = second.join().unwrap_or((true, Duration::ZERO));
        assert!(!claimed, "второй запуск открыл своё окно");
        assert!(took >= Duration::from_secs(3), "{took:?}");
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).ok(), Some(urls));
        // Ровно один раз: повторный запрос открыл бы адреса дважды.
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    }

    /// Открытый Axiom закрылся, пока второй запуск ждал: окно открывает второй.
    #[test]
    fn takes_over_when_open_instance_exits() {
        let profile = Profile::new("exit");
        let Some(lock) = held(&profile.0.root) else {
            panic!("замок не взялся");
        };
        let paths = Paths {
            root: profile.0.root.clone(),
        };
        let second = thread::spawn(move || {
            claim_within(&paths, &[], Duration::from_secs(20)).map(|c| c.lock.is_some())
        });
        thread::sleep(Duration::from_millis(600));
        drop(lock);
        assert_eq!(second.join().ok().flatten(), Some(true));
        assert!(profile.0.root.join(FILE).exists());
    }

    /// Открытый Axiom так и не ответил: второе окно в том же профиле не открываем.
    #[test]
    fn gives_up_without_second_window() {
        let profile = Profile::new("hung");
        let lock = held(&profile.0.root);
        let server = bind(&profile.0).ok();
        let (Some(_lock), Some(_server)) = (lock, server) else {
            panic!("профиль не занят");
        };
        let before = read_json::<Info>(&profile.0.root.join(FILE)).map(|i| i.port);
        let started = Instant::now();
        assert!(claim_within(&profile.0, &[], Duration::from_secs(1)).is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
        let after = read_json::<Info>(&profile.0.root.join(FILE)).map(|i| i.port);
        assert_eq!(before, after, "instance.json перезаписан");
    }

    /// Закрывающееся окно адреса не берёт: иначе они пропали бы вместе с ним.
    #[test]
    fn closing_instance_refuses() {
        let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
            return;
        };
        let Ok(addr) = listener.local_addr() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let closing = Arc::new(AtomicBool::new(true));
        thread::spawn(move || {
            serve(&listener, "secret", &closing, move |urls| {
                let _ = tx.send(urls);
            });
        });
        let info = Info {
            port: addr.port(),
            token: "secret".into(),
        };
        let urls = vec!["https://example.com".to_string()];
        assert_eq!(forward(&info, &urls, soon()), Err(Failed::Refused));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }
}
