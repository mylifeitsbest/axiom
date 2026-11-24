//! Ссылка средней кнопкой или с Ctrl открывается во вкладке в фоне, как в Chrome.
//!
//! WebView2 сообщает о новом окне одинаково и для `target=_blank`, и для среднего
//! клика: как именно открыли ссылку, он не передаёт. Поэтому страница сама запоминает
//! последний такой клик по ссылке (только настоящий, `isTrusted`), а браузер спрашивает
//! её, когда приходит запрос на новое окно. Подделать ответ сайт может, но толку
//! в этом нет: хуже всего его вкладка откроется в фоне, а не впереди.

/// Скрипт в каждой странице вкладки: запоминает средний клик и Ctrl+клик по ссылке.
/// С Shift ссылка открывается впереди (Ctrl+Shift+клик в Chrome).
pub const TRACKER: &str = r"(() => {
  let last = null;
  const note = (e) => {
    if (!e.isTrusted) return;
    const bg = (e.button === 1 || (e.button === 0 && (e.ctrlKey || e.metaKey))) && !e.shiftKey;
    const link = bg && e.composedPath().find((n) => n instanceof HTMLAnchorElement || n instanceof HTMLAreaElement);
    last = link && link.href ? { href: link.href, at: Date.now() } : null;
  };
  addEventListener('auxclick', note, true);
  addEventListener('click', note, true);
  Object.defineProperty(window, '__axiomBackground', {
    value: (url) => {
      if (!last || Date.now() - last.at > 3000) return false;
      try { return new URL(url).href === last.href; } catch { return false; }
    },
  });
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
    fn probe_quotes_the_url() {
        let js = probe(r#"https://e.example/?q="x"</script>"#);
        assert!(js.contains(r#"window.__axiomBackground("https://e.example/?q=\"x\"</script>")"#));
        assert!(answer("true\n"));
        assert!(!answer("false"));
        assert!(!answer("null"));
        assert!(!answer(""));
    }
}
