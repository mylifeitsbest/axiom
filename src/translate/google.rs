//! Запрос к Google Переводчику и разбор ответа.
//!
//! Основной адрес — бесплатный `translate_a/t?client=gtx`: принимает сразу много
//! фрагментов (по `q` на абзац), не склеивает их и для каждого называет язык
//! оригинала. Если он не ответил (например, Google ограничил частоту запросов с
//! этого адреса), тот же текст уходит в `translateHtml` — им пользуется виджет
//! Google Website Translator, ключ публичный, из его element.js.
//!
//! Абзац со ссылками и выделением уходит одним фрагментом, где куски текста
//! размечены `<a i=N>`: так Google переводит фразу целиком, а мы раскладываем
//! перевод обратно по исходным текстовым узлам. HTML собирает только Rust из
//! экранированного текста, а странице возвращается простой текст.
//!
//! Сеть — через системный `curl`, как у проверки обновлений: тело запроса идёт
//! через stdin, а не в командной строке, ответ читается не больше [`MAX_RESPONSE`].

use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};

use serde_json::Value;

use super::Group;

const GTX_URL: &str = "https://translate.googleapis.com/translate_a/t";
const HTML_URL: &str = "https://translate-pa.googleapis.com/v1/translateHtml";
/// Публичный ключ виджета Google Website Translator (translate_a/element.js).
const HTML_KEY: &str = "AIzaSyATBXajvzQLTDHEQbcpq0Ihe0vWDHmO520";
/// Больше перевода, чем в несколько раз от исходного текста, не бывает.
const MAX_RESPONSE: u64 = 4 * 1024 * 1024;

/// `translate_a/t` уже отказал в этом сеансе — сразу идём во второй адрес.
static GTX_REFUSED: AtomicBool = AtomicBool::new(false);

/// Перевод порции: текст для каждого узла и язык оригинала (по большинству текста).
#[derive(Debug, Default)]
pub struct Output {
    pub items: Vec<(u64, String)>,
    pub lang: Option<String>,
}

/// Переводит порцию на язык `to` (код из [`super::TARGETS`]). Блокирует поток.
pub fn translate(to: &str, groups: &[Group]) -> Result<Output, String> {
    let html: Vec<String> = groups.iter().map(|g| marked(g)).collect();
    let tl = google_code(to);
    let gtx = || -> Result<_, String> {
        let url = format!("{GTX_URL}?client=gtx&sl=auto&tl={tl}&format=html");
        let body = form_body(&html);
        let raw = post(
            &url,
            &["Content-Type: application/x-www-form-urlencoded;charset=utf-8"],
            body.as_bytes(),
        )?;
        parse_gtx(&raw, html.len())
    };
    let widget = || -> Result<_, String> {
        let body = serde_json::json!([[html, "auto", tl], "te_lib"]).to_string();
        let key = format!("X-Goog-API-Key: {HTML_KEY}");
        let raw = post(
            HTML_URL,
            &["Content-Type: application/json+protobuf", &key],
            body.as_bytes(),
        )?;
        parse_widget(&raw, html.len())
    };
    let translated = if GTX_REFUSED.load(Ordering::Relaxed) {
        widget().or_else(|_| gtx())
    } else {
        gtx().or_else(|err| {
            let result = widget();
            if result.is_ok() {
                GTX_REFUSED.store(true, Ordering::Relaxed);
            }
            result.map_err(|_| err)
        })
    }?;
    // В китайском и японском слова пробелами не разделяют.
    Ok(assemble(groups, translated, !matches!(to, "zh" | "ja")))
}

/// Google называет китайский упрощённый `zh-CN`.
fn google_code(to: &str) -> &str {
    if to == "zh" { "zh-CN" } else { to }
}

/// Фрагмент для Google: один узел — просто текст, несколько — с разметкой `<a i=N>`.
/// Пробел между кусками ставим там, где он был на странице: иначе слова слиплись бы.
pub fn marked(group: &[(u64, String)]) -> String {
    if let [(_, text)] = group {
        return escape(text.trim());
    }
    let mut out = String::new();
    let mut gap = false;
    for (i, (_, text)) in group.iter().enumerate() {
        if i > 0 && (gap || text.starts_with(char::is_whitespace)) {
            out.push(' ');
        }
        gap = text.ends_with(char::is_whitespace);
        out.push_str(&format!("<a i={i}>{}</a>", escape(text.trim())));
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

fn form_body(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| {
            format!(
                "q={}",
                url::form_urlencoded::byte_serialize(p.as_bytes()).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Ответ `translate_a/t`: `[["перевод","en"], …]`. На один фрагмент Google иногда
/// отвечает без внешнего массива: `["перевод","en"]`.
pub fn parse_gtx(raw: &str, n: usize) -> Result<Vec<(String, Option<String>)>, String> {
    let value: Value = serde_json::from_str(raw.trim()).map_err(|_| bad())?;
    let list = value.as_array().ok_or_else(bad)?;
    if n == 1 && list.len() == 2 && list.iter().all(Value::is_string) {
        let text = list[0].as_str().unwrap_or_default().to_string();
        return Ok(vec![(text, list[1].as_str().map(String::from))]);
    }
    if list.len() != n {
        return Err(bad());
    }
    list.iter()
        .map(|item| match item {
            Value::String(text) => Ok((text.clone(), None)),
            Value::Array(pair) => {
                let text = pair.first().and_then(Value::as_str).ok_or_else(bad)?;
                let lang = pair.get(1).and_then(Value::as_str).map(String::from);
                Ok((text.to_string(), lang))
            }
            _ => Err(bad()),
        })
        .collect()
}

/// Ответ `translateHtml`: `[["перевод", …], ["en", …]]`.
pub fn parse_widget(raw: &str, n: usize) -> Result<Vec<(String, Option<String>)>, String> {
    let value: Value = serde_json::from_str(raw.trim()).map_err(|_| bad())?;
    let texts = value.get(0).and_then(Value::as_array).ok_or_else(bad)?;
    if texts.len() != n {
        return Err(bad());
    }
    let langs = value.get(1).and_then(Value::as_array);
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let text = t.as_str().ok_or_else(bad)?;
            let lang = langs
                .and_then(|l| l.get(i))
                .and_then(Value::as_str)
                .map(String::from);
            Ok((text.to_string(), lang))
        })
        .collect()
}

fn bad() -> String {
    "непонятный ответ Google Переводчика".into()
}

/// Раскладывает переводы фрагментов по узлам и выбирает язык оригинала.
fn assemble(groups: &[Group], translated: Vec<(String, Option<String>)>, spaced: bool) -> Output {
    let mut items = Vec::new();
    let mut weights: Vec<(String, usize)> = Vec::new();
    for (group, (html, lang)) in groups.iter().zip(translated) {
        let pieces = split_marked(&html, group.len());
        // Пустой перевод всего фрагмента — сбой, а не перевод: оставляем оригинал.
        if pieces.iter().all(String::is_empty) {
            continue;
        }
        let pieces = fit(group, pieces, spaced);
        if let Some(lang) = lang.as_deref().and_then(super::lang::normalize) {
            let size: usize = group.iter().map(|(_, t)| t.chars().count()).sum();
            match weights.iter_mut().find(|(l, _)| *l == lang) {
                Some((_, w)) => *w += size,
                None => weights.push((lang, size)),
            }
        }
        // Узлы из одних пробелов не трогаем.
        items.extend(
            group
                .iter()
                .zip(pieces)
                .filter(|((_, orig), _)| !orig.trim().is_empty())
                .map(|((id, _), text)| (*id, text)),
        );
    }
    let lang = weights
        .into_iter()
        .max_by_key(|&(_, w)| w)
        .map(|(lang, _)| lang);
    Output { items, lang }
}

/// Знаки, перед которыми пробел не ставят.
const CLOSING: [char; 10] = [',', '.', ';', ':', '!', '?', ')', ']', '»', '…'];

/// Окончательный текст узлов фрагмента. Пробелы на стыках — как в оригинале
/// (пробел по краю узла страница заменит исходным), но без пробела перед
/// запятой, без задвоенного знака и без слипшихся слов, когда Google выбросил
/// знак между ними: «concurrency» + «, and» → «параллелизм» + « и».
pub fn fit(group: &[(u64, String)], pieces: Vec<String>, spaced: bool) -> Vec<String> {
    let n = group.len().min(pieces.len());
    let orig: Vec<&str> = group.iter().map(|(_, t)| t.as_str()).collect();
    let mut core = pieces;
    let filled =
        |core: &[String]| -> Vec<usize> { (0..n).filter(|&i| !core[i].is_empty()).collect() };
    // «производительности,» + «,» — Google повторил знак и внутри метки, и снаружи.
    for pair in filled(&core).windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if let Some(p) = core[b].chars().next()
            && matches!(p, ',' | '.' | ';' | ':')
            && core[a].len() > 1
            && core[a].ends_with(p)
        {
            core[a].pop();
            let keep = core[a].trim_end().len();
            core[a].truncate(keep);
        }
    }
    let ws_start = |s: &str| s.starts_with(char::is_whitespace);
    let ws_end = |s: &str| s.ends_with(char::is_whitespace);
    let alnum_start = |s: &str| s.chars().next().is_some_and(char::is_alphanumeric);
    let alnum_end = |s: &str| s.chars().next_back().is_some_and(char::is_alphanumeric);
    let filled = filled(&core);
    let mut out = vec![String::new(); n];
    for (j, &i) in filled.iter().enumerate() {
        let mut text = String::new();
        match j.checked_sub(1).map(|k| filled[k]) {
            None => {
                if orig[..=i].iter().any(|t| ws_start(t)) {
                    text.push(' ');
                }
            }
            Some(prev) => {
                // Между ними были узлы, которые Google влил в соседей: там был текст.
                let ws = ws_end(orig[prev]) || ws_start(orig[i]) || prev + 1 < i;
                let glued = spaced
                    && alnum_end(&core[prev])
                    && alnum_start(&core[i])
                    && !(alnum_end(orig[prev]) && alnum_start(orig[i]));
                if !core[i].starts_with(CLOSING) && (ws || glued) {
                    if ws_end(orig[prev]) {
                        out[prev].push(' ');
                    } else {
                        text.push(' ');
                    }
                }
            }
        }
        text.push_str(&core[i]);
        out[i] = text;
    }
    if let Some(&last) = filled.last()
        && orig[last..n].iter().any(|t| ws_end(t))
    {
        out[last].push(' ');
    }
    out
}

/// Добавляет кусок текста к переводу узла; пробел перед запятой и точкой убирает.
fn append(piece: &mut String, chunk: &str) {
    let trimmed = chunk.trim_start();
    if trimmed.starts_with(CLOSING) {
        let keep = piece.trim_end().len();
        piece.truncate(keep);
        piece.push_str(trimmed);
    } else {
        piece.push_str(chunk);
    }
}

/// Делит переведённый фрагмент по меткам `<a i=N>` на `n` кусков. Текст вне меток
/// достаётся предыдущему куску (Google выносит за метку пробелы и знаки препинания),
/// прочие теги выбрасываются, сущности раскодируются.
pub fn split_marked(html: &str, n: usize) -> Vec<String> {
    let mut out = vec![String::new(); n];
    if n == 0 {
        return out;
    }
    let mut stack: Vec<usize> = Vec::new();
    let mut last = 0;
    let mut rest = html;
    while !rest.is_empty() {
        let at = stack.last().copied().unwrap_or(last);
        let Some(open) = rest.find('<') else {
            append(&mut out[at], rest);
            break;
        };
        append(&mut out[at], &rest[..open]);
        let tag = &rest[open..];
        let Some(close) = tag.find('>') else {
            append(&mut out[at], tag);
            break;
        };
        let inner = &tag[1..close];
        if n > 1 {
            if let Some(i) = open_index(inner) {
                // Чужой номер — продолжение текущего куска.
                let i = if i < n {
                    i
                } else {
                    stack.last().copied().unwrap_or(last)
                };
                stack.push(i);
                last = i;
            } else if inner.trim().eq_ignore_ascii_case("/a") {
                stack.pop();
            }
        }
        rest = &tag[close + 1..];
    }
    out.iter()
        .map(|s| {
            let text = decode_entities(s);
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .collect()
}

/// `a i=3`, `A i="3"` → 3.
fn open_index(tag: &str) -> Option<usize> {
    let rest = tag.trim().strip_prefix(['a', 'A'])?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix("i=")
        .or_else(|| rest.strip_prefix("I="))?;
    let digits: String = rest
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(char::is_ascii_digit)
        .take(6)
        .collect();
    digits.parse().ok()
}

/// Сущности HTML, которые Google оставляет в переводе: `&amp;`, `&#39;`, `&#x2F;`.
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail.find(';').filter(|&end| end <= 10).and_then(|end| {
            let name = &tail[1..end];
            let c = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => '\u{a0}',
                _ => {
                    let num = name.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code).filter(|c| !c.is_control() || c.is_whitespace())?
                }
            };
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// POST через системный curl. Тело — через stdin, ответ — не больше [`MAX_RESPONSE`].
fn post(url: &str, headers: &[&str], body: &[u8]) -> Result<String, String> {
    let mut cmd = curl();
    cmd.args(["--fail", "-X", "POST", "--data-binary", "@-"]);
    for header in headers {
        cmd.arg("-H").arg(header);
    }
    cmd.arg(url);
    let mut child = cmd
        .spawn()
        .map_err(|_| "не найден curl для связи с Google Переводчиком".to_string())?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let (sent, out) = std::thread::scope(|scope| {
        // Пишем тело в своём потоке: иначе большой запрос и большой ответ
        // могли бы ждать друг друга в заполненных трубах.
        let writer = scope.spawn(move || stdin.map(|mut s| s.write_all(body)));
        let mut out = Vec::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(MAX_RESPONSE + 1).read_to_end(&mut out);
        }
        (writer.join(), out)
    });
    if out.len() as u64 > MAX_RESPONSE {
        let _ = child.kill();
        let _ = child.wait();
        return Err(bad());
    }
    let mut err = String::new();
    if let Some(stderr) = stderr.as_mut() {
        let _ = stderr.take(4096).read_to_string(&mut err);
    }
    let status = child
        .wait()
        .map_err(|_| "curl завершился с ошибкой".to_string())?;
    if !status.success() || !matches!(sent, Ok(Some(Ok(())))) {
        return Err(curl_error(status.code(), &err));
    }
    String::from_utf8(out).map_err(|_| bad())
}

fn curl_error(code: Option<i32>, stderr: &str) -> String {
    match code {
        Some(22) if stderr.contains("429") => "Google временно ограничил перевод, попробуйте позже",
        Some(22) => "Google Переводчик отказал",
        Some(6 | 7) => "нет связи с Google Переводчиком",
        Some(28) => "Google Переводчик не ответил вовремя",
        _ => "не удалось связаться с Google Переводчиком",
    }
    .to_string()
}

fn curl() -> Command {
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
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "10",
        "--max-time",
        "40",
    ])
    .arg("--user-agent")
    .arg(concat!("Axiom/", env!("CARGO_PKG_VERSION")))
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(texts: &[&str]) -> Group {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| (i as u64 + 10, (*t).to_string()))
            .collect()
    }

    #[test]
    fn marks_pieces_and_keeps_spaces() {
        let g = group(&["Rust is a ", "general-purpose", " language & <fast>"]);
        assert_eq!(
            marked(&g),
            "<a i=0>Rust is a</a> <a i=1>general-purpose</a> <a i=2>language &amp; &lt;fast&gt;</a>"
        );
        // Без пробела на стыке — без пробела и в запросе: «don» + «'t» не разорвутся.
        assert_eq!(
            marked(&group(&["don", "'t"])),
            "<a i=0>don</a><a i=1>'t</a>"
        );
        // Один узел — без разметки.
        assert_eq!(marked(&group(&[" Hello <b> "])), "Hello &lt;b&gt;");
        assert_eq!(
            form_body(&["a b&c".into(), "д".into()]),
            "q=a+b%26c&q=%D0%B4"
        );
    }

    #[test]
    fn parses_gtx_responses() {
        let raw = r#"[["Привет, мир","en"],["<a i=0>Доброе</a> <a i=1>утро</a>","de"]]"#;
        let parsed = parse_gtx(raw, 2);
        assert_eq!(
            parsed,
            Ok(vec![
                ("Привет, мир".into(), Some("en".into())),
                (
                    "<a i=0>Доброе</a> <a i=1>утро</a>".into(),
                    Some("de".into())
                ),
            ])
        );
        // Один фрагмент — бывает и без внешнего массива.
        assert_eq!(
            parse_gtx(r#"["Привет","en"]"#, 1),
            Ok(vec![("Привет".into(), Some("en".into()))])
        );
        assert_eq!(
            parse_gtx(r#"[["Привет","en"]]"#, 1),
            Ok(vec![("Привет".into(), Some("en".into()))])
        );
        // Язык оригинала указан явно — только строки.
        assert_eq!(
            parse_gtx(r#"["Раз","Два"]"#, 2),
            Ok(vec![("Раз".into(), None), ("Два".into(), None)])
        );
        // Число фрагментов не сошлось, мусор, страница капчи.
        assert!(parse_gtx(r#"[["Раз","en"]]"#, 2).is_err());
        assert!(parse_gtx(r#"{"a":1}"#, 1).is_err());
        assert!(parse_gtx("<html>Sorry...</html>", 1).is_err());
        assert!(parse_gtx(r#"[[1,"en"]]"#, 1).is_err());
    }

    #[test]
    fn parses_widget_responses() {
        let raw = r#"[["<a i=0>Привет,</a> <a i=1>мир</a>","Добрый день"],["en","de"]]"#;
        assert_eq!(
            parse_widget(raw, 2),
            Ok(vec![
                (
                    "<a i=0>Привет,</a> <a i=1>мир</a>".into(),
                    Some("en".into())
                ),
                ("Добрый день".into(), Some("de".into())),
            ])
        );
        assert!(parse_widget(r#"[["один"],["en"]]"#, 2).is_err());
        assert_eq!(
            parse_widget(r#"[["один"]]"#, 1),
            Ok(vec![("один".into(), None)])
        );
    }

    #[test]
    fn splits_by_markers() {
        let html = "<a i=0>Rust — это</a> <a i=1>универсальный</a><a i=2> </a><a i=3>язык,</a> \
                    <a i=4>в котором</a> <a i=5>упор</a> <a i=6>, &lt;быстро&gt; &amp; &#39;надёжно&#39;.</a>";
        assert_eq!(
            split_marked(html, 7),
            vec![
                "Rust — это",
                "универсальный",
                "",
                "язык,",
                "в котором",
                "упор",
                ", <быстро> & 'надёжно'."
            ]
        );
        // Текст до первой метки — первому куску, после закрытия — предыдущему,
        // чужие номера и теги не ломают разбор, пропавший кусок пуст.
        assert_eq!(
            split_marked(
                "Ну <a i=1>вот</a>, <b>и</b> <a i=9>всё</a> <A I=\"0\">тут</a>",
                3
            ),
            vec!["Ну тут", "вот, и всё", ""]
        );
        // Один кусок: метки не разбираем, теги выбрасываем.
        assert_eq!(split_marked("Привет, <b>мир</b>", 1), vec!["Привет, мир"]);
        // Незакрытый тег не теряет текст.
        assert_eq!(
            split_marked("<a i=0>раз</a> два < три", 2),
            vec!["раз два < три", ""]
        );
        assert!(split_marked("что-то", 0).is_empty());
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("&lt;b&gt; &amp;amp; &quot;x&quot; &#39;y&#x27; &#8212;"),
            "<b> &amp; \"x\" 'y' —"
        );
        // Неизвестное и битое — как есть.
        assert_eq!(
            decode_entities("AT&T &unknown; &#xZZ; &"),
            "AT&T &unknown; &#xZZ; &"
        );
        // Управляющие символы из сущностей не пропускаем.
        assert_eq!(decode_entities("a&#0;b&#7;c"), "a&#0;b&#7;c");
    }

    #[test]
    fn assembles_items_and_majority_language() {
        let groups = vec![
            group(&["Hello"]),
            group(&["Big ", "world", " here"]),
            group(&["x"]),
        ];
        let translated = vec![
            ("Привет".into(), Some("en".into())),
            (
                "<a i=0>Большой</a> <a i=1>мир</a> <a i=2>здесь</a>".into(),
                Some("en".into()),
            ),
            (String::new(), Some("zh-CN".into())),
        ];
        let out = assemble(&groups, translated, true);
        assert_eq!(out.lang.as_deref(), Some("en"));
        assert_eq!(
            out.items,
            vec![
                (10, "Привет".to_string()),
                (10, "Большой ".to_string()),
                (11, "мир".to_string()),
                (12, " здесь".to_string()),
            ]
        );
        // Узел из одних пробелов назад не уходит.
        let out = assemble(
            &[group(&["One", " ", "two"])],
            vec![("<a i=0>Раз</a> <a i=2>два</a>".into(), None)],
            true,
        );
        assert_eq!(
            out.items,
            vec![(10, "Раз".to_string()), (12, " два".to_string())]
        );
    }

    /// Абзац из Википедии: ссылки, запятые между ними — отдельные узлы.
    #[test]
    fn fits_spaces_and_punctuation() {
        let g = group(&[
            "Rust",
            " is a ",
            "general-purpose programming language",
            " that emphasizes ",
            "performance",
            ", ",
            "type safety",
            ", ",
            "concurrency",
            ", and ",
            "memory safety",
            ". Rust supports multiple ",
            "programming paradigms",
            ".",
        ]);
        let html = "<a i=0>Rust</a> <a i=1>— это</a> <a i=2>язык программирования общего назначения</a> \
            <a i=3>, в котором упор делается на</a> <a i=4>производительность</a> <a i=5>,</a> \
            <a i=6>безопасность типов</a> <a i=7>,</a> <a i=8>параллелизм</a> <a i=9>и</a> \
            <a i=10>безопасность памяти</a> <a i=11>. Rust поддерживает несколько</a> \
            <a i=12>парадигм программирования</a> <a i=13>.</a>";
        let out = fit(&g, split_marked(html, g.len()), true);
        assert_eq!(
            out.concat(),
            "Rust — это язык программирования общего назначения, в котором упор делается на \
             производительность, безопасность типов, параллелизм и безопасность памяти. \
             Rust поддерживает несколько парадигм программирования."
        );
        // Пробел у края узла — там же, где был: страница поставит туда исходный.
        assert_eq!(out[1], " — это ");
        assert_eq!(out[9], " и ");

        // Google вынес запятую за метку и повторил её внутри.
        let g = group(&["performance", ", ", "type safety"]);
        let out = fit(
            &g,
            split_marked(
                "<a i=0>производительности</a> ,<a i=1>,</a> <a i=2>типов</a>",
                3,
            ),
            true,
        );
        assert_eq!(out.concat(), "производительности, типов");

        // «Click here to continue»: без пробела перед запятой.
        let g = group(&["Click ", "here", " to continue"]);
        let out = fit(
            &g,
            split_marked(
                "<a i=0>Нажмите</a> <a i=1>здесь</a> <a i=2>, чтобы продолжить</a>",
                3,
            ),
            true,
        );
        assert_eq!(out, vec!["Нажмите ", "здесь", ", чтобы продолжить"]);

        // Слово, разрезанное разметкой (<b>W</b>ord), не разрывается.
        let g = group(&["W", "ord"]);
        assert_eq!(
            fit(&g, vec!["С".into(), "лово".into()], true).concat(),
            "Слово"
        );

        // Влитый в соседа узел пустеет; в китайском слова не разделяют.
        let g = group(&["Good", " ", "morning"]);
        assert_eq!(
            fit(
                &g,
                vec!["早上好".into(), String::new(), String::new()],
                false
            ),
            vec!["早上好 ", "", ""]
        );
        let g = group(&["concurrency", ", and ", "safety"]);
        assert_eq!(
            fit(&g, vec!["并发".into(), "和".into(), "安全".into()], false).concat(),
            "并发和 安全"
        );
    }

    #[test]
    fn chinese_target_code() {
        assert_eq!(google_code("zh"), "zh-CN");
        assert_eq!(google_code("ru"), "ru");
    }
}
