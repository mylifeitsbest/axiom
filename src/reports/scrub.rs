//! Очистка текста отчёта от личного перед отправкой.
//!
//! Что убирается:
//! - адреса: от `http(s)://`, `ws(s)://`, `ftp://` остаётся только сайт
//!   (`example.com`), IP-адрес сайта (кроме 127.0.0.1) — `<ip>`; остальные схемы
//!   (`file:`, `data:`, `blob:`, `javascript:`, `mailto:`, `socks5://`…) — `<url>`;
//! - домашняя папка и любые `X:\Users\имя`, `/home/имя` — `~`;
//! - почта — `<email>`;
//! - имя компьютера и пользователя целым словом — `<pc>`, `<user>`;
//! - длинные шестнадцатеричные строки (24+) — `<hex>`, похожие на ключи и токены
//!   (32+ символа, буквы, цифры и регистр вперемешку) — `<token>`.
//!
//! Очистка идемпотентна: повторный проход ничего не меняет.

/// Что в тексте выдаёт человека: домашняя папка, имя пользователя и компьютера.
#[derive(Debug, Clone, Default)]
pub struct Private {
    homes: Vec<String>,
    users: Vec<String>,
    hosts: Vec<String>,
}

impl Private {
    /// Данные этого компьютера.
    pub fn current() -> Self {
        let mut homes = Vec::new();
        let mut users = Vec::new();
        if let Some(base) = directories::BaseDirs::new() {
            let home = base.home_dir().to_string_lossy().into_owned();
            if let Some(name) = home.rsplit(['\\', '/']).find(|s| !s.is_empty()) {
                users.push(name.to_string());
            }
            homes.push(home);
        }
        for var in ["USERPROFILE", "HOME"] {
            if let Ok(home) = std::env::var(var) {
                homes.push(home);
            }
        }
        for var in ["USERNAME", "USER", "LOGNAME"] {
            if let Ok(name) = std::env::var(var) {
                users.push(name);
            }
        }
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut hosts: Vec<String> = ["COMPUTERNAME", "HOSTNAME"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .collect();
        #[cfg(target_os = "linux")]
        if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
            hosts.push(name.trim().to_string());
        }
        Self::new(homes, users, hosts)
    }

    pub fn new(homes: Vec<String>, users: Vec<String>, hosts: Vec<String>) -> Self {
        let mut homes: Vec<String> = homes
            .into_iter()
            .map(|h| h.trim().trim_end_matches(['\\', '/']).to_string())
            // Корень диска или `/` домашней папкой не считаем: заменили бы все пути.
            .filter(|h| h.chars().count() >= 4)
            .flat_map(|h| {
                // Тот же путь в другой записи: прямые слэши и двойные обратные (Debug, JSON).
                [
                    h.replace('\\', "/"),
                    h.replace('\\', "\\\\"),
                    h.replace('/', "\\"),
                    h,
                ]
            })
            .collect();
        // Сначала длинные: `C:\\Users\\x` не должен заменяться по частям.
        homes.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        homes.dedup();
        let words = |v: Vec<String>| {
            let mut v: Vec<String> = v
                .into_iter()
                .map(|s| s.trim().to_string())
                // Имя из одной-двух букв заменяло бы обычные слова.
                .filter(|s| s.chars().count() >= 3 && !s.eq_ignore_ascii_case("localhost"))
                .collect();
            v.sort_by(|a, b| {
                b.len()
                    .cmp(&a.len())
                    .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
            });
            v.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
            v
        };
        Self {
            homes,
            users: words(users),
            hosts: words(hosts),
        }
    }

    pub fn scrub(&self, text: &str) -> String {
        let text = urls(text);
        let text = user_dirs(&text, &self.homes);
        let mut text = emails(&text);
        // Имя компьютера часто содержит имя пользователя (`roman-pc`): его первым.
        for host in &self.hosts {
            text = replace_ci(&text, host, "<pc>", word_edges);
        }
        for user in &self.users {
            text = replace_ci(&text, user, "<user>", word_edges);
        }
        tokens(&text)
    }
}

// ---------- адреса ----------

/// Схемы без `//`, от которых не оставляем ничего: внутри путь, данные или код.
const OPAQUE: [&str; 6] = ["file", "data", "blob", "javascript", "mailto", "filesystem"];

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
}

/// Конец адреса с `//`: пробел, кавычка или угловая скобка.
fn ends_url(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`')
}

/// Конец `data:` и подобных: внутри бывает разметка, поэтому только пробел и кавычки.
fn ends_opaque(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`')
}

fn urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((start, scheme_len, opaque)) = find_url(rest) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let stop = if opaque { ends_opaque } else { ends_url };
        let mut end = tail.find(stop).unwrap_or(tail.len());
        // Знаки препинания после адреса — не часть его («см. https://a.b/c).»),
        // а закрывающая скобка — только лишняя: `javascript:f(x)` целиком адрес.
        while end > scheme_len + 1 {
            let url = &tail[..end];
            let extra =
                |open: char, close: char| url.matches(close).count() > url.matches(open).count();
            let trim = match url.chars().next_back() {
                Some('.' | ',' | ';' | ':' | '!' | '?') => true,
                Some(')') => extra('(', ')'),
                Some(']') => extra('[', ']'),
                Some('}') => extra('{', '}'),
                _ => false,
            };
            if !trim {
                break;
            }
            end -= 1;
        }
        let scheme = tail[..scheme_len].to_ascii_lowercase();
        out.push_str(&replace_url(&tail[..end], &scheme));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Ближайший адрес: (начало, длина схемы, схема без `//`).
fn find_url(text: &str) -> Option<(usize, usize, bool)> {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(pos) = text[from..].find(':').map(|p| p + from) {
        from = pos + 1;
        // Схема — латиница перед двоеточием, начиная с буквы.
        let mut start = pos;
        while start > 0 && is_scheme_char(bytes[start - 1] as char) {
            start -= 1;
        }
        while start < pos && !bytes[start].is_ascii_alphabetic() {
            start += 1;
        }
        if start == pos || pos - start > 32 {
            continue;
        }
        let scheme = &text[start..pos];
        let after = &text[pos + 1..];
        if let Some(rest) = after.strip_prefix("//")
            && rest.chars().next().is_some_and(|c| !ends_url(c))
        {
            return Some((start, pos - start, false));
        }
        let opaque = OPAQUE.iter().any(|s| s.eq_ignore_ascii_case(scheme));
        if opaque && after.chars().next().is_some_and(|c| !ends_opaque(c)) {
            return Some((start, pos - start, true));
        }
    }
    None
}

fn replace_url(url: &str, scheme: &str) -> String {
    if !matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp") {
        return "<url>".into();
    }
    match url::Url::parse(url)
        .ok()
        .and_then(|u| u.host().map(|h| h.to_owned()))
    {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) if ip.is_loopback() => ip.to_string(),
        Some(url::Host::Ipv6(ip)) if ip.is_loopback() => "[::1]".into(),
        Some(_) => "<ip>".into(),
        None => "<url>".into(),
    }
}

// ---------- пути и имена ----------

fn user_dirs(text: &str, homes: &[String]) -> String {
    let mut text = text.to_string();
    for home in homes {
        text = replace_ci(&text, home, "~", path_edges);
    }
    // Папки любых пользователей, в том числе в коротком виде `C:\Users\ROMAN~1`.
    for prefix in [
        ":\\\\Users\\\\",
        ":\\Users\\",
        ":/Users/",
        "/home/",
        "/Users/",
    ] {
        text = replace_dirs(&text, prefix);
    }
    text
}

/// `X:\Users\имя` → `~`: префикс, затем имя до следующего разделителя.
fn replace_dirs(text: &str, prefix: &str) -> String {
    let drive = prefix.starts_with(':');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((at, end)) = find_ci(rest, prefix) {
        let b = rest.as_bytes();
        // У Windows-пути перед `:` — буква диска на границе слова.
        let start = if drive {
            let ok = at >= 1
                && b[at - 1].is_ascii_alphabetic()
                && (at == 1 || !(b[at - 2] as char).is_ascii_alphanumeric());
            ok.then(|| at - 1)
        } else {
            Some(at)
        };
        let tail = &rest[end..];
        let name_len = tail
            .find(|c: char| ends_url(c) || matches!(c, '/' | '\\' | ':' | ',' | ';' | ')' | ']'))
            .unwrap_or(tail.len());
        match start {
            Some(start) if name_len > 0 => {
                out.push_str(&rest[..start]);
                out.push('~');
                rest = &tail[name_len..];
            }
            _ => {
                out.push_str(&rest[..end]);
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Сравнение без учёта регистра по символам (кириллица тоже).
fn same_ci(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// Первое вхождение `needle` без учёта регистра: (начало, конец) в байтах.
fn find_ci(hay: &str, needle: &str) -> Option<(usize, usize)> {
    let first = needle.chars().next()?;
    'outer: for (i, c) in hay.char_indices() {
        if !same_ci(c, first) {
            continue;
        }
        let mut end = i;
        let mut hay_chars = hay[i..].chars();
        for n in needle.chars() {
            match hay_chars.next() {
                Some(h) if same_ci(h, n) => end += h.len_utf8(),
                _ => continue 'outer,
            }
        }
        return Some((i, end));
    }
    None
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Имя — целым словом.
fn word_edges(before: Option<char>, after: Option<char>) -> bool {
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

/// Домашняя папка — только целиком: `C:\Users\Roman` не должна задеть `C:\Users\ROMAN~1`.
fn path_edges(before: Option<char>, after: Option<char>) -> bool {
    !before.is_some_and(is_word_char)
        && !after.is_some_and(|c| is_word_char(c) || matches!(c, '~' | '.' | '-'))
}

/// Замена без учёта регистра там, где `edges(символ до, символ после)` разрешает.
fn replace_ci(
    text: &str,
    needle: &str,
    with: &str,
    edges: fn(Option<char>, Option<char>) -> bool,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((start, end)) = find_ci(rest, needle) {
        let before = rest[..start]
            .chars()
            .next_back()
            .or(out.chars().next_back());
        let after = rest[end..].chars().next();
        out.push_str(&rest[..start]);
        if edges(before, after) {
            out.push_str(with);
        } else {
            out.push_str(&rest[start..end]);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

// ---------- почта ----------

fn is_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

fn is_domain_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

fn emails(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        let left = rest[..at]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| is_local_char(c))
            .last()
            .map_or(at, |(i, _)| i);
        let right_len = rest[at + 1..]
            .find(|c: char| !is_domain_char(c))
            .unwrap_or(rest.len() - at - 1);
        let domain = rest[at + 1..at + 1 + right_len].trim_end_matches(['.', '-']);
        let tld_ok = domain.rsplit_once('.').is_some_and(|(host, tld)| {
            !host.is_empty() && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
        });
        if left < at && tld_ok {
            out.push_str(&rest[..left]);
            out.push_str("<email>");
            rest = &rest[at + 1 + domain.len()..];
        } else {
            out.push_str(&rest[..=at]);
            rest = &rest[at + 1..];
        }
    }
    out.push_str(rest);
    out
}

// ---------- токены ----------

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '-')
}

fn tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(is_token_char) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let len = tail.find(|c| !is_token_char(c)).unwrap_or(tail.len());
        let run = &tail[..len];
        match classify(run) {
            Some(mark) => {
                out.push_str(mark);
                // `=` в конце base64 — часть токена.
                let pad = tail[len..].len() - tail[len..].trim_start_matches('=').len();
                rest = &tail[len + pad..];
            }
            None => {
                out.push_str(run);
                rest = &tail[len..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Чем заменить отрезок без разделителей, если он похож на ключ или токен.
fn classify(run: &str) -> Option<&'static str> {
    let hex = |p: &str| p.len() >= 24 && p.bytes().all(|b| b.is_ascii_hexdigit());
    // Шестнадцатеричные куски внутри (`id-0123…`) и GUID — тоже.
    let guid = run.len() >= 32 && run.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    if hex(run) || guid || run.split(['-', '_']).any(hex) {
        return Some("<hex>");
    }
    if run.len() < 32 {
        return None;
    }
    let bytes = run.as_bytes();
    let has = |f: fn(&u8) -> bool| bytes.iter().any(f);
    let letters: Vec<u8> = bytes
        .iter()
        .filter(|b| b.is_ascii_alphabetic())
        .map(u8::to_ascii_lowercase)
        .collect();
    let vowels = letters.iter().filter(|b| b"aeiou".contains(b)).count();
    // Гласных в именах и словах около 40 %, в случайной строке — около 20 %.
    let few_vowels = letters.len() >= 12 && vowels * 4 < letters.len();
    // Имя вроде `CoreWebView2ProcessFailedEventArgs2` почти целиком из «слов»
    // (заглавная и строчные), а в случайной строке их меньше половины.
    let mixed =
        has(u8::is_ascii_lowercase) && has(u8::is_ascii_uppercase) && has(u8::is_ascii_digit);
    let few_words = mixed && words_len(bytes) * 10 < bytes.len() * 7;
    (letters.len() >= 4 && (few_vowels || few_words)).then_some("<token>")
}

/// Сколько символов покрывают «слова»: `[A-Z]?[a-z]{2,}`.
fn words_len(bytes: &[u8]) -> usize {
    let mut covered = 0;
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if bytes[i].is_ascii_uppercase() {
            i += 1;
        }
        let lower = i;
        while i < bytes.len() && bytes[i].is_ascii_lowercase() {
            i += 1;
        }
        if i - lower >= 2 {
            covered += i - start;
        } else if i == start {
            i += 1;
        }
    }
    covered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private() -> Private {
        Private::new(
            vec![r"C:\Users\Roman".into(), "/home/roman/".into()],
            vec!["Roman".into(), "roman".into(), "a".into()],
            vec!["ROMAN-PC".into(), "localhost".into()],
        )
    }

    fn scrub(s: &str) -> String {
        private().scrub(s)
    }

    #[test]
    fn urls_keep_only_the_site() {
        assert_eq!(
            scrub("ошибка: https://mail.example.com/inbox?token=abc#x."),
            "ошибка: mail.example.com."
        );
        assert_eq!(
            scrub("(см. http://User:pw@Example.org:8080/a)"),
            "(см. example.org)"
        );
        assert_eq!(scrub("wss://chat.example.net/socket"), "chat.example.net");
        assert_eq!(
            scrub("view-source:https://a.example/b"),
            "view-source:a.example"
        );
        assert_eq!(
            scrub("прокси socks5://10.1.2.3:1080 не ответил"),
            "прокси <url> не ответил"
        );
        assert_eq!(scrub("http://93.184.216.34/x"), "<ip>");
        assert_eq!(scrub("http://127.0.0.1:8787/r"), "127.0.0.1");
        assert_eq!(scrub("\"https://a.example/x\""), "\"a.example\"");
        assert_eq!(scrub("https://пример.рф/путь"), "xn--e1afmkfd.xn--p1ai");
    }

    #[test]
    fn opaque_urls_are_dropped() {
        assert_eq!(scrub("file:///C:/Users/Roman/secret.html"), "<url>");
        assert_eq!(scrub("open file:C:/x/y.pdf now"), "open <url> now");
        assert_eq!(scrub("data:text/html,<b>hi</b> end"), "<url> end");
        assert_eq!(scrub("blob:https://a.example/123-456"), "<url>");
        assert_eq!(scrub("javascript:alert(document.cookie)"), "<url>");
        assert_eq!(scrub("mailto:someone@example.com"), "<url>");
        assert_eq!(scrub("axiom://settings"), "<url>");
        // Не адреса: двоеточие в обычном тексте, время и «metadata:».
        assert_eq!(scrub("Файл: готов"), "Файл: готов");
        assert_eq!(scrub("metadata: ok, data: 5"), "metadata: ok, data: 5");
        assert_eq!(scrub("в 12:30:45 UTC"), "в 12:30:45 UTC");
    }

    #[test]
    fn home_and_user_names_go_away() {
        assert_eq!(
            scrub(r"не удалось записать C:\Users\Roman\AppData\Local\Axiom\data\x.json"),
            r"не удалось записать ~\AppData\Local\Axiom\data\x.json"
        );
        assert_eq!(scrub(r"c:\users\roman\Downloads"), r"~\Downloads");
        assert_eq!(scrub("C:/Users/Roman/Desktop"), "~/Desktop");
        assert_eq!(scrub(r#"path: "C:\\Users\\Roman\\x""#), r#"path: "~\\x""#);
        assert_eq!(
            scrub("/home/roman/.local/share/axiom"),
            "~/.local/share/axiom"
        );
        // Чужие и короткие имена папок тоже.
        assert_eq!(
            scrub(r"C:\Users\ROMAN~1\AppData\Local\Temp"),
            r"~\AppData\Local\Temp"
        );
        assert_eq!(scrub(r"D:\Users\Guest\file"), r"~\file");
        assert_eq!(scrub("/home/alice/file, ok"), "~/file, ok");
        // Имя пользователя и компьютера — целым словом, без учёта регистра.
        assert_eq!(scrub("вошёл ROMAN на roman-pc"), "вошёл <user> на <pc>");
        assert_eq!(scrub("Romanov и romantic"), "Romanov и romantic");
        // Имя из одной буквы и localhost не трогаем.
        assert_eq!(scrub("a b c localhost"), "a b c localhost");
    }

    #[test]
    fn cyrillic_user_name() {
        let p = Private::new(vec![r"C:\Users\Роман".into()], vec!["Роман".into()], vec![]);
        assert_eq!(p.scrub(r"C:\Users\роман\x и РОМАН"), r"~\x и <user>");
    }

    #[test]
    fn emails_and_tokens() {
        assert_eq!(
            scrub("пишите на john.doe+axiom@mail.example.co.uk."),
            "пишите на <email>."
        );
        assert_eq!(scrub("user@localhost и a@b"), "user@localhost и a@b");
        assert_eq!(
            scrub("sha 3f786850e387550fdab836ed7e6dc881de23001b"),
            "sha <hex>"
        );
        assert_eq!(scrub("id 550e8400-e29b-41d4-a716-446655440000"), "id <hex>");
        assert_eq!(
            scrub("jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4ifQ"),
            "jwt eyJhbGciOiJIUzI1NiJ9.<token>"
        );
        assert_eq!(
            scrub("key=AbC9dEf1GhI2jKl3MnO4pQr5StU6vWx7YzA8== end"),
            "key=<token> end"
        );
        // Обычные длинные имена, адреса в памяти и хэши символов Rust остаются.
        for keep in [
            "webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2ProcessFailedEventHandler",
            "CoreWebView2ProcessFailedEventArgs2",
            "COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED",
            "this_is_a_rather_long_function_name_2024",
            "  12:     0x7ff6a1b2c3d4 - core::panicking::panic::h1a2b3c4d5e6f7a8b",
            "не удалось создать webview вкладки: WebView2 error 0x80070002",
            "Axiom 0.5.4-beta.312 · Windows 11 (сборка 26200) · WebView2 142.0.3595.53",
        ] {
            assert_eq!(scrub(keep), keep);
        }
    }

    #[test]
    fn random_tokens_are_caught() {
        const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+_";
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut caught = 0;
        for _ in 0..1000 {
            let token: String = (0..32)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    B64[(seed % 64) as usize] as char
                })
                .collect();
            if scrub(&format!("t={token}")) == "t=<token>" {
                caught += 1;
            }
        }
        assert!(caught >= 970, "поймано {caught} из 1000");
    }

    /// В release паника = аварийное завершение, а очистка работает и в обработчике
    /// паники, и при запуске: на любом мусоре она не должна падать.
    #[test]
    fn never_panics_on_garbage() {
        const PARTS: [&str; 24] = [
            ":", "//", "/", "\\", "\\\\", "@", ".", "-", "=", "~", "é", "ё", "İ", "ß", "😀",
            "http", "file:", "data:", "C:", "Users", "home", "Roman", " ", "\n",
        ];
        let p = Private::new(
            vec![r"C:\Users\Roman".into(), "/home/İß".into()],
            vec!["Roman".into(), "İßé".into()],
            vec!["roman-pc".into()],
        );
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..5000 {
            let mut text = String::new();
            for _ in 0..(seed % 24) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push_str(PARTS[(seed % PARTS.len() as u64) as usize]);
                if seed.is_multiple_of(5) {
                    text.push_str("aB3xY9zQ7wE2rT5uI8oP1aS4dF6gH0jK");
                }
            }
            let once = p.scrub(&text);
            let _ = p.scrub(&once);
        }
    }

    #[test]
    fn scrub_is_idempotent() {
        let raw = "https://a.example/x C:\\Users\\Roman\\y roman@example.com \
                   3f786850e387550fdab836ed7e6dc881de23001b data:x,y ROMAN-PC";
        let once = scrub(raw);
        assert_eq!(once, "a.example ~\\y <email> <hex> <url> <pc>");
        assert_eq!(scrub(&once), once);
    }
}
