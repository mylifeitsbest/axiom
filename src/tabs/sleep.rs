//! Усыпление вкладок: что спрашиваем у страницы перед выгрузкой
//! и какие фоновые вкладки лишние.
//!
//! Вкладку с недописанным текстом не усыпляем, как и Chrome при выгрузке вкладок:
//! иначе после пробуждения пропадёт начатый комментарий или письмо.
//!
//! Вкладку, которую человек попросил не усыплять, не усыпляют ни таймер, ни нехватка
//! памяти, ни лимит живых вкладок. Место в лимите она всё равно занимает.
//!
//! Живые фоновые вкладки не замораживаем (`TrySuspend`) и не переводим в экономный
//! режим памяти: WebView2 тогда выталкивает почти всю память страницы в сжатие и
//! подкачку, и при возврате на вкладку она подолгу дёргается, пока память
//! подгружается обратно. Спрятанную страницу Chromium и так притормаживает сам.

use std::time::{Duration, Instant};

use serde::Deserialize;

use super::TabId;

/// Когда системе не хватает памяти, фоновые вкладки засыпают уже через минуту.
pub const LOW_MEMORY_IDLE: Duration = Duration::from_secs(60);

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

/// Сколько из `candidates` усыпляемых вкладок остаётся живыми при лимите `limit`.
/// `live` — все живые фоновые вкладки: те, что усыплять нельзя, занимают место первыми.
pub fn room(limit: usize, live: usize, candidates: usize) -> usize {
    limit.saturating_sub(live.saturating_sub(candidates))
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
    fn room_counts_tabs_that_cannot_sleep() {
        // Все живые можно усыпить: в лимите остаётся сколько разрешено.
        assert_eq!(room(3, 5, 5), 3);
        // Две живые усыплять нельзя: они занимают два места из трёх.
        assert_eq!(room(3, 5, 3), 1);
        // Неусыпляемых больше лимита: живыми не остаётся ни одна из остальных.
        assert_eq!(room(3, 5, 1), 0);
        assert_eq!(room(3, 0, 0), 3);
    }

    /// Фоновые живые вкладки 1..=n: первая открыта давнее всех.
    fn live_tabs(n: u64) -> crate::tabs::TabStore {
        let base = Instant::now();
        let mut s = crate::tabs::TabStore::default();
        for i in 0..n {
            let id = s.add(format!("https://t{i}.example/"), String::new(), false, None);
            if let Some(tab) = s.get_mut(id) {
                tab.last_active = base + Duration::from_secs(i);
            }
        }
        s
    }

    /// Кого усыпит лимит живых вкладок, как `App::limit_live_tabs`.
    fn over_limit(s: &crate::tabs::TabStore, limit: usize) -> Vec<TabId> {
        let candidates: Vec<_> = s
            .iter()
            .filter(|t| t.may_sleep())
            .map(|t| (t.last_active, t.id))
            .collect();
        let keep = room(limit, s.len(), candidates.len());
        excess(candidates, keep)
    }

    #[test]
    fn no_sleep_tab_is_never_chosen() {
        let mut s = live_tabs(4);
        assert_eq!(over_limit(&s, 3), vec![1]);
        // Самую давнюю вкладку попросили не усыплять.
        if let Some(tab) = s.get_mut(1) {
            tab.no_sleep = true;
        }
        assert!(s.get(1).is_some_and(|t| !t.may_sleep()));
        // Она живая и занимает место в лимите: вместо неё засыпает следующая по давности.
        assert_eq!(over_limit(&s, 3), vec![2]);
        assert_eq!(over_limit(&s, 1), vec![4, 3, 2]);
        assert_eq!(over_limit(&s, 4), Vec::<TabId>::new());
        // Не усыплять попросили все: никто не засыпает, даже сверх лимита.
        for tab in s.iter_mut() {
            tab.no_sleep = true;
        }
        assert_eq!(over_limit(&s, 1), Vec::<TabId>::new());
    }

    #[test]
    fn may_sleep_respects_every_flag() {
        let flags: [fn(&mut crate::tabs::Tab); 3] = [
            |t| t.keep_alive = true,
            |t| t.sleep_requested = Some(Instant::now()),
            |t| t.no_sleep = true,
        ];
        assert!(live_tabs(1).iter().all(|t| t.may_sleep()));
        for set in flags {
            let mut s = live_tabs(1);
            s.iter_mut().for_each(set);
            assert!(s.iter().all(|t| !t.may_sleep()));
        }
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
