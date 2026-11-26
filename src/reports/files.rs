//! Файлы к «Сообщить о проблеме»: снимки экрана, фото, видео и текст.
//!
//! Интерфейс читает выбранный файл сам и шлёт его кусками (base64): они ложатся
//! в `reports/new/<место>`. «Отправить» забирает эти файлы под отчёт ([`take`]),
//! перед отправкой из них вырезается личное ([`clean`]): у фото и видео — метаданные
//! с местом съёмки и камерой (см. [`super::meta`]), текст чистится как журнал.
//! Тип узнаётся по первым байтам, а не по имени: принимаются только те, что панель
//! разработчика умеет показать.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{meta, scrub};

/// Файлов к одному отчёту — не больше стольких.
pub const MAX_FILES: usize = 10;
/// Все файлы отчёта вместе — не больше 50 МБ (столько же примет сервер).
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;
/// Кусок файла в одном сообщении интерфейса, до base64: так он влезает в предел IPC.
pub const MAX_CHUNK: usize = 176 * 1024;
/// Мест под файлы в форме. Интерфейс берёт их по кругу, чтобы ответ про убранный
/// файл не приняли за ответ про новый на том же месте.
pub const SLOTS: usize = 64;
/// Имя файла — до стольких байт.
const MAX_NAME: usize = 160;
/// Куда интерфейс складывает выбранные файлы.
const NEW: &str = "new";
/// Начало имени папки отчёта, который уходит прямо сейчас.
const SENDING: &str = "send-";

/// Файл в отчёте. Байты лежат в папке отчёта под номером по порядку: `0`, `1`…
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attached {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Jpeg,
    Png,
    /// GIF, WebP, AVIF — метаданных с местом у них почти не бывает.
    Image,
    /// MP4, MOV, 3GP.
    Mp4,
    /// WebM, MKV.
    Matroska,
    Text,
}

/// Почему файл не приложен.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Type,
    /// HEIC: браузеры его не показывают.
    Heic,
    Empty,
    TooBig,
    /// Не записался или куски пришли не по порядку.
    Io,
}

impl Refused {
    pub fn text(self) -> &'static str {
        match self {
            Self::Type => "Такой файл не приложить: подойдут фото, снимки экрана, видео и текст",
            Self::Heic => "HEIC не открыть в браузере — сохраните фото как JPG или PNG",
            Self::Empty => "Файл пустой",
            Self::TooBig => "Файлы вместе — не больше 50 МБ",
            Self::Io => "Не удалось приложить файл",
        }
    }
}

/// Что за файл, по первым байтам.
fn kind(head: &[u8]) -> Result<Kind, Refused> {
    let at = |i: usize, s: &[u8]| head.get(i..i + s.len()) == Some(s);
    if head.is_empty() {
        Err(Refused::Empty)
    } else if at(0, b"\xff\xd8\xff") {
        Ok(Kind::Jpeg)
    } else if at(0, b"\x89PNG\r\n\x1a\n") {
        Ok(Kind::Png)
    } else if at(0, b"GIF87a") || at(0, b"GIF89a") || (at(0, b"RIFF") && at(8, b"WEBP")) {
        Ok(Kind::Image)
    } else if at(4, b"ftyp") {
        match head.get(8..12) {
            Some(b"avif" | b"avis") => Ok(Kind::Image),
            Some(b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1") => {
                Err(Refused::Heic)
            }
            _ => Ok(Kind::Mp4),
        }
    } else if at(0, b"\x1a\x45\xdf\xa3") {
        Ok(Kind::Matroska)
    } else if is_text(head) {
        Ok(Kind::Text)
    } else {
        Err(Refused::Type)
    }
}

/// Текст: с BOM или UTF-8 без управляющих байтов, кроме переводов строки, табуляции
/// и ESC (цвета в выводе терминала). Начало могло оборваться посреди буквы.
fn is_text(head: &[u8]) -> bool {
    if [&b"\xef\xbb\xbf"[..], b"\xff\xfe", b"\xfe\xff"]
        .iter()
        .any(|bom| head.starts_with(bom))
    {
        return true;
    }
    head.iter()
        .all(|&b| b >= 0x20 || matches!(b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b))
        && match std::str::from_utf8(head) {
            Ok(_) => true,
            Err(e) => e.error_len().is_none(),
        }
}

/// Текст файла в UTF-8: Блокнот и PowerShell пишут и UTF-16.
fn text(data: &[u8]) -> String {
    let utf16 = |bytes: &[u8], little: bool| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| {
                if little {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    let text = if let Some(rest) = data.strip_prefix(b"\xff\xfe") {
        utf16(rest, true)
    } else if let Some(rest) = data.strip_prefix(b"\xfe\xff") {
        utf16(rest, false)
    } else {
        String::from_utf8_lossy(data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)).into_owned()
    };
    text.replace('\0', "")
}

/// Base64 (стандартный алфавит, `=` в конце можно опустить).
fn base64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=').as_bytes();
    if s.len() % 4 == 1 {
        return None;
    }
    let value = |c: u8| -> Option<u32> {
        Some(u32::from(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 2);
    for quad in s.chunks(4) {
        let mut n = 0;
        for (k, &c) in quad.iter().enumerate() {
            n |= value(c)? << (18 - 6 * k);
        }
        let [_, a, b, c] = n.to_be_bytes();
        out.extend_from_slice(&[a, b, c][..quad.len() - 1]);
    }
    Some(out)
}

fn new_dir(root: &Path) -> PathBuf {
    root.join(super::DIR).join(NEW)
}

/// Выбранные файлы на диске: место и сколько байт уже пришло.
fn staged(dir: &Path) -> Vec<(usize, u64)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let slot = e.file_name().to_str()?.parse().ok()?;
            Some((slot, e.metadata().ok()?.len()))
        })
        .collect()
}

/// Кусок выбранного файла: `offset` — сколько байт этого файла уже пришло (0 — начало,
/// тогда же проверяется тип). Возвращает, сколько байт файла теперь на диске.
/// Не принят — файл на этом месте удаляется.
pub fn write_chunk(root: &Path, slot: usize, offset: u64, data: &str) -> Result<u64, Refused> {
    let dir = new_dir(root);
    let path = dir.join(slot.to_string());
    let write = || -> Result<u64, Refused> {
        let bytes = base64(data).ok_or(Refused::Io)?;
        if offset == 0 {
            kind(&bytes)?;
            fs::create_dir_all(&dir).map_err(|_| Refused::Io)?;
        } else if fs::metadata(&path).map(|m| m.len()).ok() != Some(offset) {
            return Err(Refused::Io);
        }
        let size = offset + bytes.len() as u64;
        let others: u64 = staged(&dir)
            .into_iter()
            .filter(|&(s, _)| s != slot)
            .map(|(_, n)| n)
            .sum();
        if others + size > MAX_BYTES {
            return Err(Refused::TooBig);
        }
        let file = if offset == 0 {
            fs::File::create(&path)
        } else {
            fs::OpenOptions::new().append(true).open(&path)
        };
        file.and_then(|mut f| f.write_all(&bytes))
            .map_err(|_| Refused::Io)?;
        Ok(size)
    };
    let result = write();
    if result.is_err() {
        let _ = fs::remove_file(&path);
    }
    result
}

/// Файл убрали из формы.
pub fn drop_slot(root: &Path, slot: usize) {
    let _ = fs::remove_file(new_dir(root).join(slot.to_string()));
}

/// Файлы, которые уходят с отчётом: своя папка и что в ней.
pub struct Taken {
    pub dir: PathBuf,
    pub files: Vec<Attached>,
}

/// «Отправить»: файлы с этих мест переезжают в свою папку `reports/send-…` (новые
/// выбранные файлы её уже не тронут), остальные выбранные удаляются.
pub fn take(root: &Path, picks: &[(usize, String)], stamp: &str) -> Result<Option<Taken>, Refused> {
    let new = new_dir(root);
    if picks.is_empty() {
        let _ = fs::remove_dir_all(&new);
        return Ok(None);
    }
    let dir = root.join(super::DIR).join(format!("{SENDING}{stamp}"));
    let collect = || -> Result<Vec<Attached>, Refused> {
        fs::create_dir_all(&dir).map_err(|_| Refused::Io)?;
        let mut files = Vec::with_capacity(picks.len());
        for (i, (slot, name)) in picks.iter().enumerate() {
            let to = dir.join(i.to_string());
            fs::rename(new.join(slot.to_string()), &to).map_err(|_| Refused::Io)?;
            let mut head = Vec::with_capacity(4096);
            fs::File::open(&to)
                .and_then(|f| f.take(4096).read_to_end(&mut head))
                .map_err(|_| Refused::Io)?;
            kind(&head)?;
            let size = fs::metadata(&to).map_err(|_| Refused::Io)?.len();
            files.push(Attached {
                name: file_name(name),
                size,
            });
        }
        if files.iter().map(|f| f.size).sum::<u64>() > MAX_BYTES {
            return Err(Refused::TooBig);
        }
        Ok(files)
    };
    let result = collect();
    let _ = fs::remove_dir_all(&new);
    match result {
        Ok(files) => Ok(Some(Taken { dir, files })),
        Err(err) => {
            let _ = fs::remove_dir_all(&dir);
            Err(err)
        }
    }
}

/// Имя без пути и служебных знаков.
fn file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base.chars().filter(|c| !c.is_control()).collect();
    match clean.trim() {
        "" => "файл".into(),
        name => super::cut_end(name, MAX_NAME),
    }
}

/// Вырезать личное: метаданные фото и видео, адреса и пути в тексте и имени.
/// Файлы перезаписываются на месте, по-прежнему под номерами по порядку; какой не
/// разобрать (битый JPEG), тот не уходит вовсе.
pub fn clean(dir: &Path, files: &[Attached], private: &scrub::Private) -> Vec<Attached> {
    let mut out = Vec::with_capacity(files.len());
    for (i, file) in files.iter().enumerate() {
        let from = dir.join(i.to_string());
        let cleaned = fs::read(&from)
            .ok()
            .and_then(|data| match kind(&data).ok()? {
                Kind::Jpeg => meta::jpeg(&data),
                Kind::Png => meta::png(&data),
                Kind::Mp4 => {
                    let mut data = data;
                    meta::bmff(&mut data);
                    Some(data)
                }
                Kind::Text => Some(private.scrub(&text(&data)).into_bytes()),
                Kind::Image | Kind::Matroska => Some(data),
            });
        let _ = fs::remove_file(&from);
        let Some(bytes) = cleaned.filter(|b| !b.is_empty()) else {
            continue;
        };
        // Номер не больше прежнего: файл с этим номером уже прочитан.
        if fs::write(dir.join(out.len().to_string()), &bytes).is_ok() {
            out.push(Attached {
                name: file_name(&private.scrub(&file.name)),
                size: bytes.len() as u64,
            });
        }
    }
    out
}

/// Файлы отчёта на диске как есть. Какого нет, на том список обрывается.
pub fn on_disk(dir: &Path, files: &[Attached]) -> Vec<Attached> {
    files
        .iter()
        .enumerate()
        .map_while(|(i, f)| {
            let size = fs::metadata(dir.join(i.to_string())).ok()?.len();
            Some(Attached {
                name: f.name.clone(),
                size,
            })
        })
        .collect()
}

/// Тело `POST /rf`: строка JSON отчёта со списком файлов, перевод строки и сами
/// файлы подряд — их размеры в списке.
pub fn write_body(out: &mut impl Write, header: &[u8], dir: &Path, count: usize) -> io::Result<()> {
    out.write_all(header)?;
    out.write_all(b"\n")?;
    for i in 0..count {
        io::copy(&mut fs::File::open(dir.join(i.to_string()))?, out)?;
    }
    Ok(())
}

/// Папка с файлами отчёта `reports/<имя>.json` — `reports/<имя>`.
pub fn dir_of(report: &Path) -> PathBuf {
    report.with_extension("")
}

/// При запуске: выбранные в прошлый раз и недоотправленные файлы больше не нужны,
/// как и папки без своего отчёта.
pub fn sweep(reports: &Path) {
    let Ok(entries) = fs::read_dir(reports) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == NEW || name.starts_with(SENDING) || !path.with_extension("json").exists() {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const B64: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn encode(bytes: &[u8]) -> String {
        let a = B64.as_bytes();
        let mut s = String::new();
        for c in bytes.chunks(3) {
            let n = c
                .iter()
                .enumerate()
                .fold(0u32, |n, (k, &b)| n | u32::from(b) << (16 - 8 * k));
            for k in 0..=c.len() {
                s.push(char::from(a[(n >> (18 - 6 * k) & 63) as usize]));
            }
        }
        while !s.len().is_multiple_of(4) {
            s.push('=');
        }
        s
    }

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("axiom-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn base64_round_trip() {
        for len in 0..20 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64(&encode(&bytes)), Some(bytes.clone()));
            assert_eq!(base64(encode(&bytes).trim_end_matches('=')), Some(bytes));
        }
        assert_eq!(base64("QUJD"), Some(b"ABC".to_vec()));
        assert_eq!(base64("QU*D"), None);
        assert_eq!(base64("QUJDR"), None);
    }

    #[test]
    fn kinds_come_from_bytes() {
        assert_eq!(kind(b"\xff\xd8\xff\xe0"), Ok(Kind::Jpeg));
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n\0"), Ok(Kind::Png));
        assert_eq!(kind(b"GIF89a"), Ok(Kind::Image));
        assert_eq!(kind(b"RIFF\0\0\0\0WEBPVP8 "), Ok(Kind::Image));
        assert_eq!(kind(b"\0\0\0\x20ftypisom"), Ok(Kind::Mp4));
        assert_eq!(kind(b"\0\0\0\x14ftypqt  "), Ok(Kind::Mp4));
        assert_eq!(kind(b"\0\0\0\x1cftypavif"), Ok(Kind::Image));
        assert_eq!(kind(b"\0\0\0\x18ftypheic"), Err(Refused::Heic));
        assert_eq!(kind(b"\x1a\x45\xdf\xa3\x01"), Ok(Kind::Matroska));
        assert_eq!(kind("журнал: ок\n".as_bytes()), Ok(Kind::Text));
        // Кусок оборвался посреди буквы.
        assert_eq!(kind(&"ёж".as_bytes()[..3]), Ok(Kind::Text));
        assert_eq!(kind(b"\xff\xfej\0o\0"), Ok(Kind::Text));
        assert_eq!(kind(b"MZ\x90\0\x03\0"), Err(Refused::Type));
        assert_eq!(kind(b"PK\x03\x04"), Err(Refused::Type));
        assert_eq!(kind(b"\x1b[31merror\x1b[0m\r\n"), Ok(Kind::Text));
        assert_eq!(kind(b""), Err(Refused::Empty));
    }

    #[test]
    fn utf16_text_becomes_utf8() {
        let le: Vec<u8> = b"\xff\xfe"
            .iter()
            .copied()
            .chain("Ошибка".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(text(&le), "Ошибка");
        let be: Vec<u8> = b"\xfe\xff"
            .iter()
            .copied()
            .chain("ok".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(text(&be), "ok");
        assert_eq!(text(b"\xef\xbb\xbfhi\0"), "hi");
    }

    #[test]
    fn chunks_build_a_file_and_take_moves_it() {
        let root = temp("take");
        let png = b"\x89PNG\r\n\x1a\nIHDR".repeat(3);
        let (a, b) = png.split_at(10);
        assert_eq!(write_chunk(&root, 7, 0, &encode(a)), Ok(10));
        // Кусок не с того места — файл сброшен.
        assert_eq!(write_chunk(&root, 7, 3, &encode(b)), Err(Refused::Io));
        assert_eq!(write_chunk(&root, 7, 10, &encode(b)), Err(Refused::Io));
        assert_eq!(write_chunk(&root, 7, 0, &encode(a)), Ok(10));
        assert_eq!(write_chunk(&root, 7, 10, &encode(b)), Ok(png.len() as u64));
        assert_eq!(
            write_chunk(&root, 3, 0, &encode(b"MZ\x90\0")),
            Err(Refused::Type)
        );
        assert_eq!(
            write_chunk(&root, 4, 0, &encode("текст".as_bytes())),
            Ok(10)
        );
        assert_eq!(write_chunk(&root, 5, 0, &encode(b"left over")), Ok(9));

        let picks = vec![
            (4, "C:\\Users\\x\\заметка.txt".to_string()),
            (7, "shot.png".to_string()),
        ];
        let taken = take(&root, &picks, "1-0").unwrap().unwrap();
        assert_eq!(
            taken.files,
            vec![
                Attached {
                    name: "заметка.txt".into(),
                    size: 10
                },
                Attached {
                    name: "shot.png".into(),
                    size: png.len() as u64
                },
            ]
        );
        assert_eq!(fs::read(taken.dir.join("1")).unwrap(), png);
        // Не выбранный в отчёт файл удалён вместе с папкой.
        assert!(!new_dir(&root).exists());
        // Места нет — ничего не уходит.
        assert_eq!(
            take(&root, &[(9, "x".to_string())], "2-0").err(),
            Some(Refused::Io)
        );
        assert!(!root.join(super::super::DIR).join("send-2-0").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn staged_files_have_a_total_limit() {
        let root = temp("limit");
        let dir = new_dir(&root);
        fs::create_dir_all(&dir).unwrap();
        // Уже выбран файл почти на весь предел.
        let big = fs::File::create(dir.join("0")).unwrap();
        big.set_len(MAX_BYTES - 4).unwrap();
        assert_eq!(write_chunk(&root, 1, 0, &encode(b"abcd")), Ok(4));
        assert_eq!(
            write_chunk(&root, 2, 0, &encode(b"e")),
            Err(Refused::TooBig)
        );
        assert!(!dir.join("2").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_keeps_order_and_drops_what_it_cannot_read() {
        let root = temp("clean");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("0"), b"\xff\xd8\xff\xe1broken").unwrap();
        fs::write(
            root.join("1"),
            "открыл https://secret.example/a?b=1 в C:\\Users\\Roman\\x".as_bytes(),
        )
        .unwrap();
        fs::write(root.join("2"), b"GIF89a....").unwrap();
        let files = [
            Attached {
                name: "photo.jpg".into(),
                size: 10,
            },
            Attached {
                name: "Roman.txt".into(),
                size: 10,
            },
            Attached {
                name: "anim.gif".into(),
                size: 10,
            },
        ];
        let private =
            scrub::Private::new(vec![r"C:\Users\Roman".into()], vec!["Roman".into()], vec![]);
        let out = clean(&root, &files, &private);
        assert_eq!(
            out.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["<user>.txt", "anim.gif"]
        );
        let text = fs::read_to_string(root.join("0")).unwrap();
        assert_eq!(text, "открыл secret.example в ~\\x");
        assert_eq!(out[0].size, text.len() as u64);
        assert_eq!(fs::read(root.join("1")).unwrap(), b"GIF89a....");
        assert!(!root.join("2").exists());
        assert_eq!(on_disk(&root, &out), out);

        let mut body = Vec::new();
        write_body(&mut body, b"{}", &root, out.len()).unwrap();
        assert_eq!(
            body,
            [b"{}\n".as_slice(), text.as_bytes(), b"GIF89a...."].concat()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sweep_removes_stale_folders_only() {
        let root = temp("sweep");
        for dir in ["new", "send-1-0", "5-0-user", "6-0-user"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("6-0-user.json"), b"{}").unwrap();
        sweep(&root);
        let mut left: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["6-0-user", "6-0-user.json"]);
        assert_eq!(dir_of(&root.join("6-0-user.json")), root.join("6-0-user"));
        let _ = fs::remove_dir_all(&root);
    }
}
