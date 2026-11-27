//! Свой SOCKS5 на 127.0.0.1 между движком и сетью — на нём держится кнопка прокси.
//!
//! Прокси у WebView2 — аргумент запуска процесса браузера (`--proxy-server`), общий
//! для всех webview профиля, и на ходу его не сменить. Поэтому, когда прокси настроен,
//! движок с самого запуска ходит через этот релей, а релей решает, куда вести каждое
//! соединение: напрямую или через прокси человека. Кнопка переключает релей — сразу,
//! без перезапуска. На Linux так же: одна дорога на обеих системах.
//!
//! - Только CONNECT и без логина: движку больше не нужно.
//! - Имя сайта приходит от движка как есть (atyp=3) и так же уходит прокси: DNS — на
//!   его стороне. Напрямую имя разрешает система, как и без релея.
//! - Прокси включён, но не отвечает — соединение не открывается вовсе, а не идёт мимо
//!   него: страницы не грузятся, пока прокси не оживёт или его не выключат.
//! - Смена маршрута рвёт все открытые соединения: иначе живые keep-alive ходили бы
//!   по-старому, и после «Включить» часть сайтов ещё шла бы напрямую.

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

/// Куда вести соединения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Direct,
    Socks5(Upstream),
    Http(Upstream),
}

/// Прокси человека. Хост IPv6 — без квадратных скобок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub host: String,
    pub port: u16,
}

/// Отвечает ли прокси (для кнопки на панели).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Ещё не пробовали или маршрут только что сменился.
    Unknown = 0,
    Up = 1,
    Down = 2,
}

impl Health {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Up,
            2 => Self::Down,
            _ => Self::Unknown,
        }
    }
}

/// Коды ответа SOCKS5 (RFC 1928, раздел 6).
mod reply {
    pub const OK: u8 = 0;
    pub const FAILURE: u8 = 1;
    pub const NOT_ALLOWED: u8 = 2;
    pub const NET_UNREACHABLE: u8 = 3;
    pub const HOST_UNREACHABLE: u8 = 4;
    pub const REFUSED: u8 = 5;
    pub const TIMED_OUT: u8 = 6;
    pub const COMMAND: u8 = 7;
    pub const ADDRESS: u8 = 8;
}

/// Сколько ждать рукопожатия движка и ответа прокси.
const HANDSHAKE: Duration = Duration::from_secs(15);
/// Сколько ждать соединения с последним из адресов.
const CONNECT: Duration = Duration::from_secs(10);
/// Попытка на каждый адрес, кроме последнего: битый IPv6 не держит страницу 10 секунд.
const CONNECT_EACH: Duration = Duration::from_secs(3);
/// Самый длинный заголовок ответа HTTP-прокси на CONNECT.
const MAX_HTTP_HEAD: usize = 16 * 1024;

/// Адрес сайта из запроса движка.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    Name(String),
    Ip(IpAddr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    host: Host,
    port: u16,
}

/// Почему соединение не открылось.
#[derive(Debug, PartialEq, Eq)]
enum Fail {
    /// Не достучались до прокси или он ответил не по протоколу.
    Proxy,
    /// Прокси работает, но сайт недоступен (или недоступен напрямую): код SOCKS5.
    Code(u8),
}

impl Fail {
    fn code(&self) -> u8 {
        match self {
            Self::Proxy => reply::FAILURE,
            Self::Code(code) => *code,
        }
    }
}

pub struct Relay {
    addr: SocketAddr,
    shared: Arc<Shared>,
}

struct Shared {
    route: Mutex<Route>,
    /// Растёт при каждой смене маршрута: соединение, начатое по старому, не доживёт.
    epoch: AtomicU64,
    /// Сокеты открытых соединений: при смене маршрута их закрывают.
    live: Mutex<HashMap<u64, [TcpStream; 2]>>,
    next: AtomicU64,
    health: AtomicU8,
    notify: Box<dyn Fn(Health) + Send + Sync>,
    closed: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Relay {
    /// Слушает случайный порт на 127.0.0.1. `notify` зовётся из потоков релея,
    /// когда прокси перестал или снова начал отвечать.
    pub fn start(
        route: Route,
        notify: impl Fn(Health) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            route: Mutex::new(route),
            epoch: AtomicU64::new(0),
            live: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
            health: AtomicU8::new(Health::Unknown as u8),
            notify: Box::new(notify),
            closed: AtomicBool::new(false),
        });
        let accept = Arc::clone(&shared);
        thread::Builder::new()
            .name("axiom-relay".into())
            .spawn(move || accept_loop(&listener, &accept))?;
        Ok(Self { addr, shared })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn health(&self) -> Health {
        Health::from_u8(self.shared.health.load(Ordering::SeqCst))
    }

    /// Новый маршрут. Соединения, открытые по прежнему, рвутся: движок откроет их
    /// заново, уже по новому.
    pub fn set_route(&self, route: Route) {
        {
            let mut current = lock(&self.shared.route);
            if *current == route {
                return;
            }
            *current = route;
            self.shared.epoch.fetch_add(1, Ordering::SeqCst);
            self.shared
                .health
                .store(Health::Unknown as u8, Ordering::SeqCst);
        }
        self.shared.cut_all();
    }

    /// Проверяет в фоне, отвечает ли прокси текущего маршрута; итог — через `notify`.
    pub fn check(&self) {
        let shared = Arc::clone(&self.shared);
        let _ = thread::Builder::new()
            .name("axiom-relay-check".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                let (route, epoch) = shared.snapshot();
                let upstream = match &route {
                    Route::Direct => return,
                    Route::Socks5(up) | Route::Http(up) => up,
                };
                let health = if dial(upstream).is_ok() {
                    Health::Up
                } else {
                    Health::Down
                };
                shared.observe(epoch, health);
            });
    }

    /// Сколько соединений сейчас открыто.
    #[cfg(test)]
    fn open_connections(&self) -> usize {
        lock(&self.shared.live).len()
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        // Разбудить accept: он увидит флаг и выйдет.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
        self.shared.cut_all();
    }
}

impl Shared {
    /// Маршрут и его номер — согласованно.
    fn snapshot(&self) -> (Route, u64) {
        let route = lock(&self.route);
        (route.clone(), self.epoch.load(Ordering::SeqCst))
    }

    /// Берёт соединение на учёт, если маршрут с `epoch` не сменился.
    /// Проверка — под тем же замком, что и `cut_all`: соединение либо попадёт
    /// под обрыв, либо не будет принято вовсе.
    fn adopt(&self, id: u64, client: &TcpStream, upstream: &TcpStream, epoch: u64) -> bool {
        let (Ok(client), Ok(upstream)) = (client.try_clone(), upstream.try_clone()) else {
            return false;
        };
        let mut live = lock(&self.live);
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return false;
        }
        live.insert(id, [client, upstream]);
        true
    }

    fn release(&self, id: u64) {
        lock(&self.live).remove(&id);
    }

    fn cut_all(&self) {
        let cut: Vec<_> = lock(&self.live).drain().collect();
        for (_, sockets) in cut {
            for socket in sockets {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
    }

    /// Запоминает, отвечает ли прокси, и говорит, если это поменялось. Итог по
    /// уже сменённому маршруту не считается.
    fn observe(&self, epoch: u64, health: Health) {
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        let before = self.health.swap(health as u8, Ordering::SeqCst);
        if before != health as u8 {
            (self.notify)(health);
        }
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.closed.load(Ordering::SeqCst) {
            return;
        }
        match stream {
            Ok(stream) => {
                let shared = Arc::clone(shared);
                // Стек с запасом: в этом потоке и разрешение имён (NSS на Linux).
                let spawned = thread::Builder::new()
                    .name("axiom-relay-conn".into())
                    .stack_size(512 * 1024)
                    .spawn(move || serve(&shared, stream));
                if let Err(err) = spawned {
                    eprintln!("прокси: не запустился поток соединения: {err}");
                }
            }
            // Кончились дескрипторы и т. п.: не крутиться вхолостую.
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn serve(shared: &Shared, mut client: TcpStream) {
    let _ = client.set_nodelay(true);
    let _ = client.set_read_timeout(Some(HANDSHAKE));
    let _ = client.set_write_timeout(Some(HANDSHAKE));
    let target = match handshake(&mut client) {
        Ok(target) => target,
        Err(code) => {
            if let Some(code) = code {
                let _ = answer(&mut client, code);
            }
            return;
        }
    };
    let id = shared.next.fetch_add(1, Ordering::SeqCst);
    // Маршрут сменился, пока соединялись, — пробуем уже по новому.
    for _ in 0..3 {
        let (route, epoch) = shared.snapshot();
        let opened = open(&route, &target);
        if route != Route::Direct {
            let health = if matches!(opened, Err(Fail::Proxy)) {
                Health::Down
            } else {
                Health::Up
            };
            shared.observe(epoch, health);
        }
        match opened {
            Err(fail) => {
                if shared.epoch.load(Ordering::SeqCst) != epoch {
                    continue;
                }
                let _ = answer(&mut client, fail.code());
                return;
            }
            Ok((upstream, early)) => {
                if !shared.adopt(id, &client, &upstream, epoch) {
                    continue;
                }
                if answer(&mut client, reply::OK).is_ok()
                    && (early.is_empty() || client.write_all(&early).is_ok())
                {
                    pipe(&client, &upstream);
                }
                shared.release(id);
                return;
            }
        }
    }
}

/// Читает приветствие и запрос движка. `Err(Some(code))` — ответить кодом и закрыть,
/// `Err(None)` — просто закрыть.
fn handshake(c: &mut TcpStream) -> Result<Target, Option<u8>> {
    let mut head = [0u8; 2];
    take(c, &mut head)?;
    if head[0] != 5 {
        return Err(None);
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    take(c, &mut methods)?;
    if !methods.contains(&0) {
        let _ = c.write_all(&[5, 0xff]);
        return Err(None);
    }
    c.write_all(&[5, 0]).map_err(|_| None)?;
    let mut req = [0u8; 4];
    take(c, &mut req)?;
    if req[0] != 5 {
        return Err(None);
    }
    // Запрос дочитываем до конца и только потом отказываем: закрытый сокет с
    // непрочитанными байтами Windows обрывает (RST), и ответ мог бы не дойти.
    let host = match req[3] {
        1 => {
            let mut ip = [0u8; 4];
            take(c, &mut ip)?;
            Some(Host::Ip(IpAddr::from(ip)))
        }
        3 => {
            let mut len = [0u8; 1];
            take(c, &mut len)?;
            let mut name = vec![0u8; usize::from(len[0])];
            take(c, &mut name)?;
            String::from_utf8(name)
                .ok()
                .and_then(|name| host_from(&name))
        }
        4 => {
            let mut ip = [0u8; 16];
            take(c, &mut ip)?;
            Some(Host::Ip(IpAddr::from(ip)))
        }
        _ => return Err(Some(reply::ADDRESS)),
    };
    let mut port = [0u8; 2];
    take(c, &mut port)?;
    // Только CONNECT: BIND и UDP движку не нужны.
    if req[1] != 1 {
        return Err(Some(reply::COMMAND));
    }
    let host = host.ok_or(Some(reply::ADDRESS))?;
    Ok(Target {
        host,
        port: u16::from_be_bytes(port),
    })
}

fn take(c: &mut TcpStream, buf: &mut [u8]) -> Result<(), Option<u8>> {
    c.read_exact(buf).map_err(|_| None)
}

/// Имя из запроса: IP-адрес строкой или имя хоста без лишних знаков (оно уходит
/// в строку CONNECT HTTP-прокси — перевод строки там недопустим).
fn host_from(name: &str) -> Option<Host> {
    let bare = name
        .strip_prefix('[')
        .and_then(|n| n.strip_suffix(']'))
        .unwrap_or(name);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Some(Host::Ip(ip));
    }
    let ok = !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    ok.then(|| Host::Name(name.to_string()))
}

fn answer(c: &mut TcpStream, code: u8) -> io::Result<()> {
    c.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0])
}

/// Соединение с сайтом по маршруту; второе — байты, которые прокси прислал сразу
/// за ответом (их надо отдать движку первыми).
fn open(route: &Route, target: &Target) -> Result<(TcpStream, Vec<u8>), Fail> {
    match route {
        Route::Direct => direct(target).map(|s| (s, Vec::new())),
        Route::Socks5(up) => {
            let mut s = dial(up).map_err(|_| Fail::Proxy)?;
            socks5_connect(&mut s, target)?;
            Ok((s, Vec::new()))
        }
        Route::Http(up) => {
            let mut s = dial(up).map_err(|_| Fail::Proxy)?;
            let early = http_connect(&mut s, target)?;
            Ok((s, early))
        }
    }
}

fn direct(target: &Target) -> Result<TcpStream, Fail> {
    let addrs: Vec<SocketAddr> = match &target.host {
        Host::Ip(ip) => vec![SocketAddr::new(*ip, target.port)],
        Host::Name(name) => (name.as_str(), target.port)
            .to_socket_addrs()
            .map_err(|_| Fail::Code(reply::HOST_UNREACHABLE))?
            .collect(),
    };
    connect_any(&addrs).map_err(|err| Fail::Code(code_for(&err)))
}

/// Соединение с прокси человека: с таймаутами на рукопожатие.
fn dial(up: &Upstream) -> io::Result<TcpStream> {
    let addrs: Vec<SocketAddr> = (up.host.as_str(), up.port).to_socket_addrs()?.collect();
    let s = connect_any(&addrs)?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(HANDSHAKE))?;
    s.set_write_timeout(Some(HANDSHAKE))?;
    Ok(s)
}

fn connect_any(addrs: &[SocketAddr]) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "у имени нет адресов");
    for (i, addr) in addrs.iter().enumerate() {
        let wait = if i + 1 < addrs.len() {
            CONNECT_EACH
        } else {
            CONNECT
        };
        match TcpStream::connect_timeout(addr, wait) {
            Ok(s) => return Ok(s),
            Err(err) => last = err,
        }
    }
    Err(last)
}

fn code_for(err: &io::Error) -> u8 {
    match err.kind() {
        io::ErrorKind::ConnectionRefused => reply::REFUSED,
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => reply::TIMED_OUT,
        io::ErrorKind::NetworkUnreachable => reply::NET_UNREACHABLE,
        io::ErrorKind::HostUnreachable | io::ErrorKind::NotFound => reply::HOST_UNREACHABLE,
        _ => reply::FAILURE,
    }
}

/// SOCKS5 CONNECT у прокси человека: тот же адрес, что прислал движок.
fn socks5_connect(s: &mut TcpStream, target: &Target) -> Result<(), Fail> {
    s.write_all(&[5, 1, 0]).map_err(|_| Fail::Proxy)?;
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello).map_err(|_| Fail::Proxy)?;
    if hello != [5, 0] {
        return Err(Fail::Proxy);
    }
    let mut req = vec![5, 1, 0];
    match &target.host {
        Host::Ip(IpAddr::V4(ip)) => {
            req.push(1);
            req.extend_from_slice(&ip.octets());
        }
        Host::Ip(IpAddr::V6(ip)) => {
            req.push(4);
            req.extend_from_slice(&ip.octets());
        }
        Host::Name(name) => {
            // `host_from` пропускает не длиннее 253.
            let len = u8::try_from(name.len()).map_err(|_| Fail::Code(reply::ADDRESS))?;
            req.push(3);
            req.push(len);
            req.extend_from_slice(name.as_bytes());
        }
    }
    req.extend_from_slice(&target.port.to_be_bytes());
    s.write_all(&req).map_err(|_| Fail::Proxy)?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).map_err(|_| Fail::Proxy)?;
    if head[0] != 5 {
        return Err(Fail::Proxy);
    }
    if head[1] != reply::OK {
        return Err(Fail::Code(head[1]));
    }
    let skip = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).map_err(|_| Fail::Proxy)?;
            usize::from(len[0])
        }
        _ => return Err(Fail::Proxy),
    };
    let mut bound = vec![0u8; skip + 2];
    s.read_exact(&mut bound).map_err(|_| Fail::Proxy)?;
    Ok(())
}

/// CONNECT у HTTP-прокси. Возвращает байты, пришедшие сразу за заголовком ответа.
fn http_connect(s: &mut TcpStream, target: &Target) -> Result<Vec<u8>, Fail> {
    let authority = match &target.host {
        Host::Name(name) => format!("{name}:{}", target.port),
        Host::Ip(IpAddr::V6(ip)) => format!("[{ip}]:{}", target.port),
        Host::Ip(ip) => format!("{ip}:{}", target.port),
    };
    let request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
    s.write_all(request.as_bytes()).map_err(|_| Fail::Proxy)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > MAX_HTTP_HEAD {
            return Err(Fail::Proxy);
        }
        let n = s.read(&mut chunk).map_err(|_| Fail::Proxy)?;
        if n == 0 {
            return Err(Fail::Proxy);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let status = http_status(&buf[..end]).ok_or(Fail::Proxy)?;
    match status {
        200..=299 => Ok(buf[end..].to_vec()),
        403 | 407 => Err(Fail::Code(reply::NOT_ALLOWED)),
        502..=504 => Err(Fail::Code(reply::HOST_UNREACHABLE)),
        _ => Err(Fail::Code(reply::FAILURE)),
    }
}

/// `HTTP/1.1 200 Connection established` → 200.
fn http_status(head: &[u8]) -> Option<u16> {
    let line = head.split(|&b| b == b'\r').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split_ascii_whitespace();
    parts.next().filter(|v| v.starts_with("HTTP/1."))?;
    parts.next()?.parse().ok()
}

/// Гоняет байты в обе стороны. Закончилось одно направление — закрываем оба:
/// движку и сайтам полузакрытые соединения не нужны, а повисший поток — лишний.
fn pipe(client: &TcpStream, upstream: &TcpStream) {
    for s in [client, upstream] {
        let _ = s.set_read_timeout(None);
        let _ = s.set_write_timeout(None);
    }
    let (Ok(mut from_client), Ok(mut to_upstream)) = (client.try_clone(), upstream.try_clone())
    else {
        return;
    };
    let up = thread::Builder::new()
        .name("axiom-relay-pipe".into())
        .stack_size(128 * 1024)
        .spawn(move || {
            let _ = io::copy(&mut from_client, &mut to_upstream);
            let _ = from_client.shutdown(Shutdown::Both);
            let _ = to_upstream.shutdown(Shutdown::Both);
        });
    let _ = io::copy(&mut &*upstream, &mut &*client);
    let _ = client.shutdown(Shutdown::Both);
    let _ = upstream.shutdown(Shutdown::Both);
    if let Ok(up) = up {
        let _ = up.join();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// Эхо-сервер: что прислали, то и вернул.
    fn echo() -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || {
                    let mut s = stream;
                    let mut r = s.try_clone().unwrap();
                    let _ = io::copy(&mut r, &mut s);
                });
            }
        });
        addr
    }

    /// Порт, на котором никто не слушает.
    fn dead_port() -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    /// SOCKS5-прокси для теста: записывает, куда просили, и ведёт всё на `to`.
    fn fake_socks(to: SocketAddr) -> (SocketAddr, mpsc::Receiver<(u8, String, u16)>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let tx = tx.clone();
                thread::spawn(move || {
                    let mut c = stream;
                    let mut head = [0u8; 2];
                    c.read_exact(&mut head).unwrap();
                    let mut methods = vec![0u8; usize::from(head[1])];
                    c.read_exact(&mut methods).unwrap();
                    c.write_all(&[5, 0]).unwrap();
                    let mut req = [0u8; 4];
                    c.read_exact(&mut req).unwrap();
                    let host = match req[3] {
                        3 => {
                            let mut len = [0u8; 1];
                            c.read_exact(&mut len).unwrap();
                            let mut name = vec![0u8; usize::from(len[0])];
                            c.read_exact(&mut name).unwrap();
                            String::from_utf8(name).unwrap()
                        }
                        _ => {
                            let mut ip = [0u8; 4];
                            c.read_exact(&mut ip).unwrap();
                            Ipv4Addr::from(ip).to_string()
                        }
                    };
                    let mut port = [0u8; 2];
                    c.read_exact(&mut port).unwrap();
                    tx.send((req[3], host, u16::from_be_bytes(port))).unwrap();
                    c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
                    let s = TcpStream::connect(to).unwrap();
                    pipe(&c, &s);
                });
            }
        });
        (addr, rx)
    }

    /// HTTP-прокси для теста: записывает строку CONNECT и ведёт всё на `to`.
    fn fake_http(to: SocketAddr) -> (SocketAddr, mpsc::Receiver<String>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let tx = tx.clone();
                thread::spawn(move || {
                    let mut c = stream;
                    let mut buf = Vec::new();
                    let mut byte = [0u8; 1];
                    while !buf.ends_with(b"\r\n\r\n") {
                        c.read_exact(&mut byte).unwrap();
                        buf.push(byte[0]);
                    }
                    let line = String::from_utf8_lossy(&buf)
                        .lines()
                        .next()
                        .unwrap()
                        .to_string();
                    tx.send(line).unwrap();
                    c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .unwrap();
                    let s = TcpStream::connect(to).unwrap();
                    pipe(&c, &s);
                });
            }
        });
        (addr, rx)
    }

    /// Как движок: приветствие и CONNECT к имени. Возвращает сокет и код ответа.
    fn connect(relay: SocketAddr, host: &str, port: u16) -> (TcpStream, u8) {
        let mut s = TcpStream::connect(relay).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(&[5, 1, 0]).unwrap();
        let mut hello = [0u8; 2];
        s.read_exact(&mut hello).unwrap();
        assert_eq!(hello, [5, 0]);
        let mut req = vec![5, 1, 0, 3, u8::try_from(host.len()).unwrap()];
        req.extend_from_slice(host.as_bytes());
        req.extend_from_slice(&port.to_be_bytes());
        s.write_all(&req).unwrap();
        let mut answer = [0u8; 10];
        let code = match s.read_exact(&mut answer) {
            Ok(()) => answer[1],
            Err(_) => 0xff,
        };
        (s, code)
    }

    fn roundtrip(s: &mut TcpStream) -> bool {
        if s.write_all(b"ping").is_err() {
            return false;
        }
        let mut back = [0u8; 4];
        s.read_exact(&mut back).is_ok() && &back == b"ping"
    }

    fn upstream(addr: SocketAddr) -> Upstream {
        Upstream {
            host: addr.ip().to_string(),
            port: addr.port(),
        }
    }

    #[test]
    fn direct_connects_by_itself() {
        let echo = echo();
        let relay = Relay::start(Route::Direct, |_| {}).unwrap();
        let (mut s, code) = connect(relay.addr(), "127.0.0.1", echo.port());
        assert_eq!(code, reply::OK);
        assert!(roundtrip(&mut s));
    }

    #[test]
    fn socks5_gets_the_name_not_an_address() {
        let echo = echo();
        let (proxy, asked) = fake_socks(echo);
        let relay = Relay::start(Route::Socks5(upstream(proxy)), |_| {}).unwrap();
        let (mut s, code) = connect(relay.addr(), "example.com", 443);
        assert_eq!(code, reply::OK);
        assert!(roundtrip(&mut s));
        // atyp=3: имя разрешает прокси, своего DNS-запроса нет.
        assert_eq!(
            asked.recv_timeout(Duration::from_secs(5)).unwrap(),
            (3, "example.com".to_string(), 443)
        );
        assert_eq!(relay.health(), Health::Up);
    }

    #[test]
    fn http_proxy_gets_connect() {
        let echo = echo();
        let (proxy, asked) = fake_http(echo);
        let relay = Relay::start(Route::Http(upstream(proxy)), |_| {}).unwrap();
        let (mut s, code) = connect(relay.addr(), "example.com", 443);
        assert_eq!(code, reply::OK);
        assert!(roundtrip(&mut s));
        assert_eq!(
            asked.recv_timeout(Duration::from_secs(5)).unwrap(),
            "CONNECT example.com:443 HTTP/1.1"
        );
    }

    #[test]
    fn dead_proxy_blocks_instead_of_going_direct() {
        let echo = echo();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let dead = Upstream {
            host: "127.0.0.1".into(),
            port: dead_port(),
        };
        let relay = Relay::start(Route::Socks5(dead), move |h| {
            let _ = lock(&tx).send(h);
        })
        .unwrap();
        // Сайт живой и доступен напрямую, но прокси молчит — соединения нет.
        let (mut s, code) = connect(relay.addr(), "127.0.0.1", echo.port());
        assert_eq!(code, reply::FAILURE);
        assert!(!roundtrip(&mut s));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Health::Down
        );
        assert_eq!(relay.health(), Health::Down);
    }

    #[test]
    fn switching_route_cuts_open_connections() {
        let echo = echo();
        let (proxy, asked) = fake_socks(echo);
        let relay = Relay::start(Route::Direct, |_| {}).unwrap();
        let (mut s, code) = connect(relay.addr(), "127.0.0.1", echo.port());
        assert_eq!(code, reply::OK);
        assert!(roundtrip(&mut s));
        assert_eq!(relay.open_connections(), 1);

        relay.set_route(Route::Socks5(upstream(proxy)));
        // Прежнее соединение шло напрямую — оно оборвано.
        let mut rest = Vec::new();
        assert!(matches!(s.read_to_end(&mut rest), Ok(0) | Err(_)));

        // Новое идёт уже через прокси.
        let (mut s, code) = connect(relay.addr(), "example.com", 80);
        assert_eq!(code, reply::OK);
        assert!(roundtrip(&mut s));
        assert_eq!(
            asked.recv_timeout(Duration::from_secs(5)).unwrap().1,
            "example.com"
        );
    }

    #[test]
    fn check_reports_health() {
        let echo = echo();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let relay = Relay::start(Route::Socks5(upstream(echo)), move |h| {
            let _ = lock(&tx).send(h);
        })
        .unwrap();
        relay.check();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Health::Up);
        relay.set_route(Route::Socks5(Upstream {
            host: "127.0.0.1".into(),
            port: dead_port(),
        }));
        assert_eq!(relay.health(), Health::Unknown);
        relay.check();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Health::Down
        );
    }

    #[test]
    fn refuses_odd_requests() {
        let relay = Relay::start(Route::Direct, |_| {}).unwrap();
        // Перевод строки в имени ушёл бы в заголовок CONNECT.
        let (_, code) = connect(relay.addr(), "a.com\r\nX: y", 80);
        assert_eq!(code, reply::ADDRESS);
        // BIND не поддерживаем.
        let mut s = TcpStream::connect(relay.addr()).unwrap();
        s.write_all(&[5, 1, 0]).unwrap();
        let mut hello = [0u8; 2];
        s.read_exact(&mut hello).unwrap();
        s.write_all(&[5, 2, 0, 1, 127, 0, 0, 1, 0, 80]).unwrap();
        let mut answer = [0u8; 10];
        s.read_exact(&mut answer).unwrap();
        assert_eq!(answer[1], reply::COMMAND);
        // Не SOCKS5 — просто закрываем.
        let mut s = TcpStream::connect(relay.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let mut rest = Vec::new();
        assert!(matches!(s.read_to_end(&mut rest), Ok(0) | Err(_)));
    }

    #[test]
    fn names_and_statuses() {
        assert_eq!(
            host_from("[::1]"),
            Some(Host::Ip(IpAddr::from([0u16, 0, 0, 0, 0, 0, 0, 1])))
        );
        assert_eq!(
            host_from("my_host.example"),
            Some(Host::Name("my_host.example".into()))
        );
        assert_eq!(host_from(""), None);
        assert_eq!(host_from("a b"), None);
        assert_eq!(http_status(b"HTTP/1.1 200 OK\r\n\r\n"), Some(200));
        assert_eq!(
            http_status(b"HTTP/1.0 407 Proxy Authentication Required\r\n\r\n"),
            Some(407)
        );
        assert_eq!(http_status(b"SSH-2.0\r\n\r\n"), None);
    }
}
