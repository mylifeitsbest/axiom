//! Пробуждение вкладки: когда уснувшую страницу пора показать.
//!
//! До первой отрисовки WebView2 показывает только фон, а некоторые сайты сначала
//! отдают пустую страницу-проверку («Please wait…» у TikTok) и лишь потом себя.
//! Поэтому, пока страница грузится, на её месте заглушка интерфейса, а саму
//! страницу показываем, когда в ней появилось что-то кроме пары слов.

use std::time::Duration;

/// Как часто спрашивать страницу, нарисовалась ли она.
pub const STEP: Duration = Duration::from_millis(250);
/// Дольше заглушку не держим: недогруженная страница показывается как есть.
pub const LIMIT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Пора спросить страницу.
    Check,
    /// Ответ страницы на [`PROBE`].
    Probed(bool),
}

/// Вопрос странице: есть ли в ней что показать. Нужны первая отрисовка
/// (спрятанная страница не рисуется, там хватит готового DOM) и содержимое:
/// картинка, видео, поле ввода или больше пары слов текста.
pub const PROBE: &str = "(() => {
  const b = document.body;
  if (!b) return false;
  const painted = document.visibilityState === 'hidden'
    || performance.getEntriesByType('paint').some((e) => e.name === 'first-contentful-paint');
  if (!painted) return false;
  const media = 'img,picture,video,canvas,svg,iframe,embed,object,textarea,input:not([type=hidden])';
  const seen = (el) => { const r = el.getBoundingClientRect(); return r.width > 1 && r.height > 1; };
  if ([...b.querySelectorAll(media)].some(seen)) return true;
  return (b.innerText || '').trim().length > 40;
})()";

/// Ответ скрипта приходит в JSON; невнятный ответ — страница ещё не готова.
pub fn parse(raw: &str) -> bool {
    raw.trim() == "true"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_answer() {
        assert!(parse("true"));
        assert!(parse(" true\n"));
        assert!(!parse("false"));
        assert!(!parse("null"));
        assert!(!parse(""));
    }
}
