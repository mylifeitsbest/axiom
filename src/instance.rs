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

use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use tao::event_loop::EventLoopProxy;

use crate::{
    events::UserEvent,
    ipc::MAX_INPUT,
    storage::{Paths, read_json, write_json},
};

const FILE: &str = "instance.json";
const MAGIC: &str = "AXIOM1";
const MAX_URLS: usize = 50;
/// Сколько соединений обслуживаем одновременно; остальные ждут в очереди сокета.
const MAX_CLIENTS: usize = 8;
/// Срок на весь запрос. Второй процесс Axiom отправляет его сразу, одним куском,
/// а молчащее соединение дольше места не занимает.
const DEADLINE: Duration = Duration::from_secs(2);

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

/// Запись о нас в `instance.json`: при выходе её нужно убрать.
pub struct Registration {
    file: PathBuf,
    token: String,
}

/// `None` — адреса переданы уже запущенному окну, этому процессу пора выйти.
/// `Some(None)` — мы первый экземпляр, но сокет поднять не удалось (работаем без него).
pub fn claim(paths: &Paths, urls: &[String]) -> Option<Option<Server>> {
    if let Some(info) = read_json::<Info>(&paths.root.join(FILE))
        && forward(&info, urls).is_ok()
    {
        return None;
    }
    Some(bind(paths).ok())
}

fn forward(info: &Info, urls: &[String]) -> io::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(400))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    // Все места сервера могут быть заняты: с запасом на их срок.
    stream.set_read_timeout(Some(DEADLINE + Duration::from_secs(1)))?;
    let mut message = format!("{MAGIC} {}\n", info.token);
    for url in urls.iter().take(MAX_URLS) {
        message.push_str(&url.replace(['\r', '\n'], ""));
        message.push('\n');
    }
    message.push('\n');
    stream.write_all(message.as_bytes())?;
    let mut reply = [0u8; 3];
    stream.read_exact(&mut reply)?;
    if &reply == b"ok\n" {
        Ok(())
    } else {
        Err(io::Error::other("чужой сервер"))
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

impl Server {
    /// Принимает адреса в фоне. Возвращает запись, которую нужно убрать при выходе.
    pub fn spawn(self, events: EventLoopProxy<UserEvent>) -> Registration {
        let registration = Registration {
            file: self.file,
            token: self.token.clone(),
        };
        let (listener, token) = (self.listener, self.token);
        thread::spawn(move || {
            serve(&listener, &token, move |urls| {
                let _ = events.send_event(UserEvent::External(urls));
            });
        });
        registration
    }
}

/// Цикл сервера: каждое соединение — в своём потоке, не больше [`MAX_CLIENTS`] сразу.
fn serve<F>(listener: &TcpListener, token: &str, deliver: F)
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
        let (token, deliver) = (token.clone(), deliver.clone());
        // Поток не создался — замыкание вместе с местом освобождается сразу.
        let _ = thread::Builder::new().spawn(move || {
            let _slot = slot;
            if let Some(urls) = read_request(stream, &token) {
                deliver(urls);
            }
        });
    }
}

impl Registration {
    /// Убирает `instance.json`, если там всё ещё мы, а не экземпляр,
    /// запущенный после нас (например, новая версия после обновления).
    pub fn release(&self) {
        if read_json::<Info>(&self.file).is_some_and(|info| info.token == self.token) {
            let _ = std::fs::remove_file(&self.file);
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

fn read_request(stream: TcpStream, token: &str) -> Option<Vec<String>> {
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
fn random_token() -> String {
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
            read_request(stream, "secret")
        });
        let info = Info {
            port,
            token: "secret".into(),
        };
        let urls = vec!["https://example.com".to_string(), "rust".to_string()];
        assert!(forward(&info, &urls).is_ok());
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
            read_request(stream, "secret")
        });
        let info = Info {
            port,
            token: "guess".into(),
        };
        assert!(forward(&info, &[]).is_err());
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
            let got = read_request(stream, "secret");
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
            serve(&listener, "secret", move |urls| {
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
        let sent = forward(&info, &urls);
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
        let ours = bind(&paths).map(|s| Registration {
            file: s.file,
            token: s.token,
        });
        // После нас запустился другой экземпляр: его запись не трогаем.
        let theirs = bind(&paths).map(|s| Registration {
            file: s.file,
            token: s.token,
        });
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
}
