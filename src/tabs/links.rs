//! Ссылка средней кнопкой или с Ctrl открывается во вкладке в фоне, как в Chrome.
//!
//! WebView2 сообщает о новом окне одинаково и для `target=_blank`, и для среднего
//! клика: как именно открыли ссылку, он не передаёт. Поэтому страница сама запоминает
//! последний такой клик по ссылке (только настоящий, `isTrusted`), а браузер спрашивает
//! её, когда приходит запрос на новое окно. Подделать ответ сайт может, но толку
//! в этом нет: хуже всего его вкладка откроется в фоне, а не впереди.

/// Скрипт в каждой странице вкладки: запоминает настоящие (`isTrusted`) клики и нажатия.
///
/// - Средний клик и Ctrl+клик по ссылке — для [`probe`]. С Shift ссылка открывается
///   впереди (Ctrl+Shift+клик в Chrome).
/// - Время последнего клика или клавиши — для ссылок на приложения
///   (`external::INPUT_PROBE`).
///
/// Слушатели встают раньше скриптов страницы, снять их она не может, а свойства
/// с ответами нельзя ни переписать, ни удалить. `performance.now` берём заранее:
/// подменённые часы страницы ничего не изменят.
pub const TRACKER: &str = r"(() => {
  const now = performance.now.bind(performance);
  let input = -1e12;
  let link = null;
  const mark = (e) => { if (e.isTrusted) input = now(); };
  // И отпускание: ссылка срабатывает по нему, с него и считаем короткое окно «по клику».
  for (const type of ['pointerdown', 'mousedown', 'keydown', 'touchend', 'pointerup', 'click', 'keyup']) addEventListener(type, mark, true);
  const note = (e) => {
    if (!e.isTrusted) return;
    const bg = (e.button === 1 || (e.button === 0 && (e.ctrlKey || e.metaKey))) && !e.shiftKey;
    const a = bg && e.composedPath().find((n) => n instanceof HTMLAnchorElement || n instanceof HTMLAreaElement);
    link = a && a.href ? { href: a.href, at: now() } : null;
  };
  addEventListener('auxclick', note, true);
  addEventListener('click', note, true);
  const answer = (name, value) => { try { Object.defineProperty(window, name, { value }); } catch {} };
  answer('__axiomBackground', (url) => {
    if (!link || now() - link.at > 3000) return false;
    try { return new URL(url).href === link.href; } catch { return false; }
  });
  answer('__axiomInputAgo', () => now() - input);
})();";

/// Вопрос странице: открыть ли `url` в фоне. Ответ разбирает [`answer`].
pub fn probe(url: &str) -> String {
    let url = serde_json::to_string(url).unwrap_or_else(|_| "\"\"".into());
    format!(
        "typeof window.__axiomBackground === 'function' && window.__axiomBackground({url}) === true"
    )
}

/// Ответ страницы: `true` — в фоне. Невнятный ответ — как обычно, впереди.
pub fn answer(raw: &str) -> bool {
    raw.trim() == "true"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_answers_both_probes() {
        // Оба вопроса спрашивают то, что ставит общий скрипт.
        assert!(TRACKER.contains("'__axiomBackground'"));
        assert!(TRACKER.contains("'__axiomInputAgo'"));
        assert!(probe("https://e.example/").contains("window.__axiomBackground("));
        assert!(crate::tabs::external::INPUT_PROBE.contains("window.__axiomInputAgo()"));
    }

    #[test]
    fn probe_quotes_the_url() {
        let js = probe(r#"https://e.example/?q="x"</script>"#);
        assert!(js.contains(r#"window.__axiomBackground("https://e.example/?q=\"x\"</script>")"#));
        assert!(answer("true\n"));
        assert!(!answer("false"));
        assert!(!answer("null"));
        assert!(!answer(""));
    }
}
