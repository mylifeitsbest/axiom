// Баннеры cookie, которых нет в списке известных (banners.css): по имени блока и по
// тексту. Прячем только то, что приклеено к экрану (position: fixed или sticky) —
// у обычных блоков страницы бывают те же слова.
let settings = axiom.settings;
const GENERIC = [
  '[id*="cookie-consent" i]', '[class*="cookie-consent" i]', '[id*="cookie-banner" i]', '[class*="cookie-banner" i]',
  '[id*="cookie-notice" i]', '[class*="cookie-notice" i]', '[id*="cookie-law" i]', '[class*="cookie-law" i]',
  '[id*="gdpr-banner" i]', '[class*="gdpr-banner" i]', '[id*="cookiebanner" i]', '[class*="cookiebanner" i]',
  '[id*="cookieconsent" i]', '[class*="cookies-banner" i]', '[id*="cookies-banner" i]',
].join(',');
// Слова в тексте баннера. «Политика cookie» в ссылке — не баннер: текст должен быть фразой.
const WORDS = /cookie|куки/i;
const CONTROLS = 'button, [role=button], input[type=button], input[type=submit], a';
// Сколько ещё следить за страницей: баннеры появляются через пару секунд после загрузки.
const WATCH_MS = 30000;
let hidden = 0;

const pinned = (el) => {
  const p = getComputedStyle(el).position;
  return p === 'fixed' || p === 'sticky';
};
const area = (el) => {
  const r = el.getBoundingClientRect();
  return (r.width * r.height) / Math.max(1, innerWidth * innerHeight);
};
const textLen = (el) => (el.textContent || '').replace(/\s+/g, ' ').trim().length;
// Полупрозрачный слой поверх страницы.
const dim = (el) => {
  const s = getComputedStyle(el);
  if (s.zIndex === 'auto' || +s.zIndex < 1) return false;
  const a = s.backgroundColor.match(/[\d.]+/g);
  const alpha = a && a.length > 3 ? +a[3] : a ? 1 : 0;
  return (alpha > 0.05 && alpha < 0.95) || +s.opacity < 1 || s.backdropFilter !== 'none';
};
const banned = (el) => !el || el === document.body || el === document.documentElement || /^(HEADER|NAV|MAIN)$/.test(el.tagName) || !!el.querySelector('nav');

// Ближайший приклеенный к экрану предок (или сам элемент).
const pinnedUp = (el, levels) => {
  for (let i = 0; el && i <= levels; i++, el = el.parentElement) {
    if (el === document.body || el === document.documentElement) return null;
    if (pinned(el)) return el;
  }
  return null;
};

const hide = (el) => {
  if (el.hasAttribute('data-axiom-cookie')) return;
  el.style.setProperty('display', 'none', 'important');
  el.setAttribute('data-axiom-cookie', '');
  hidden++;
  // Затемнение под окном: соседний полупрозрачный блок поверх страницы на весь экран почти без
  // текста (фоновые слои сайта — под страницей, с z-index ниже нуля или непрозрачные — не трогаем).
  for (const sib of el.parentElement ? el.parentElement.children : []) {
    if (sib !== el && !banned(sib) && pinned(sib) && area(sib) > 0.85 && textLen(sib) < 20 && dim(sib)) {
      sib.style.setProperty('display', 'none', 'important');
      sib.setAttribute('data-axiom-cookie', '');
    }
  }
  unlock();
};

// Сайт запретил прокрутку, пока баннер открыт, — возвращаем её.
const unlock = () => {
  for (const el of [document.documentElement, document.body]) {
    if (el && getComputedStyle(el).overflowY === 'hidden') el.setAttribute('data-axiom-unlock', '');
  }
};

const byName = () => {
  for (const el of document.querySelectorAll(GENERIC)) {
    if (banned(el) || el.hasAttribute('data-axiom-cookie')) continue;
    const box = pinnedUp(el, 3);
    if (box && !banned(box) && textLen(box) < 3000) hide(box);
  }
};

const byText = () => {
  if (!settings.text || !document.body) return;
  const found = document.evaluate(
    "//body//text()[contains(translate(., 'COKIE', 'cokie'), 'cookie') or contains(translate(., 'КУИ', 'куи'), 'куки')]",
    document.body, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null,
  );
  for (let i = 0; i < Math.min(found.snapshotLength, 60); i++) {
    const node = found.snapshotItem(i);
    const parent = node.parentElement;
    // Фраза, а не подпись ссылки «Политика cookie».
    if (!parent || parent.closest('a, script, style, noscript') || node.textContent.trim().length < 30 || !WORDS.test(node.textContent)) continue;
    const box = pinnedUp(parent, 8);
    if (!box || banned(box) || box.hasAttribute('data-axiom-cookie')) continue;
    const r = box.getBoundingClientRect();
    const atEdge = r.top < 80 || r.bottom > innerHeight - 80;
    if (textLen(box) < 1500 && box.querySelector(CONTROLS) && (atEdge || area(box) < 0.6)) hide(box);
  }
};

let queued = false;
const scan = () => {
  queued = false;
  byName();
  byText();
};
const schedule = () => {
  if (queued) return;
  queued = true;
  setTimeout(scan, 300);
};

const start = () => {
  scan();
  const observer = new MutationObserver(schedule);
  observer.observe(document.documentElement, { childList: true, subtree: true });
  setTimeout(() => observer.disconnect(), WATCH_MS);
  // Известные баннеры спрятал CSS: если сайт при этом запретил прокрутку — вернём её.
  setTimeout(() => {
    if (document.querySelector('#onetrust-consent-sdk, #CybotCookiebotDialog, #didomi-host, #qc-cmp2-container, #usercentrics-root, .cky-consent-container')) unlock();
  }, 1500);
};
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
else start();

axiom.on('settings', (next) => {
  settings = next;
  schedule();
});
