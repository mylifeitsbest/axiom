//! Усыпление вкладок: что спрашиваем у страницы перед выгрузкой
//! и какие фоновые вкладки лишние.
//!
//! Вкладку с недописанным текстом не усыпляем, как и Chrome при выгрузке вкладок:
//! иначе после пробуждения пропадёт начатый комментарий или письмо.

use std::time::Instant;

use serde::Deserialize;

use super::TabId;

/// Скрипт в каждой странице вкладки: запоминает поле, в которое человек печатал.
/// Флаг выставляют только настоящие нажатия (`isTrusted`), не скрипты сайта.
pub const DRAFT_TRACKER: &str = r"(() => {
  let field = null;
  const skip = /^(checkbox|radio|range|color|file|submit|button|reset|image|hidden|select-one|select-multiple)$/;
  addEventListener('input', (e) => {
    const t = e.composedPath()[0];
    if (e.isTrusted && t && !skip.test(t.type || '')) field = t;
  }, true);
  Object.defineProperty(window, '__axiomDraft', {
    value: () => {
      const f = field;
      if (!f || !f.isConnected) return false;
      const text = typeof f.value === 'string' ? f.value : f.textContent;
      return (text || '').trim() !== '';
    },
  });
})();";

/// Вопрос странице перед усыплением: место прокрутки и остался ли недописанный текст.
pub const PROBE: &str = "(() => ({ y: window.scrollY, \
    keep: typeof window.__axiomDraft === 'function' && window.__axiomDraft() }))()";

/// Ответ страницы на [`PROBE`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct PageState {
    pub y: f64,
    /// В поле остался текст, который человек начал печатать.
    pub keep: bool,
}

impl PageState {
    /// Ответ скрипта приходит в JSON. Невнятный ответ — усыпляем как обычно.
    pub fn parse(raw: &str) -> Self {
        serde_json::from_str(raw.trim()).unwrap_or_default()
    }
}

/// Вкладки сверх лимита: оставляем `keep` недавно открытых, остальные — на усыпление.
pub fn excess(mut live: Vec<(Instant, TabId)>, keep: usize) -> Vec<TabId> {
    live.sort_by_key(|&(seen, _)| std::cmp::Reverse(seen));
    live.into_iter().skip(keep).map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn excess_drops_the_oldest() {
        let base = Instant::now();
        let at = |s| base + Duration::from_secs(s);
        let live = vec![(at(5), 1), (at(1), 2), (at(9), 3), (at(3), 4)];
        // Свежее всех 3 и 1 — остаются; 4 и 2 — на усыпление, давние в конце.
        assert_eq!(excess(live.clone(), 2), vec![4, 2]);
        assert_eq!(excess(live.clone(), 4), Vec::<TabId>::new());
        assert_eq!(excess(live, 0).len(), 4);
    }

    #[test]
    fn page_state_parsing() {
        assert_eq!(
            PageState::parse(r#"{"y":120.5,"keep":true}"#),
            PageState {
                y: 120.5,
                keep: true
            }
        );
        assert_eq!(PageState::parse(r#"{"y":0}"#), PageState::default());
        assert_eq!(PageState::parse("null"), PageState::default());
        assert_eq!(PageState::parse("oops"), PageState::default());
    }
}
