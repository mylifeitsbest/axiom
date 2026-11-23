//! Один экземпляр браузера.
//!
//! Когда Axiom — браузер по умолчанию, каждая ссылка из другой программы
//! запускает `axiom.exe <url>`. Второй процесс передаёт адреса первому через
//! локальный сокет (127.0.0.1 + секретный токен из папки профиля) и сразу выходит.

use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    thread,
    time::{Duration, SystemTime},
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

#[derive(Serialize, Deserialize)]
struct Info {
    port: u16,
    token: String,
}

pub struct Server {
    listener: TcpListener,
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

fn forward(info: &Info, urls: &[String]) -> std::io::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(400))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
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
        Err(std::io::Error::other("чужой сервер"))
    }
}

fn bind(paths: &Paths) -> std::io::Result<Server> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let token = random_token();
    write_json(
        &paths.root.join(FILE),
        &Info {
            port,
            token: token.clone(),
        },
    )
    .map_err(std::io::Error::other)?;
    Ok(Server { listener, token })
}

impl Server {
    pub fn spawn(self, events: EventLoopProxy<UserEvent>) {
        thread::spawn(move || {
            for stream in self.listener.incoming().flatten() {
                if let Some(urls) = read_request(stream, &self.token)
                    && events.send_event(UserEvent::External(urls)).is_err()
                {
                    return;
                }
            }
        });
    }
}

fn read_request(mut stream: TcpStream, token: &str) -> Option<Vec<String>> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?.take(64 * 1024));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    if line.trim_end() != format!("{MAGIC} {token}") {
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
    stream.write_all(b"ok\n").ok()?;
    Some(urls)
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
}
