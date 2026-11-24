//! Язык страницы без отправки текста наружу: атрибут `lang` и сам текст.
//!
//! Атрибуту верим, пока он не спорит с письменностью: русский сайт с шаблонным
//! `lang="en"` не должен получить предложение перевода, а английский с `lang="ru"` —
//! должен. Для латиницы различаем частые языки по служебным словам.

/// Язык неизвестен, но точно не тот, что в атрибуте (предлагаем перевод без названия).
pub const UNKNOWN: &str = "und";

/// Сколько букв нужно, чтобы судить о письменности.
const MIN_LETTERS: usize = 30;
/// Сколько слов нужно, чтобы судить о языке латиницей.
const MIN_WORDS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Cyrillic,
    Greek,
    Arabic,
    Hebrew,
    Devanagari,
    Thai,
    Georgian,
    Armenian,
    Hangul,
    Kana,
    Han,
}

/// `en-US` → `en`, `zh_Hans` → `zh`, устаревшие коды — к нынешним. Мусор — `None`.
pub fn normalize(code: &str) -> Option<String> {
    let primary = code.trim().split(['-', '_']).next()?.to_ascii_lowercase();
    if !(2..=3).contains(&primary.len()) || !primary.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let code = match primary.as_str() {
        "iw" => "he",
        "in" => "id",
        "ji" => "yi",
        "jw" => "jv",
        "mo" => "ro",
        "nb" | "nn" => "no",
        "und" => return None,
        other => other,
    };
    Some(code.to_string())
}

/// Язык по атрибуту `lang` и образцу текста. `None` — судить не по чему (пустая
/// страница без атрибута), [`UNKNOWN`] — язык не ясен, но перевод предложить стоит.
pub fn detect(attr: &str, text: &str) -> Option<String> {
    let attr = normalize(attr);
    let guess = guess(text);
    match (attr, guess) {
        (attr, None) => attr,
        (None, Some((lang, _, _))) => Some(lang),
        (Some(attr), Some((lang, script, confident))) => {
            if !writes_in(&attr, script) {
                // Письменность спорит с атрибутом: верим тексту.
                Some(lang)
            } else if script == Script::Latin
                && confident
                && lang != attr
                && STOPWORDS.iter().any(|(l, _)| *l == attr)
            {
                // Шаблонный `lang="en"` на испанской странице и подобное.
                Some(lang)
            } else {
                Some(attr)
            }
        }
    }
}

/// Тот же ли это язык (с учётом `zh-CN` и `zh`).
pub fn same(a: &str, b: &str) -> bool {
    match (normalize(a), normalize(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Может ли язык быть записан этой письменностью. Для незнакомых языков —
/// латиница: на ней пишет большинство остальных.
fn writes_in(lang: &str, script: Script) -> bool {
    match lang {
        "ru" | "uk" | "be" | "bg" | "mk" | "kk" | "ky" | "tg" | "mn" | "tt" | "ba" | "cv" => {
            script == Script::Cyrillic
        }
        "sr" => matches!(script, Script::Cyrillic | Script::Latin),
        "el" => script == Script::Greek,
        "ar" | "fa" | "ur" | "ps" | "ug" => script == Script::Arabic,
        "he" | "yi" => script == Script::Hebrew,
        "hi" | "mr" | "ne" | "sa" => script == Script::Devanagari,
        "th" => script == Script::Thai,
        "ka" => script == Script::Georgian,
        "hy" => script == Script::Armenian,
        "ko" => script == Script::Hangul,
        "ja" => matches!(script, Script::Kana | Script::Han),
        "zh" | "yue" => script == Script::Han,
        _ => script == Script::Latin,
    }
}

fn script_of(c: char) -> Option<Script> {
    Some(match c as u32 {
        0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F => {
            if c == '×' || c == '÷' {
                return None;
            }
            Script::Latin
        }
        0x370..=0x3FF => Script::Greek,
        0x400..=0x52F => Script::Cyrillic,
        0x530..=0x58F => Script::Armenian,
        0x590..=0x5FF => Script::Hebrew,
        0x600..=0x6FF | 0x750..=0x77F => Script::Arabic,
        0x900..=0x97F => Script::Devanagari,
        0xE00..=0xE7F => Script::Thai,
        0x10A0..=0x10FF => Script::Georgian,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Script::Hangul,
        0x3040..=0x30FF => Script::Kana,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF => Script::Han,
        _ => return None,
    })
}

/// Язык по тексту: (код, письменность, уверенно ли различён язык латиницей).
fn guess(text: &str) -> Option<(String, Script, bool)> {
    const ALL: [Script; 12] = [
        Script::Latin,
        Script::Cyrillic,
        Script::Greek,
        Script::Arabic,
        Script::Hebrew,
        Script::Devanagari,
        Script::Thai,
        Script::Georgian,
        Script::Armenian,
        Script::Hangul,
        Script::Kana,
        Script::Han,
    ];
    let mut counts = [0usize; ALL.len()];
    let mut letters = 0;
    for c in text.chars() {
        if let Some(s) = script_of(c) {
            letters += 1;
            if let Some(i) = ALL.iter().position(|&x| x == s) {
                counts[i] += 1;
            }
        }
    }
    if letters < MIN_LETTERS {
        return None;
    }
    let count = |s: Script| ALL.iter().position(|&x| x == s).map_or(0, |i| counts[i]);
    // Японский — это иероглифы вперемешку с каной.
    let (han, kana) = (count(Script::Han), count(Script::Kana));
    if han + kana > letters / 2 {
        return Some(if kana * 10 >= han + kana {
            ("ja".into(), Script::Kana, false)
        } else {
            ("zh".into(), Script::Han, false)
        });
    }
    let (i, &top) = counts.iter().enumerate().max_by_key(|&(_, n)| *n)?;
    let script = ALL[i];
    if top * 2 < letters {
        return None;
    }
    let lang = match script {
        Script::Latin => return Some(latin(text)),
        Script::Cyrillic => cyrillic(text, top),
        Script::Greek => "el",
        Script::Arabic => "ar",
        Script::Hebrew => "he",
        Script::Devanagari => "hi",
        Script::Thai => "th",
        Script::Georgian => "ka",
        Script::Armenian => "hy",
        Script::Hangul => "ko",
        Script::Kana => "ja",
        Script::Han => "zh",
    };
    Some((lang.into(), script, false))
}

/// Кириллица: русский, если нет букв, которых в русском не бывает.
fn cyrillic(text: &str, letters: usize) -> &'static str {
    let has = |set: &[char]| text.chars().filter(|c| set.contains(c)).count();
    let share = |n: usize| n * 100 >= letters; // от 1% букв
    if share(has(&[
        'ј', 'љ', 'њ', 'ћ', 'ђ', 'џ', 'Ј', 'Љ', 'Њ', 'Ћ', 'Ђ', 'Џ',
    ])) {
        "sr"
    } else if share(has(&[
        'ә', 'ғ', 'қ', 'ң', 'ө', 'ұ', 'ү', 'һ', 'Ә', 'Ғ', 'Қ', 'Ң', 'Ө', 'Ұ',
    ])) {
        "kk"
    } else if has(&['ў', 'Ў']) > 0 && share(has(&['і', 'І'])) {
        "be"
    } else if share(has(&['і', 'ї', 'є', 'ґ', 'І', 'Ї', 'Є', 'Ґ'])) {
        "uk"
    } else if has(&['ы', 'э', 'Ы', 'Э']) == 0 && share(has(&['ъ'])) {
        "bg"
    } else {
        "ru"
    }
}

/// Служебные слова частых языков на латинице: по ним язык виден даже в коротком тексте.
const STOPWORDS: [(&str, &[&str]); 6] = [
    (
        "en",
        &[
            "the", "and", "of", "to", "is", "that", "with", "for", "this", "are", "was", "you",
            "have", "not", "be", "it", "on", "by", "from", "or",
        ],
    ),
    (
        "de",
        &[
            "der", "die", "und", "das", "ist", "nicht", "mit", "ein", "eine", "ich", "sie", "auf",
            "für", "den", "dem", "sich", "auch", "wird", "zu", "im",
        ],
    ),
    (
        "fr",
        &[
            "le", "les", "et", "des", "est", "une", "pour", "dans", "que", "pas", "qui", "sur",
            "au", "aux", "du", "ce", "sont", "avec", "nous", "vous",
        ],
    ),
    (
        "es",
        &[
            "el", "los", "las", "del", "que", "por", "con", "es", "se", "como", "pero", "más",
            "su", "al", "lo", "está", "sus", "y", "muy", "también",
        ],
    ),
    (
        "it",
        &[
            "il", "della", "che", "per", "sono", "non", "gli", "delle", "anche", "questo", "di",
            "è", "nel", "alla", "dei", "ed", "più", "si", "una", "uno",
        ],
    ),
    (
        "pt",
        &[
            "os", "das", "não", "uma", "para", "com", "são", "também", "pelo", "mais", "ao", "dos",
            "em", "você", "isso", "ser", "foi", "pela", "seu", "à",
        ],
    ),
];

/// Латиница: язык по служебным словам; не различили — [`UNKNOWN`].
fn latin(text: &str) -> (String, Script, bool) {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .collect();
    let unknown = (UNKNOWN.to_string(), Script::Latin, false);
    if words.len() < MIN_WORDS {
        return unknown;
    }
    let mut scores: Vec<(&str, usize)> = STOPWORDS
        .iter()
        .map(|(lang, set)| (*lang, words.iter().filter(|w| set.contains(w)).count()))
        .collect();
    scores.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let (lang, top) = scores[0];
    let second = scores[1].1;
    // Хотя бы 8% слов служебные и вдвое больше, чем у ближайшего соседа.
    if top * 100 >= words.len() * 8 && top >= second * 2 {
        (lang.to_string(), Script::Latin, true)
    } else {
        unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = "Rust is a general-purpose programming language that emphasizes performance, \
        type safety and concurrency. It enforces memory safety, meaning that all references point \
        to valid memory, without a garbage collector. This is one of the reasons it is loved.";
    const DE: &str = "Die Programmiersprache ist für die Entwicklung von sicheren Programmen gedacht \
        und wird von einer großen Gemeinschaft gepflegt. Sie ist nicht nur schnell, sondern auch \
        sicher, und das ist der Grund, warum sich viele Entwickler für sie entscheiden.";
    const ES: &str = "El lenguaje de programación es muy rápido y también seguro. Los programas se \
        ejecutan con poca memoria, pero el compilador es estricto con los errores y por eso \
        muchos desarrolladores lo usan como su herramienta principal en el trabajo.";
    const RU: &str = "Rust — мультипарадигменный компилируемый язык программирования общего \
        назначения, сочетающий парадигмы функционального и процедурного программирования.";
    const UK: &str = "Rust — мультипарадигмова компільована мова програмування загального \
        призначення, яка поєднує парадигми функціонального та процедурного програмування.";

    #[test]
    fn normalizes_codes() {
        assert_eq!(normalize("en-US").as_deref(), Some("en"));
        assert_eq!(normalize(" ZH_hans ").as_deref(), Some("zh"));
        assert_eq!(normalize("iw").as_deref(), Some("he"));
        assert_eq!(normalize("nb-NO").as_deref(), Some("no"));
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("x"), None);
        assert_eq!(normalize("en1"), None);
        assert_eq!(normalize("<script>"), None);
        assert!(same("zh-CN", "zh"));
        assert!(!same("en", "ru"));
    }

    #[test]
    fn attribute_wins_when_script_agrees() {
        assert_eq!(detect("en", EN).as_deref(), Some("en"));
        assert_eq!(detect("de-DE", DE).as_deref(), Some("de"));
        assert_eq!(detect("ru", RU).as_deref(), Some("ru"));
        // Нет текста — только атрибут.
        assert_eq!(detect("fr", "").as_deref(), Some("fr"));
        assert_eq!(detect("", ""), None);
    }

    #[test]
    fn text_beats_a_wrong_attribute() {
        // Русская страница с шаблонным lang="en": перевод не предлагаем.
        assert_eq!(detect("en", RU).as_deref(), Some("ru"));
        // Английская страница с lang="ru".
        assert_eq!(detect("ru", EN).as_deref(), Some("en"));
        // Испанская с lang="en".
        assert_eq!(detect("en", ES).as_deref(), Some("es"));
    }

    #[test]
    fn guesses_without_attribute() {
        assert_eq!(detect("", EN).as_deref(), Some("en"));
        assert_eq!(detect("", DE).as_deref(), Some("de"));
        assert_eq!(detect("", ES).as_deref(), Some("es"));
        assert_eq!(detect("", RU).as_deref(), Some("ru"));
        assert_eq!(detect("", UK).as_deref(), Some("uk"));
        let ja = "日本語の文章は漢字と仮名で書かれています。東京は日本の首都であり、多くの人が住んでいます。";
        assert_eq!(detect("", ja).as_deref(), Some("ja"));
        let zh = "中华人民共和国是位于东亚的社会主义国家，首都为北京。全国人口超过十四亿，是世界上人口最多的国家之一。";
        assert_eq!(detect("", zh).as_deref(), Some("zh"));
        // Латиница, но язык не различить: перевод предложим без названия языка.
        let nl = "Het is een mooie dag in de stad en wij gaan naar het park met de kinderen \
            omdat het weer goed is en de zon schijnt over de huizen van onze buren vandaag.";
        assert_eq!(detect("", nl).as_deref(), Some(UNKNOWN));
    }
}
